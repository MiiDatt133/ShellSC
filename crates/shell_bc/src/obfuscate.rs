//! LLVM-style obfuscation passes over the assembled bytecode (O-LLVM
//! `bcf` + `sub` principles, applied at the bytecode tier instead of
//! machine code):
//!
//! 1. **Bogus control flow** — for a seed-chosen subset of unconditional
//!    `Jmp`s, retarget the jump to a decoy block appended at the end of
//!    the program. The decoy runs a few stack-neutral junk instructions
//!    and jumps to the original target. A static CFG analysis sees extra
//!    blocks and edges indistinguishable from real ones.
//!
//! 2. **Instruction substitution** — `PushConst(s)` is split into
//!    `PushConst(a) PushConst(b) ConcatN(2)` at a seed-chosen cut point.
//!    The VM's `pop_n` returns slots oldest-first and `ConcatN` joins in
//!    that order, so the concatenation is `a + b == s` — semantics
//!    preserved, but the pool no longer holds the full literal in one
//!    piece for a static string-dump to read.
//!
//! Both passes are seed-driven: two protected builds of the same script
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

    // Choose which unconditional Jmps get a decoy, then build decoy
    // blocks at the end. Appending keeps every existing label stable, so
    // func entries, redir operands and untouched jumps need no relabeling.
    let chosen: Vec<usize> = instrs
        .iter()
        .enumerate()
        .filter(|(i, ins)| {
            ins.op == Opcode::Jmp && {
                // Never decoy the terminal `Jmp` at the very last position —
                // some tooling assumes the stream ends in Exit.
                *i + 1 < instrs.len()
            }
        })
        .filter(|_| rng.chance(1, 2))
        .map(|(i, _)| i)
        .collect();

    if chosen.is_empty() {
        bc.instructions = instrs;
        bc.const_pool = pool;
        return;
    }

    let base = instrs.len() as u32;
    let mut body: Vec<Instruction> = instrs;

    // Each decoy block: junk_len save/restore pairs + 1 final Jmp.
    let junk_len = 2 + (rng.next() % 3) as usize; // 2-4 pairs
    let block_len = (junk_len * 2 + 1) as u32;
    let mut entry = base;
    let mut patch: Vec<(usize, u32, u32)> = Vec::new(); // (jmp_pos, decoy_entry, original_target)
    for &jmp_pos in &chosen {
        let original_target = body[jmp_pos].operand;
        patch.push((jmp_pos, entry, original_target));
        entry += block_len;
    }

    // Retarget the chosen jumps to their decoy entries.
    for &(pos, decoy_entry, _) in &patch {
        body[pos].operand = decoy_entry;
    }

    // Append the decoy blocks.
    for &(_, decoy_entry, target) in &patch {
        let mut block = decoy_junk(&mut rng, junk_len);
        block.push(Instruction::new(Opcode::Jmp, target));
        let expected = decoy_entry as usize;
        debug_assert_eq!(body.len(), expected, "decoy layout mismatch");
        body.extend(block);
    }

    bc.instructions = body;
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

    fn replay(bc: &Bytecode) -> Vec<String> {
        // Simulate the VM stack protocol for PushConst/ConcatN/Jmp and the
        // neutral RedirSave/RedirRestore decoy ops: walk until a non-handled
        // opcode; Jmp follows its operand.
        let mut stack: Vec<String> = vec![];
        let mut ip = 0usize;
        let mut steps = 0;
        while ip < bc.instructions.len() && steps < 100 {
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
                Opcode::RedirSave | Opcode::RedirRestore => ip += 1,
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
        let exit_pos = bc.instructions.len() - 1;
        obfuscate(
            &mut bc,
            &ObfuscateOptions {
                bogus_cf: true,
                subst: false,
                seed: 3,
            },
        );
        let len = bc.instructions.len() as u32;
        for ins in &bc.instructions {
            if matches!(ins.op, Opcode::Jmp | Opcode::JmpIfFail | Opcode::JmpIfOk) {
                assert!(ins.operand < len, "target {} out of range", ins.operand);
            }
        }
        // Exit still exists at its original index — decoy blocks are
        // appended after it and runtime never falls past Exit.
        assert_eq!(bc.instructions[exit_pos].op, Opcode::Exit);
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
}
