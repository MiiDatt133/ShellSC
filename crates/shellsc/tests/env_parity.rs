//! Environment parity regressions: `$0` visibility inside functions, and
//! the exported-vs-plain split in the process environment (spawn() inherits
//! the process env verbatim, so a plain `x=1` must never appear there).
//!
//! Unit-level coverage lives in shell_vm::env tests; these run the full
//! lexer -> IR -> bytecode -> VM pipeline like the original repros.

use shell_bc::{assemble_sbc, compile_to_sbc, Bytecode};
use shell_vm::Vm;

fn build(src: &str) -> Bytecode {
    let tokens = shell_lex::Lexer::new(src).tokenize().expect("lex failed");
    let script = shell_parse::Parser::new(tokens)
        .parse()
        .expect("parse failed");
    let chunk = shell_ir::Lowerer::new()
        .lower(&script)
        .expect("lower failed");
    assemble_sbc(&compile_to_sbc(&chunk)).expect("assemble failed")
}

fn run(src: &str) -> i32 {
    let mut vm = Vm::new(build(src));
    vm.set_arg0("test");
    vm.run().expect("vm run failed").code()
}

#[test]
fn arg0_visible_inside_function() {
    // Before the fix, the function frame's `#` marker shadowed `$0` in the
    // positional lookup and expand() fell back to "shellsc".
    assert_eq!(
        run("f() { [ \"$0\" = test ]; }; f"),
        0,
        "$0 inside a function must keep the script name"
    );
}

#[test]
fn plain_assignment_stays_out_of_process_env() {
    run("sc_e2e_plain=LEAK");
    assert_eq!(
        std::env::var("sc_e2e_plain"),
        Err(std::env::VarError::NotPresent),
        "non-exported assignment must not reach the process environment"
    );
}

#[test]
fn exported_assignment_reaches_process_env() {
    // Guards the other half of the fix: exported names still sync, so
    // children spawned via Command inherit the updated value.
    run("export sc_e2e_exp=ONE; sc_e2e_exp=TWO");
    assert_eq!(
        std::env::var("sc_e2e_exp").as_deref(),
        Ok("TWO"),
        "reassigning an exported var must update the process environment"
    );
    std::env::remove_var("sc_e2e_exp");
}

#[test]
fn prefix_assignment_passed_to_child_then_restored() {
    // `VAR=val cmd` — the child must see val; the shell env must revert.
    assert_eq!(
        run("x=OUTER; x=TEMP sh -c '[ \"$x\" = TEMP ]'; [ \"$x\" = OUTER ]"),
        0,
        "prefix assignment must be visible to the child and restored after"
    );
}

#[test]
fn prefix_assignment_not_exported_before() {
    // A variable that was never exported must not leak into the process env
    // after a `VAR=val cmd` statement finishes.
    run("sc_e2e_pre=V sh -c ':'");
    assert_eq!(
        std::env::var("sc_e2e_pre"),
        Err(std::env::VarError::NotPresent),
        "prefix assignment of a non-exported var must not persist in process env"
    );
}
