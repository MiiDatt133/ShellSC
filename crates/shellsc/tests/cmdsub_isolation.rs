//! `$(...)` runs in a subshell: assignments made inside it must not leak
//! back into the enclosing shell. Before this, a function body invoked from
//! a command substitution wrote its locals into the caller's frame, so
//! `a=$(f 3)` in `fib` returned the wrong value and `fib 8` produced 4.

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
fn assignment_inside_cmdsub_does_not_leak() {
    assert_eq!(
        run("a=OUTER; b=$(a=INNER; echo 1); [ \"$a\" = OUTER ]"),
        0,
        "assignment inside $( ) must not survive into the caller"
    );
}

#[test]
fn new_variable_inside_cmdsub_does_not_leak() {
    assert_eq!(
        run("b=$(fresh=1; echo 1); [ -z \"$fresh\" ]"),
        0,
        "variables created inside $( ) must vanish at the closing paren"
    );
}

#[test]
fn recursive_function_in_cmdsub_returns_full_result() {
    // fib 8 = 21. Leaked frames truncated the inner calls, yielding 4.
    assert_eq!(
        run(
            "fib() { if [ \"$1\" -le 1 ]; then echo \"$1\"; return; fi; \
             a=$(fib $(($1-1))); b=$(fib $(($1-2))); echo $(($a+$b)); }; \
             [ \"$(fib 8)\" = 21 ]"
        ),
        0
    );
}
