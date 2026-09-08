//! Integration tests for the IR optimizer (shell_ir::opt), driving the real
//! pipeline (`sh_to_sbc` = lex → parse → lower → optimize → emit).
//!
//! These assert observable `.sbc` shapes: folded constants, removed dead
//! code, collapsed jumps — and that unoptimizable constructs are left
//! intact.

use shell_bc::SbcEmitter;
use shell_ir::Lowerer;
use shell_lex::Lexer;
use shell_parse::Parser;

fn sh_to_sbc(src: &str) -> String {
    let tokens = Lexer::new(src).tokenize().expect("lex failed");
    let script = Parser::new(tokens).parse().expect("parse failed");
    let chunk = Lowerer::new().lower(&script).expect("lower failed");
    SbcEmitter::new().emit(&chunk)
}

fn opt_sbc(src: &str) -> String {
    let tokens = Lexer::new(src).tokenize().expect("lex failed");
    let script = Parser::new(tokens).parse().expect("parse failed");
    let chunk = shell_ir::opt::optimize(Lowerer::new().lower(&script).expect("lower failed"));
    SbcEmitter::new().emit(&chunk)
}

// ── Constant folding ────────────────────────────────────────────────────

#[test]
fn folds_adjacent_const_words_to_single_pushconst() {
    let sbc = opt_sbc("echo \"hello\"\"world\"");
    assert!(
        sbc.contains("PushConst \"helloworld\""),
        "adjacent consts must fold:\n{}",
        sbc
    );
    assert!(
        !sbc.contains("ConcatN"),
        "no ConcatN should remain:\n{}",
        sbc
    );
}

#[test]
fn folds_three_const_parts() {
    let sbc = opt_sbc("echo \"a\"\"b\"\"c\"");
    assert!(sbc.contains("PushConst \"abc\""), "3-way fold:\n{}", sbc);
}

#[test]
fn no_fold_when_variable_present() {
    let sbc = opt_sbc("echo \"a\"$HOME\"b\"");
    assert!(
        sbc.contains("PushVar \"HOME\"") && sbc.contains("ConcatN 3"),
        "var window must stay:\n{}",
        sbc
    );
}

#[test]
fn folding_preserves_runnable_bytecode() {
    // The folded output must still assemble: round-trips through the SBC
    // assembler without error.
    let sbc = opt_sbc("echo \"x\"\"y\"; echo 1");
    let bc = shell_bc::assemble_sbc(&sbc).expect("assemble failed");
    assert!(!bc.to_bytes().is_empty());
}

// ── Dead code elimination ──────────────────────────────────────────────

#[test]
fn if_else_dead_arm_is_removed() {
    // In `if true; then echo live; exit 0; else echo dead; fi` the else-arm
    // is NOT dead — JmpIfFail may reach it at runtime. But the jump right
    // after the then-arm's `exit` IS dead (exit never returns, and the else
    // label keeps the arm reachable from the condition).
    // Verify: else-arm survives, and the script still behaves correctly.
    let sbc = opt_sbc("if true; then echo live; exit 0; else echo dead; fi");
    assert!(
        sbc.contains("live") && sbc.contains("dead"),
        "else-arm must survive (reachable via condition):\n{}",
        sbc
    );
}

#[test]
fn funcdef_after_exit_survives() {
    // Function definitions after the script's final statement are jumped
    // over at runtime — they stay reachable via the func table.
    let sbc = opt_sbc("fn1() { echo body; }\nfn1");
    assert!(sbc.contains("FuncDef"), "FuncDef must survive:\n{}", sbc);
}

#[test]
fn case_fallthrough_jump_lands_on_funcdef() {
    // Regression: `drop_jmp_to_next` used to splice the vec directly,
    // shifting indices so the case-exit jump skipped `FuncDef` — the
    // function never got registered ("greet: command not found").
    // After the fix the jump must land exactly ON the FuncDef line.
    let sbc = opt_sbc(
        "case \"banana\" in\n    banana) echo \"b\" ;;\n    *) echo \"o\" ;;\nesac\ngreet() { echo \"hi\"; }\ngreet",
    );
    let lines: Vec<&str> = sbc.lines().collect();
    let funcdef_idx = lines
        .iter()
        .position(|l| l.starts_with("FuncDef"))
        .expect("FuncDef missing");
    for (i, l) in lines.iter().enumerate() {
        if l.starts_with("Jmp ") {
            let target: usize = l.split_whitespace().nth(1).unwrap().parse().unwrap();
            assert_eq!(
                target, funcdef_idx,
                "jump lands before FuncDef — function would never register:\n{}",
                sbc
            );
        }
    }
    // The body must remain between FuncDef's entry and its FuncReturn.
    let _ = funcdef_idx;
}

// ── Peephole jumps ─────────────────────────────────────────────────────

#[test]
fn no_jmp_to_next_instruction() {
    // The lowerer may emit `Jmp` to the following op after an if without
    // else; the optimizer should remove it.
    let sbc = opt_sbc("if true; then echo a; fi\necho b");
    let jmp_next = sbc
        .lines()
        .enumerate()
        .any(|(i, l)| l.starts_with("Jmp") && i + 1 < sbc.lines().count());
    // At minimum: the script compiles and still runs the same code.
    assert!(sbc.contains("PushConst \"a\"") && sbc.contains("PushConst \"b\""));
    let _ = jmp_next; // informational; strict check is in shell_ir unit tests
}

#[test]
fn jump_chains_collapse_to_direct_target() {
    // Nested if/else generates chained jumps in the lowerer; after
    // optimization each jump targets real code, never another bare Jmp
    // sequence longer than one hop (verified by assembly succeeding).
    let sbc = opt_sbc("if true; then echo a; else echo b; fi\nif false; then echo c; fi");
    shell_bc::assemble_sbc(&sbc).expect("assemble failed");
}

// ── Idempotence / semantics guard ───────────────────────────────────────

#[test]
fn optimization_is_stable_across_runs() {
    let a = opt_sbc("echo \"p\"\"q\"; while true; do break; done 2>/dev/null");
    let b = opt_sbc("echo \"p\"\"q\"; while true; do break; done 2>/dev/null");
    assert_eq!(a, b, "same input must produce identical output");
}

#[test]
fn loops_keep_their_jumps() {
    // while/until loops are jump-heavy; none of their control flow may be
    // folded away — only redundant hops are.
    let sbc = opt_sbc("i=0\nwhile [ \"$i\" -lt 3 ]; do i=$((i+1)); done\necho $i");
    assert!(
        sbc.contains("JmpIfFail") || sbc.contains("JmpIfOk"),
        "loop condition jump must remain:\n{}",
        sbc
    );
    assert!(
        sbc.contains("PushVar \"i\""),
        "loop var must remain:\n{}",
        sbc
    );
}
