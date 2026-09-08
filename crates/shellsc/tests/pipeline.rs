//! End-to-end pipeline tests: source → lex → parse → lower → assemble.
//!
//! These exercise compile-time behaviour only (no ELF execution):
//! parsing succeeds, expected opcodes appear, and runtime-critical
//! lowering (loop redirects, field splitting, positional scoping)
//! produces the right instruction shapes.

use shell_bc::{assemble_sbc, SbcEmitter};
use shell_ir::Lowerer;
use shell_lex::Lexer;
use shell_parse::Parser;

fn compile_to_sbc(src: &str) -> String {
    let tokens = Lexer::new(src).tokenize().expect("lex failed");
    let script = Parser::new(tokens).parse().expect("parse failed");
    let chunk = Lowerer::new().lower(&script).expect("lower failed");
    SbcEmitter::new().emit(&chunk)
}

fn compile_ok(src: &str) {
    compile_to_sbc(src);
}

// ── Basic pipeline ──────────────────────────────────────────────────────

#[test]
fn sbc_not_empty() {
    let sbc = compile_to_sbc("echo hello");
    assert!(!sbc.is_empty());
}

#[test]
fn sbc_contains_exit() {
    let sbc = compile_to_sbc("echo hello");
    assert!(sbc.contains("Exit"), "missing Exit:\n{}", sbc);
}

#[test]
fn sbc_assembles_roundtrip() {
    let sbc = compile_to_sbc("echo hi; ls | wc -l");
    let bc = assemble_sbc(&sbc).expect("assemble failed");
    assert!(!bc.to_bytes().is_empty());
}

// ── until loop ─────────────────────────────────────────────────────────

#[test]
fn until_loop_compiles() {
    compile_ok("until [ \"$i\" -ge 3 ]; do i=$((i+1)); done");
}

#[test]
fn until_uses_jmpifok() {
    let sbc = compile_to_sbc("until false; do :; done");
    assert!(
        sbc.contains("JmpIfOk"),
        "until must exit on success:\n{}",
        sbc
    );
}

#[test]
fn while_uses_jmpiffail() {
    let sbc = compile_to_sbc("while false; do :; done");
    assert!(
        sbc.contains("JmpIfFail"),
        "while must exit on failure:\n{}",
        sbc
    );
}

// ── Loop redirects (regression: `done <<EOT` used to be parsed apart) ──

#[test]
fn while_loop_heredoc_redirect_parses() {
    compile_ok("while read line; do echo $line; done <<EOT\none\ntwo\nEOT");
}

#[test]
fn while_loop_redirect_emits_redirsave() {
    let sbc = compile_to_sbc("while read l; do echo $l; done <<EOT\nx\nEOT");
    assert!(
        sbc.contains("RedirSave") && sbc.contains("RedirRestore"),
        "loop redirects must be scoped:\n{}",
        sbc
    );
    // The heredoc redirect must be set BEFORE the loop body runs.
    let save_pos = sbc.find("RedirSave").unwrap();
    let redir_pos = sbc.find("Redirect").unwrap();
    let restore_pos = sbc.find("RedirRestore").unwrap();
    assert!(save_pos < redir_pos && redir_pos < restore_pos);
}

#[test]
fn for_loop_file_redirect_parses() {
    compile_ok("for i in 1 2; do echo $i; done > /dev/null");
}

// ── Field splitting (regression: `for x in $var` was one word) ──────────

#[test]
fn unquoted_var_emits_globexpand() {
    let sbc = compile_to_sbc("for x in $list; do echo $x; done");
    assert!(
        sbc.contains("GlobExpand"),
        "unquoted $var must be field-split:\n{}",
        sbc
    );
}

#[test]
fn quoted_var_no_globexpand() {
    let sbc = compile_to_sbc("for x in \"$list\"; do echo \"$x\"; done");
    assert!(
        !sbc.contains("GlobExpand"),
        "quoted \"$var\" must not split:\n{}",
        sbc
    );
}

// ── Positional param scoping (regression: $1 leaked into no-arg fn) ─────
// Scoping is runtime (Env), but the lowering shape is stable: a call
// pushes its own args. Compile-time check here; runtime covered by
// fixtures/differential tests.

#[test]
fn function_call_no_args_compiles() {
    compile_ok("fn() { echo \"$1\"; }\nfn");
}

// ── Redirect last-wins / compound scoping ───────────────────────────────

#[test]
fn brace_group_redir_scoped() {
    let sbc = compile_to_sbc("{ echo a; echo b; } >file 2>/dev/null");
    assert!(
        sbc.contains("RedirSave"),
        "brace redirs must be scoped:\n{}",
        sbc
    );
}

#[test]
fn stdout_to_stderr_then_null_compiles() {
    compile_ok("echo x 1>&2 >/dev/null");
}

// ── printf / test operator coverage (compile-time smoke) ───────────────

#[test]
fn printf_formats_parse() {
    compile_ok("printf \"%x %5s %-5s %.2f\\n\" 255 ab ab 3.14");
}

#[test]
fn test_string_lt_escapes_parse() {
    compile_ok("[ \"a\" \\< \"b\" ] && echo lt");
}

// ── Adjacent quoting (regression: `"hello""world"` was 2 argv words) ──

#[test]
fn adjacent_quoted_strings_one_echo_arg() {
    // `"hello""world"` must be ONE word — check the Builtin arity in the SBC.
    let sbc = compile_to_sbc("echo \"hello\"\"world\"");
    assert!(
        sbc.contains("Builtin echo 1"),
        "adjacent quoted strings must merge into one word:\n{}",
        sbc
    );
}

#[test]
fn quoted_unquoted_mix_one_arg() {
    let sbc = compile_to_sbc("echo ab\"cd\"ef 'x'\"y\"");
    assert!(
        sbc.contains("Builtin echo 2"),
        "adjacent fragments merge; 'x'\"y\" is a second word:\n{}",
        sbc
    );
}
