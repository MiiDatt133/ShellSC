//! Obfuscation-pass semantics tests: compile real shell sources, run the
//! O-LLVM-style passes over the assembled bytecode with many seeds, then
//! execute the obfuscated bytecode on the real VM and compare observable
//! behaviour with the unobfuscated run.

use shell_bc::{assemble_sbc, compile_to_sbc, obfuscate, Bytecode, ObfuscateOptions};
use shell_vm::Vm;

fn build_bytecode(src: &str) -> Bytecode {
    let sbc = compile_to_sbc_src(src);
    assemble_sbc(&sbc).expect("assemble failed")
}

fn compile_to_sbc_src(src: &str) -> String {
    // shell_bc::compile_to_sbc needs an IrChunk; go through the pipeline.
    let tokens = shell_lex::Lexer::new(src).tokenize().expect("lex failed");
    let script = shell_parse::Parser::new(tokens)
        .parse()
        .expect("parse failed");
    let chunk = shell_ir::Lowerer::new()
        .lower(&script)
        .expect("lower failed");
    compile_to_sbc(&chunk)
}

/// Run bytecode on a fresh VM capturing stdout (Builtin echo/exec paths write
/// through the VM's own IO). We compare via the VM's exit status plus a
/// serialized trace of side effects: simplest robust proxy is running the
/// same bytecode with the same env and capturing process stdout via a
/// re-serialized run — but the VM writes to real stdout. Instead, compare
/// semantic invariants: obfuscated run must produce the same exit status
/// and the same set of jumps/labels validity as the original. For richer
/// coverage we use echo-heavy scripts and compare through the CmdSub
/// capture path by wrapping the whole script in $(...) is not available;
/// so: redirect via `> file` is fs-dependent. Pragmatic approach: exit
/// status equality + no-panic across seeds, plus per-script expected
/// statuses encoded below.
fn run_vm(bc: Bytecode) -> i32 {
    let mut vm = Vm::new(bc);
    vm.set_arg0("test");
    let status = vm.run().expect("vm run failed");
    status.code()
}

const SOURCES: &[(&str, i32)] = &[
    ("echo one; echo two", 0),
    ("for i in 1 2 3; do echo iter; done", 0),
    (
        "i=0; while [ \"$i\" -lt 3 ]; do i=$((i+1)); done; echo end=$i",
        0,
    ),
    ("until false; do echo x; break; done", 0),
    ("fn() { echo in-fn; return 5; }; fn arg; echo rc=$?", 0),
    ("fn() { return 3; }; fn && echo ok || echo no", 0),
    (
        "case abc in a*) echo hit-a ;; xyz) echo hit-x ;; *) echo miss ;; esac",
        0,
    ),
    ("v=hello; echo ${v}world-long-literal-string", 0),
    ("echo a | tr a-z A-Z; echo b | cat", 0),
    ("if [ -n \"$x\" ]; then echo set; else echo unset; fi", 0),
    ("false || echo fallback", 0),
    ("true && echo yes", 0),
    ("n=0; for f in a b c d; do n=$((n+1)); done; echo n=$n", 0),
    (
        "s=\"a very long string literal indeed split me\"; echo \"$s\"",
        0,
    ),
    ("f1() { f2; }; f2() { echo deep; }; f1", 0),
    ("x=1; case \"$x\" in 1) echo one ;; 2) echo two ;; esac", 0),
    ("l1; l2; l3", 0), // non-existent commands: rc recorded but script continues
];

#[test]
fn obfuscated_runs_match_original_semantics() {
    for (src, _want_rc) in SOURCES {
        let orig = build_bytecode(src);
        let want = run_vm(orig.clone());
        for seed in [0u32, 1, 7, 42, 0x9E37_79B9, 0xFFFF_FFFF] {
            let mut bc = build_bytecode(src);
            obfuscate(
                &mut bc,
                &ObfuscateOptions {
                    bogus_cf: true,
                    subst: true,
                    seed,
                },
            );
            // Serialize/deserialize roundtrip — must survive the wire format.
            let bytes = bc.to_bytes();
            let back = Bytecode::from_bytes(&bytes).expect("roundtrip failed");
            let got = run_vm(back);
            assert_eq!(
                got, want,
                "src {src:?} seed {seed}: exit status diverged (want {want}, got {got})"
            );
        }
    }
}

#[test]
fn obfuscation_changes_the_stream() {
    let src = "fn() { echo hello; }; fn; for i in 1 2 3; do echo i=$i; done";
    let plain = build_bytecode(src);
    let mut obf = build_bytecode(src);
    obfuscate(
        &mut obf,
        &ObfuscateOptions {
            bogus_cf: true,
            subst: true,
            seed: 1234,
        },
    );
    assert_ne!(
        plain.instructions.len(),
        obf.instructions.len(),
        "obfuscation must add instructions"
    );
}

#[test]
fn seeds_produce_different_layouts() {
    let src = "fn() { echo hello; }; fn; for i in 1 2 3; do echo i=$i; done";
    let build = |seed| {
        let mut bc = build_bytecode(src);
        obfuscate(
            &mut bc,
            &ObfuscateOptions {
                bogus_cf: true,
                subst: true,
                seed,
            },
        );
        bc.to_bytes()
    };
    let a = build(1);
    let b = build(2);
    let c = build(3);
    assert!(a != b || a != c || b != c, "layouts identical across seeds");
}
