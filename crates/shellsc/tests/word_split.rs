//! Field-splitting regressions:
//! - unquoted `$(...)` in a word must word-split like `$var` (for-list,
//!   argv, assignments excluded — assignment RHS keeps the raw string)
//! - redirect targets that expand to 0 or >1 fields are an "ambiguous
//!   redirect": statement fails with $?=1, command does not run, the
//!   script continues (previously: stack underflow crash / silent wrong
//!   file / leaked surplus fields)
//! - `> /nodir/f` open failure fails the statement instead of being
//!   silently ignored

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
    let sbc = compile_to_sbc(&chunk);
    assemble_sbc(&sbc).expect("assemble failed")
}

fn run(src: &str) -> i32 {
    let mut vm = Vm::new(build(src));
    vm.set_arg0("test");
    vm.run().expect("vm run failed").code()
}

#[test]
fn for_list_cmdsub_word_splits() {
    assert_eq!(
        run("n=0; for i in $(printf 'a b c'); do n=$((n+1)); done; [ \"$n\" -eq 3 ]"),
        0,
        "for $(cmd) must iterate per IFS field"
    );
}

#[test]
fn argv_cmdsub_word_splits() {
    assert_eq!(
        run("set -- $(printf 'p q'); [ $# -eq 2 ]"),
        0,
        "argv $(cmd) must word-split"
    );
}

#[test]
fn assignment_rhs_keeps_cmdsub_raw() {
    assert_eq!(
        run("v=$(printf 'm n'); [ \"$v\" = 'm n' ]"),
        0,
        "assignment RHS must not field-split"
    );
}

#[test]
fn backtick_cmdsub_word_splits() {
    assert_eq!(
        run("n=0; for i in `printf 'a b'`; do n=$((n+1)); done; [ \"$n\" -eq 2 ]"),
        0,
        "backtick substitution must word-split like $()"
    );
}

#[test]
fn ambiguous_redirect_empty_target_fails_statement() {
    // Used to die with "stack underflow"; must now fail $?=1 and continue.
    assert_eq!(run("echo hi > $unsetvar; [ $? -eq 1 ]"), 0);
}

#[test]
fn ambiguous_redirect_multifield_fails_statement() {
    assert_eq!(run("f=\"a b\"; echo hi > $f; [ $? -eq 1 ]"), 0);
}

#[test]
fn command_after_ambiguous_redirect_still_runs() {
    assert_eq!(
        run("echo hi > $unsetvar; echo next; [ $? -eq 0 ]"),
        0,
        "the ambiguous-redirect flag must not skip the next statement"
    );
}

#[test]
fn redirect_open_failure_fails_statement() {
    assert_eq!(run("echo hi > /shellsc_nodir_xyz/f; [ $? -eq 1 ]"), 0);
}

#[test]
fn valid_cmdsub_redirect_still_works() {
    assert_eq!(
        run("printf ok > /dev/null; [ $? -eq 0 ]"),
        0,
        "single-field redirect targets must behave as before"
    );
}
