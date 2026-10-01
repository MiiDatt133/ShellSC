//! LLVM-style obfuscation passes over the assembled bytecode (O-LLVM
//! `bcf` + `sub` principles, applied at the bytecode tier instead of
//! machine code):
//!
//! 1. **Bogus control flow** — for a seed-chosen subset of unconditional
//!    `Jmp`s, retarget the jump to a decoy block inserted directly after
//!    it. The decoy runs a few stack-neutral junk instructions and jumps
//!    to the original target. A static CFG analysis sees extra blocks
//!    and edges indistinguishable from real ones. The block must stay
//!    inside the Jmp's own `ip_fence` region (pipeline-subshell bodies,
//!    func calls), so it is inserted inline, not appended at the end.
//!
//! 2. **Instruction substitution** — `PushConst(s)` is split into
//!    `PushConst(a) PushConst(b) ConcatN(2)` at a seed-chosen cut point.
//!    The VM's `pop_n` returns slots oldest-first and `ConcatN` joins in
//!    that order, so the concatenation is `a + b == s` — semantics
//!    preserved, but the pool no longer holds the full literal in one
//!    piece for a static string-dump to read.
//!
//! 3. **Conditional double negation** — `JmpIfFail T` becomes
//!    `JmpIfOk SKIP; Jmp T; SKIP:` and vice versa: the branch condition is
//!    inverted, the fall-through path hops over an inserted unconditional
//!    jump. Status is never mutated, so `$?` at the target and fall-through
//!    is byte-identical to the original. Exception: a `JmpIfFail` directly
//!    after `ForBind` is left alone — its `for_exit_status.take()` side
//!    effect (for-loop exit status restoration) has no equivalent here.
//!
//! 4. **Arithmetic neutral append** — before `ArithEvalStack`, insert
//!    `PushConst("+0") ConcatN(2)` so the evaluated expression becomes
//!    `expr+0`. `+` is the lowest-precedence operator in the VM's
//!    ArithParser, so the appended term never changes the value.
//!
//! All passes are seed-driven: two protected builds of the same script
//! get different cut points and decoy placements.

use crate::{
    bytecode::{Bytecode, Instruction},
    const_pool::ConstPool,
    opcode::Opcode,
};

#[derive(Debug, Clone, Copy)]
pub struct ObfuscateOptions {
    pub bogus_cf: bool,
    pub subst: bool,
    pub condneg: bool,
    pub arith_neutral: bool,
    pub seed: u32,
}

/// Minimal xorshift PRNG (same shape as shell_pack's, kept local so this
/// crate has no dependency on the packer).
struct Rng(u64);

impl Rng {
    fn new(seed: u32) -> Self {
        Self((seed as u64) | 1)
    }
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    /// Pseudo-boolean with probability num/den.
    fn chance(&mut self, num: u32, den: u32) -> bool {
        (self.next() % den as u64) < num as u64
    }
}

pub fn obfuscate(bc: &mut Bytecode, opts: &ObfuscateOptions) {
    if opts.subst {
        substitute_consts(bc, opts.seed);
    }
    if opts.bogus_cf {
        bogus_control_flow(bc, opts.seed);
    }
    if opts.condneg {
        cond_double_neg(bc, opts.seed);
    }
    if opts.arith_neutral {
        arith_neutral(bc, opts.seed);
    }
    if std::env::var_os("SHELLSC_OBF_DEBUG").is_some() {
        for (i, ins) in bc.instructions.iter().enumerate() {
            eprintln!("ins[{i}]: {:?} {}", ins.op, ins.operand);
        }
    }
}

// ── Pass 2: instruction substitution ─────────────────────────────────────────

fn substitute_consts(bc: &mut Bytecode, seed: u32) {
    let mut rng = Rng::new(seed ^ 0x5B7E_C0F5);
    let instrs = std::mem::take(&mut bc.instructions);
    let mut pool: ConstPool = std::mem::take(&mut bc.const_pool);
    let mut out = Vec::with_capacity(instrs.len() + instrs.len() / 2);
    // One old instruction index may expand into several, so every
    // label-carrying operand must be remapped: a jump aimed at the split
    // PushConst should land on the first piece.
    let mut label_map = vec![0u32; instrs.len()];

    for (old_i, instr) in instrs.into_iter().enumerate() {
        label_map[old_i] = out.len() as u32;
        // Only a long single-piece string is worth splitting, and only
        // when the seed says so — leave short strings and some long ones
        // alone so the split pattern is not uniform.
        if instr.op == Opcode::PushConst && instr.operand < 0x00FF_FFFF {
            let s = match pool.get(instr.operand) {
                Some(s) => s.to_string(),
                None => {
                    out.push(instr);
                    continue;
                }
            };
            let len = s.chars().count();
            if len >= 3 && rng.chance(7, 10) {
                let cut = (rng.next() % (len as u64 - 1)) as usize + 1;
                let head: String = s.chars().take(cut).collect();
                let tail: String = s.chars().skip(cut).collect();
                let a = pool.intern(&head);
                let b = pool.intern(&tail);
                out.push(Instruction::new(Opcode::PushConst, a));
                out.push(Instruction::new(Opcode::PushConst, b));
                out.push(Instruction::new(Opcode::ConcatN, 2));
                continue;
            }
        }
        out.push(instr);
    }

    for ins in out.iter_mut() {
        if matches!(
            ins.op,
            Opcode::Jmp | Opcode::JmpIfFail | Opcode::JmpIfOk | Opcode::PipeSubshellBegin
        ) {
            ins.operand = label_map[ins.operand as usize];
        }
    }
    for f in bc.funcs.iter_mut() {
        f.entry_ip = label_map[f.entry_ip as usize];
    }

    bc.instructions = out;
    bc.const_pool = pool;
}

// ── Pass 1: bogus control flow ──────────────────────────────────────────────

/// Junk body for a decoy block: `RedirSave RedirRestore` pairs — push a
/// snapshot of the redirect state and immediately pop it back. Neutral for
/// the eval stack, `$?` and the redirect state, but static analysis sees
/// real opcodes with operand-independent side effects.
fn decoy_junk(rng: &mut Rng, count: usize) -> Vec<Instruction> {
    let _ = rng; // reserved for future junk variation
    let mut junk = Vec::with_capacity(count * 2);
    for _ in 0..count {
        junk.push(Instruction::no_operand(Opcode::RedirSave));
        junk.push(Instruction::no_operand(Opcode::RedirRestore));
    }
    junk
}

fn bogus_control_flow(bc: &mut Bytecode, seed: u32) {
    let mut rng = Rng::new(seed ^ 0xB09_05C7);
    let instrs = std::mem::take(&mut bc.instructions);
    let pool: ConstPool = std::mem::take(&mut bc.const_pool);

    // Pick the unconditional Jmps that get a decoy. Never the terminal
    // `Jmp` at the very last position — some tooling assumes the stream
    // ends in Exit.
    let mut chosen = vec![false; instrs.len()];
    let mut any = false;
    for (i, ins) in instrs.iter().enumerate() {
        if ins.op == Opcode::Jmp && i + 1 < instrs.len() && rng.chance(1, 2) {
            chosen[i] = true;
            any = true;
        }
    }
    if !any {
        bc.instructions = instrs;
        bc.const_pool = pool;
        return;
    }

    // Insert each decoy DIRECTLY AFTER its Jmp, never at the end of the
    // program: pipeline-subshell bodies and func calls run bounded by
    // `ip_fence`, and ANY `ip >= fence` breaks out of the region — a
    // decoy parked past the fence silently killed the enclosing loop
    // after one iteration. Same-region detours stay under the fence.
    // Insertion shifts every later index, so a label_map (same scheme
    // as substitute_consts) remaps jump operands, PipeSubshellBegin
    // fence ends and func entries afterwards.
    let mut out: Vec<Instruction> = Vec::with_capacity(instrs.len() + instrs.len() / 2);
    let mut label_map = vec![0u32; instrs.len()];
    let mut retargets: Vec<(usize, u32)> = Vec::new(); // (new pos of chosen Jmp, decoy entry)

    for (old_i, ins) in instrs.into_iter().enumerate() {
        label_map[old_i] = out.len() as u32;
        if !chosen[old_i] {
            out.push(ins);
            continue;
        }
        let target = ins.operand; // old index — remapped below via label_map
        let jmp_pos = out.len();
        out.push(ins);
        let decoy_entry = out.len() as u32;
        if std::env::var_os("SHELLSC_OBF_DEBUG").is_some() {
            eprintln!("bcf: Jmp@{old_i} target={target} -> decoy@{decoy_entry}");
        }
        // Decoy block: junk_len save/restore pairs + 1 final Jmp back.
        let junk_len = 2 + (rng.next() % 3) as usize; // 2-4 pairs
        let mut block = decoy_junk(&mut rng, junk_len);
        block.push(Instruction::new(Opcode::Jmp, target));
        out.extend(block);
        retargets.push((jmp_pos, decoy_entry));
    }

    // Remap every label operand from old- to new-space, then point the
    // chosen Jmps at their (already new-space) decoy entries.
    for ins in out.iter_mut() {
        if matches!(
            ins.op,
            Opcode::Jmp | Opcode::JmpIfFail | Opcode::JmpIfOk | Opcode::PipeSubshellBegin
        ) {
            ins.operand = label_map[ins.operand as usize];
        }
    }
    for &(pos, decoy_entry) in &retargets {
        out[pos].operand = decoy_entry;
    }
    for f in bc.funcs.iter_mut() {
        f.entry_ip = label_map[f.entry_ip as usize];
    }

    bc.instructions = out;
    bc.const_pool = pool;
}

// ── Pass 3: conditional double negation ──────────────────────────────────────

fn cond_double_neg(bc: &mut Bytecode, seed: u32) {
    let mut rng = Rng::new(seed ^ 0xC0FD_6E97);
    let instrs = std::mem::take(&mut bc.instructions);
    let pool: ConstPool = std::mem::take(&mut bc.const_pool);

    // Pick conditionals to invert. A JmpIfFail directly after ForBind is
    // the for-loop exit: its take() of for_exit_status restores the
    // pre-exhaust $? at the loop exit, which the inverted form loses.
    let mut chosen = vec![false; instrs.len()];
    let mut any = false;
    for (i, ins) in instrs.iter().enumerate() {
        if !matches!(ins.op, Opcode::JmpIfFail | Opcode::JmpIfOk) {
            continue;
        }
        if ins.op == Opcode::JmpIfFail && i > 0 && instrs[i - 1].op == Opcode::ForBind {
            continue;
        }
        if rng.chance(4, 10) {
            chosen[i] = true;
            any = true;
        }
    }
    if !any {
        bc.instructions = instrs;
        bc.const_pool = pool;
        return;
    }

    // Inverted form (for `JmpIfCond T` with opposite condition `Opp`):
    //   Opp SKIP     ; taken on the original fall-through path
    //   Jmp T        ; taken on the original jump path
    //   SKIP:        ; next original instruction
    // Status is untouched, so $? at T and at SKIP equals the original.
    // The inverted condition's operand is a new-space local label — it is
    // filled in after the old-space remap (same retarget scheme as bcf).
    let mut out: Vec<Instruction> = Vec::with_capacity(instrs.len() * 2);
    let mut label_map = vec![0u32; instrs.len()];
    let mut retargets: Vec<(usize, u32)> = Vec::new();

    for (old_i, ins) in instrs.into_iter().enumerate() {
        label_map[old_i] = out.len() as u32;
        if !chosen[old_i] {
            out.push(ins);
            continue;
        }
        let target = ins.operand; // old index — remapped below via label_map
        let inv_op = if ins.op == Opcode::JmpIfFail {
            Opcode::JmpIfOk
        } else {
            Opcode::JmpIfFail
        };
        let cond_pos = out.len();
        out.push(Instruction::new(inv_op, 0));
        out.push(Instruction::new(Opcode::Jmp, target));
        retargets.push((cond_pos, out.len() as u32));
    }

    for ins in out.iter_mut() {
        if matches!(
            ins.op,
            Opcode::Jmp | Opcode::JmpIfFail | Opcode::JmpIfOk | Opcode::PipeSubshellBegin
        ) {
            ins.operand = label_map[ins.operand as usize];
        }
    }
    for &(pos, skip) in &retargets {
        out[pos].operand = skip;
    }
    for f in bc.funcs.iter_mut() {
        f.entry_ip = label_map[f.entry_ip as usize];
    }

    bc.instructions = out;
    bc.const_pool = pool;
}

// ── Pass 4: arithmetic neutral append ────────────────────────────────────────

fn arith_neutral(bc: &mut Bytecode, seed: u32) {
    let mut rng = Rng::new(seed ^ 0xA417_0000);
    let instrs = std::mem::take(&mut bc.instructions);
    let mut pool: ConstPool = std::mem::take(&mut bc.const_pool);

    let mut label_map = vec![0u32; instrs.len()];
    let mut out: Vec<Instruction> = Vec::with_capacity(instrs.len() + 8);
    let mut plus0: Option<u32> = None;

    for (old_i, ins) in instrs.into_iter().enumerate() {
        label_map[old_i] = out.len() as u32;
        if ins.op == Opcode::ArithEvalStack && rng.chance(5, 10) {
            let p = *plus0.get_or_insert_with(|| pool.intern("+0"));
            out.push(Instruction::new(Opcode::PushConst, p));
            out.push(Instruction::new(Opcode::ConcatN, 2));
            // Re-anchor the label past the insertion: a jump aimed at this
            // ArithEvalStack arrives exactly as in the original stream
            // (stack untouched), skipping the inserted "+0" pair.
            label_map[old_i] = out.len() as u32;
            out.push(ins);
            continue;
        }
        out.push(ins);
    }

    for ins in out.iter_mut() {
        if matches!(
            ins.op,
            Opcode::Jmp | Opcode::JmpIfFail | Opcode::JmpIfOk | Opcode::PipeSubshellBegin
        ) {
            ins.operand = label_map[ins.operand as usize];
        }
    }
    for f in bc.funcs.iter_mut() {
        f.entry_ip = label_map[f.entry_ip as usize];
    }

    bc.instructions = out;
    bc.const_pool = pool;
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Bytecode {
        let mut bc = Bytecode::default();
        bc.const_pool.strings.push("echo".to_string()); // short — never split
        bc.const_pool.strings.push("hello world".to_string()); // long — split candidate
        bc.const_pool.strings.push("hi".to_string()); // short — never split
        bc.const_pool.strings.push("abc".to_string()); // long — split candidate
        bc.instructions.push(Instruction::new(Opcode::PushConst, 0));
        bc.instructions.push(Instruction::new(Opcode::PushConst, 1));
        bc.instructions.push(Instruction::new(Opcode::PushConst, 2));
        bc.instructions.push(Instruction::new(Opcode::PushConst, 3));
        bc.instructions.push(Instruction::no_operand(Opcode::Exit));
        bc
    }

    fn sample_with_jmp() -> Bytecode {
        let mut bc = Bytecode::default();
        bc.const_pool.strings.push("echo".to_string());
        bc.const_pool.strings.push("hello".to_string());
        bc.const_pool.strings.push("world".to_string());
        bc.instructions.push(Instruction::new(Opcode::PushConst, 0));
        bc.instructions.push(Instruction::new(Opcode::Jmp, 3));
        bc.instructions.push(Instruction::new(Opcode::PushConst, 1));
        bc.instructions.push(Instruction::new(Opcode::PushConst, 2)); // target of the Jmp
        bc.instructions.push(Instruction::no_operand(Opcode::Exit));
        bc
    }

    fn sample_fenced() -> Bytecode {
        // Mimics a `... | while read; do ... done` body: PipeSubshellBegin
        // fences [1..7), a Jmp inside the body back/forward, code after
        // the fence must never run in this single-stream replay.
        let mut bc = Bytecode::default();
        bc.const_pool.strings.push("a".to_string());
        bc.const_pool.strings.push("skip".to_string());
        bc.const_pool.strings.push("b".to_string());
        bc.const_pool.strings.push("outside".to_string());
        bc.instructions
            .push(Instruction::new(Opcode::PipeSubshellBegin, 7)); // 0: fence = 7
        bc.instructions.push(Instruction::new(Opcode::PushConst, 0)); // 1: a
        bc.instructions.push(Instruction::new(Opcode::Jmp, 5)); // 2 -> 5 (body back edge)
        bc.instructions.push(Instruction::new(Opcode::PushConst, 1)); // 3: dead
        bc.instructions
            .push(Instruction::no_operand(Opcode::RedirRestore)); // 4: filler
        bc.instructions.push(Instruction::new(Opcode::PushConst, 2)); // 5: b
        bc.instructions
            .push(Instruction::no_operand(Opcode::RedirSave)); // 6: last body slot
        bc.instructions.push(Instruction::new(Opcode::PushConst, 3)); // 7: outside
        bc.instructions.push(Instruction::no_operand(Opcode::Exit)); // 8
        bc
    }

    fn replay(bc: &Bytecode) -> Vec<String> {
        replay_with(bc, false)
    }

    fn replay_with(bc: &Bytecode, failed: bool) -> Vec<String> {
        // Simulate the VM stack protocol for PushConst/ConcatN/Jmp and the
        // neutral RedirSave/RedirRestore decoy ops: walk until a non-handled
        // opcode; Jmp follows its operand. PipeSubshellBegin arms an
        // ip_fence — crossing it ends the walk, mirroring vm.rs run().
        // `failed` is the static $? seen by conditional jumps (the harness
        // does not model status changes mid-walk).
        let mut stack: Vec<String> = vec![];
        let mut ip = 0usize;
        let mut steps = 0;
        let mut fence: Option<usize> = None;
        while ip < bc.instructions.len() && steps < 100 {
            if let Some(f) = fence {
                if ip >= f {
                    break;
                }
            }
            steps += 1;
            let ins = &bc.instructions[ip];
            match ins.op {
                Opcode::PushConst => {
                    stack.push(bc.const_pool.get(ins.operand).unwrap().to_string());
                    ip += 1;
                }
                Opcode::ConcatN => {
                    let n = ins.operand as usize;
                    let vals: Vec<String> = stack.drain(stack.len() - n..).collect();
                    stack.push(vals.join(""));
                    ip += 1;
                }
                Opcode::Jmp => ip = ins.operand as usize,
                Opcode::JmpIfFail => {
                    if failed {
                        ip = ins.operand as usize;
                    } else {
                        ip += 1;
                    }
                }
                Opcode::JmpIfOk => {
                    if !failed {
                        ip = ins.operand as usize;
                    } else {
                        ip += 1;
                    }
                }
                Opcode::ArithEvalStack => {
                    // ArithParser value semantics: evaluating "expr+0"
                    // yields the same i64 as "expr" (mirror in harness).
                    let e = stack.pop().unwrap();
                    stack.push(e.strip_suffix("+0").map(str::to_string).unwrap_or(e));
                    ip += 1;
                }
                Opcode::ForBind => ip += 1,
                Opcode::RedirSave | Opcode::RedirRestore => ip += 1,
                Opcode::PipeSubshellBegin => {
                    fence = Some(ins.operand as usize);
                    ip += 1;
                }
                Opcode::Exit => break,
                _ => break,
            }
        }
        stack
    }

    #[test]
    fn subst_preserves_concat_result() {
        let mut bc = sample(); // no mid-stream Jmp — replay walks the whole stream
        obfuscate(
            &mut bc,
            &ObfuscateOptions {
                bogus_cf: false,
                subst: true,
                condneg: false,
                arith_neutral: false,
                seed: 7,
            },
        );
        let stack = replay(&bc);
        assert_eq!(stack[0], "echo");
        assert_eq!(stack[1], "hello world");
        assert_eq!(stack[2], "hi");
        assert_eq!(stack[3], "abc");
    }

    #[test]
    fn subst_seed_varies_result() {
        let build = |seed| {
            let mut bc = sample();
            obfuscate(
                &mut bc,
                &ObfuscateOptions {
                    bogus_cf: false,
                    subst: true,
                    condneg: false,
                    arith_neutral: false,
                    seed,
                },
            );
            format!("{:?}", bc.instructions)
        };
        assert_ne!(build(1), build(2));
    }

    #[test]
    fn bogus_cf_preserves_replay_semantics() {
        let orig = sample_with_jmp();
        let want = replay(&orig);

        for seed in [0u32, 1, 7, 0xFFFF_FFFF] {
            let mut bc = sample_with_jmp();
            obfuscate(
                &mut bc,
                &ObfuscateOptions {
                    bogus_cf: true,
                    subst: false,
                    condneg: false,
                    arith_neutral: false,
                    seed,
                },
            );
            let got = replay(&bc);
            assert_eq!(got, want, "seed {seed} changed semantics");
        }
    }

    #[test]
    fn bogus_cf_jump_targets_in_range() {
        let mut bc = sample_with_jmp();
        obfuscate(
            &mut bc,
            &ObfuscateOptions {
                bogus_cf: true,
                subst: false,
                condneg: false,
                arith_neutral: false,
                seed: 3,
            },
        );
        let len = bc.instructions.len() as u32;
        for ins in &bc.instructions {
            if matches!(ins.op, Opcode::Jmp | Opcode::JmpIfFail | Opcode::JmpIfOk) {
                assert!(ins.operand < len, "target {} out of range", ins.operand);
            }
        }
        // Exit still exists — decoy blocks are inserted before it and
        // runtime must reach it when the program ends.
        assert!(
            bc.instructions.iter().any(|ins| ins.op == Opcode::Exit),
            "Exit lost after bcf"
        );
    }

    #[test]
    fn bogus_cf_decoy_stays_inside_ip_fence() {
        // Regression: appending decoys at the end of the program put them
        // past any enclosing ip_fence (pipeline-subshell body), so the VM
        // broke out of the region on the very first detour and the loop
        // after it ran once. Decoys must stay within the fenced body and
        // the fence end must shift with the insertion.
        let want = replay(&sample_fenced());
        assert_eq!(want, vec!["a", "b"], "fenced sample baseline wrong");

        for seed in [0u32, 1, 7, 0xFFFF_FFFF] {
            let mut bc = sample_fenced();
            obfuscate(
                &mut bc,
                &ObfuscateOptions {
                    bogus_cf: true,
                    subst: false,
                    condneg: false,
                    arith_neutral: false,
                    seed,
                },
            );
            assert_eq!(replay(&bc), want, "seed {seed} broke fenced body");
        }
    }

    #[test]
    fn bogus_cf_seed_varies_layout() {
        let build = |seed| {
            let mut bc = sample_with_jmp();
            obfuscate(
                &mut bc,
                &ObfuscateOptions {
                    bogus_cf: true,
                    subst: false,
                    condneg: false,
                    arith_neutral: false,
                    seed,
                },
            );
            format!("{:?}", bc.instructions)
        };
        // At least one pair of seeds should produce different layouts
        // (jump chosen vs not, or junk length differs).
        let a = build(1);
        let b = build(2);
        let c = build(3);
        assert!(a != b || a != c || b != c, "layouts identical across seeds");
    }

    // ── condneg ──────────────────────────────────────────────────────────────

    fn sample_cond(op: Opcode) -> Bytecode {
        // 0: head; 1: op -> 4; 2: fall; 3: Jmp 5; 4: taken; 5: Exit
        let mut bc = Bytecode::default();
        bc.const_pool.strings.push("head".to_string());
        bc.const_pool.strings.push("fall".to_string());
        bc.const_pool.strings.push("taken".to_string());
        bc.instructions.push(Instruction::new(Opcode::PushConst, 0));
        bc.instructions.push(Instruction::new(op, 4));
        bc.instructions.push(Instruction::new(Opcode::PushConst, 1));
        bc.instructions.push(Instruction::new(Opcode::Jmp, 5));
        bc.instructions.push(Instruction::new(Opcode::PushConst, 2));
        bc.instructions.push(Instruction::no_operand(Opcode::Exit));
        bc
    }

    #[test]
    fn condneg_preserves_replay_semantics() {
        for op in [Opcode::JmpIfFail, Opcode::JmpIfOk] {
            let orig = sample_cond(op.clone());
            for failed in [false, true] {
                let want = replay_with(&orig, failed);
                for seed in [0u32, 1, 7, 42, 0xFFFF_FFFF] {
                    let mut bc = sample_cond(op.clone());
                    obfuscate(
                        &mut bc,
                        &ObfuscateOptions {
                            bogus_cf: false,
                            subst: false,
                            condneg: true,
                            arith_neutral: false,
                            seed,
                        },
                    );
                    assert_eq!(
                        replay_with(&bc, failed),
                        want,
                        "op {op:?} failed={failed} seed {seed}"
                    );
                }
            }
        }
    }

    #[test]
    fn condneg_targets_in_range() {
        for op in [Opcode::JmpIfFail, Opcode::JmpIfOk] {
            let mut bc = sample_cond(op);
            obfuscate(
                &mut bc,
                &ObfuscateOptions {
                    bogus_cf: false,
                    subst: false,
                    condneg: true,
                    arith_neutral: false,
                    seed: 3,
                },
            );
            let len = bc.instructions.len() as u32;
            for ins in &bc.instructions {
                if matches!(ins.op, Opcode::Jmp | Opcode::JmpIfFail | Opcode::JmpIfOk) {
                    assert!(ins.operand < len, "target {} out of range", ins.operand);
                }
            }
            assert!(bc.instructions.iter().any(|i| i.op == Opcode::Exit));
        }
    }

    #[test]
    fn condneg_varies_layout_across_seeds() {
        let orig_len = sample_cond(Opcode::JmpIfFail).instructions.len();
        let grew = (0u32..16).any(|seed| {
            let mut bc = sample_cond(Opcode::JmpIfFail);
            obfuscate(
                &mut bc,
                &ObfuscateOptions {
                    bogus_cf: false,
                    subst: false,
                    condneg: true,
                    arith_neutral: false,
                    seed,
                },
            );
            bc.instructions.len() == orig_len + 1 // 1 cond replaced by 2
        });
        assert!(grew, "condneg never fired across 16 seeds");
    }

    #[test]
    fn condneg_skips_forbind_loop_exit() {
        // ForBind immediately followed by JmpIfFail is the for-loop exit —
        // its for_exit_status.take() side effect must survive, so the
        // transform must leave that jump alone.
        let mut bc = Bytecode::default();
        bc.const_pool.strings.push("i".to_string());
        bc.const_pool.strings.push("body".to_string());
        bc.instructions.push(Instruction::new(Opcode::ForBind, 0)); // 0
        bc.instructions.push(Instruction::new(Opcode::JmpIfFail, 4)); // 1: loop exit
        bc.instructions.push(Instruction::new(Opcode::PushConst, 1)); // 2
        bc.instructions.push(Instruction::new(Opcode::Jmp, 1)); // 3: back edge
        bc.instructions.push(Instruction::no_operand(Opcode::Exit)); // 4
        let orig_len = bc.instructions.len();

        for seed in 0u32..16 {
            let mut bc = bc.clone();
            obfuscate(
                &mut bc,
                &ObfuscateOptions {
                    bogus_cf: false,
                    subst: false,
                    condneg: true,
                    arith_neutral: false,
                    seed,
                },
            );
            assert_eq!(bc.instructions.len(), orig_len, "seed {seed}");
            assert_eq!(bc.instructions[1].op, Opcode::JmpIfFail, "seed {seed}");
        }
    }

    // ── arith_neutral ────────────────────────────────────────────────────────

    fn sample_arith() -> Bytecode {
        let mut bc = Bytecode::default();
        bc.const_pool.strings.push("7".to_string());
        bc.instructions.push(Instruction::new(Opcode::PushConst, 0));
        bc.instructions
            .push(Instruction::new(Opcode::ArithEvalStack, 0));
        bc.instructions.push(Instruction::no_operand(Opcode::Exit));
        bc
    }

    #[test]
    fn arith_neutral_preserves_result() {
        let want = replay(&sample_arith());
        assert_eq!(want, vec!["7"], "arith baseline wrong");

        let mut grew = false;
        for seed in 0u32..16 {
            let mut bc = sample_arith();
            obfuscate(
                &mut bc,
                &ObfuscateOptions {
                    bogus_cf: false,
                    subst: false,
                    condneg: false,
                    arith_neutral: true,
                    seed,
                },
            );
            if bc.instructions.len() > 3 {
                grew = true;
            }
            assert_eq!(replay(&bc), want, "seed {seed} changed arith result");
        }
        assert!(grew, "arith_neutral never fired across test seeds");
    }

    #[test]
    fn arith_neutral_appends_zero_expr() {
        // Find a seed where the pass fires and check the stack shape:
        // expr pushed, "+0" pushed, ConcatN joins to "7+0".
        for seed in 0u32..32 {
            let mut bc = sample_arith();
            obfuscate(
                &mut bc,
                &ObfuscateOptions {
                    bogus_cf: false,
                    subst: false,
                    condneg: false,
                    arith_neutral: true,
                    seed,
                },
            );
            if bc.instructions.len() > 3 {
                let has_plus0 = bc.const_pool.strings.iter().any(|s| s == "+0");
                assert!(has_plus0, "seed {seed}: +0 missing from pool");
                assert_eq!(
                    bc.instructions[0].op,
                    Opcode::PushConst,
                    "seed {seed}: insert must precede ArithEvalStack"
                );
                return;
            }
        }
        panic!("arith_neutral never fired across 32 seeds");
    }
}
