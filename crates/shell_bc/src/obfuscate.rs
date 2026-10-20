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
//! 5. **Mixed boolean-arithmetic (MBA) substitution** — the instruction
//!    range that builds an arithmetic expression string is duplicated three
//!    times and recombined as `(E)|((E)^(E))` (seed-chosen identity from
//!    four shapes). Every copy is parenthesised, so ArithParser's precedence
//!    never leaks into the identity; a static reader sees boolean/algebra
//!    noise instead of the operand structure. The range is re-executed, so
//!    it must be pure (PushConst/PushVar/ConcatN only) and free of label
//!    entries other than at its root.
//!
//! 6. **Region control-flow flattening** — maximal straight-line regions
//!    are cut into seed-chosen blocks driven by a state-machine dispatcher:
//!    a hidden shell variable holds the block id, and each dispatch entry
//!    is `PushVar state; PushConst id; BashBinary -eq; JmpIfOk block`.
//!    Block order in the stream is scrambled; execution order is restored
//!    by `SetVar state` transitions. Regions are restricted so the
//!    dispatcher's status-comparing checks cannot be observed: no `$?`
//!    reads, no status-flow opcodes, the region's last status setter must
//!    sit in the last block, and calls to functions that can read the
//!    inherited status are kept outside.
//!
//! All passes are seed-driven: two protected builds of the same script
//! get different cut points and decoy placements.

use crate::{
    bytecode::{Bytecode, FuncEntry, Instruction},
    const_pool::ConstPool,
    opcode::Opcode,
};
use shell_ir::BuiltinId;

#[derive(Debug, Clone, Copy)]
pub struct ObfuscateOptions {
    pub bogus_cf: bool,
    pub subst: bool,
    pub condneg: bool,
    pub arith_neutral: bool,
    pub mba: bool,
    pub region_cff: bool,
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
    if opts.mba {
        mba_substitute(bc, opts.seed);
    }
    if opts.region_cff {
        region_cff(bc, opts.seed);
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
            Opcode::Jmp
                | Opcode::JmpIfFail
                | Opcode::JmpIfOk
                | Opcode::PipeSubshellBegin
                | Opcode::SubshellBgEnd
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
            Opcode::Jmp
                | Opcode::JmpIfFail
                | Opcode::JmpIfOk
                | Opcode::PipeSubshellBegin
                | Opcode::SubshellBgEnd
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
            Opcode::Jmp
                | Opcode::JmpIfFail
                | Opcode::JmpIfOk
                | Opcode::PipeSubshellBegin
                | Opcode::SubshellBgEnd
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
            Opcode::Jmp
                | Opcode::JmpIfFail
                | Opcode::JmpIfOk
                | Opcode::PipeSubshellBegin
                | Opcode::SubshellBgEnd
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

// ── Shared helpers for passes 5–6 ───────────────────────────────────────────

/// True for instructions addressed by something else: jump targets, fence
/// and body ends, function entries. A run containing one of these cannot
/// be rearranged as a unit — control can land mid-run.
fn label_targets(instrs: &[Instruction], funcs: &[FuncEntry]) -> Vec<bool> {
    let mut t = vec![false; instrs.len()];
    for ins in instrs {
        if matches!(
            ins.op,
            Opcode::Jmp
                | Opcode::JmpIfFail
                | Opcode::JmpIfOk
                | Opcode::PipeSubshellBegin
                | Opcode::SubshellBgEnd
        ) && (ins.operand as usize) < t.len()
        {
            t[ins.operand as usize] = true;
        }
    }
    for f in funcs {
        if (f.entry_ip as usize) < t.len() {
            t[f.entry_ip as usize] = true;
        }
    }
    t
}

fn remap_labels(out: &mut [Instruction], label_map: &[u32]) {
    for ins in out.iter_mut() {
        if matches!(
            ins.op,
            Opcode::Jmp
                | Opcode::JmpIfFail
                | Opcode::JmpIfOk
                | Opcode::PipeSubshellBegin
                | Opcode::SubshellBgEnd
        ) {
            ins.operand = label_map[ins.operand as usize];
        }
    }
}

// ── Pass 5: mixed boolean-arithmetic substitution ────────────────────────────

/// The pure range `[start, i)` feeding the `ArithEvalStack` at `i`: only
/// PushConst/PushVar/ConcatN, exactly one net stack value out, and no reads
/// of values pushed before `start`. `need` counts the pushes still owed to
/// the suffix while walking backwards; hitting zero on a push marks the root.
fn arith_build_range(instrs: &[Instruction], i: usize) -> Option<usize> {
    let mut need = 1i32;
    for j in (0..i).rev() {
        match instrs[j].op {
            Opcode::PushConst | Opcode::PushVar => {
                need -= 1;
                if need == 0 {
                    return Some(j);
                }
            }
            Opcode::ConcatN => {
                let n = instrs[j].operand as i32;
                if n == 0 {
                    return None;
                }
                need = need - 1 + n;
            }
            _ => return None,
        }
    }
    None
}

/// Rewrite `ArithEvalStack(build_range)` so the evaluated text becomes an
/// MBA identity wrapping three copies of the range, e.g.
/// `(E)|((E)^(E))` = E. Each copy sits in its own parens, so precedence
/// inside E can never reach the outer identity. ArithParser never fails
/// outright (unparsed tail is ignored, empty primary → 0), so even a
/// degenerate E collapses to its original value.
fn mba_substitute(bc: &mut Bytecode, seed: u32) {
    // Four identities, split as [literal before each of the three copies,
    // final suffix]: L1 E L2 E L3 E L4 joined by ConcatN(7).
    const IDENTS: [[&str; 4]; 4] = [
        ["(", ")|((", ")^(", "))"], // (E)|((E)^(E))  = E|(E^E) = E
        ["(", ")+(", "-(", "))"],   // (E)+(E-(E))    = E+(E-E) = E
        ["(", ")&((", ")|(", "))"], // (E)&((E)|(E))  = E&(E|E) = E
        ["(", ")^(", "^(", "))"],   // (E)^(E^(E))    = E^(E^E) = E
    ];
    let mut rng = Rng::new(seed ^ 0x6BA0_1E17);
    let instrs = std::mem::take(&mut bc.instructions);
    let mut pool: ConstPool = std::mem::take(&mut bc.const_pool);
    let targets = label_targets(&instrs, &bc.funcs);

    // Plan which sites fire. Ranges are disjoint: a pure range cannot
    // contain another ArithEvalStack, so one site's scan-back aborts on
    // another site's eval.
    let mut plan: Vec<Option<Vec<Instruction>>> = vec![None; instrs.len()];
    let mut skip = vec![false; instrs.len()];
    for i in 0..instrs.len() {
        if instrs[i].op != Opcode::ArithEvalStack || !rng.chance(5, 10) {
            continue;
        }
        let Some(s) = arith_build_range(&instrs, i) else {
            continue;
        };
        // A jump into the middle of the range would skip the leading "(" and
        // misalign ConcatN(7). A jump AT the root is fine (re-anchored to the
        // literal); a jump at the eval skips the insertion entirely, exactly
        // as in arith_neutral.
        if targets[s + 1..i].iter().any(|&t| t) {
            continue;
        }
        plan[i] = Some(instrs[s..i].to_vec());
        skip[s..i].fill(true);
    }

    let mut label_map = vec![0u32; instrs.len()];
    let mut out: Vec<Instruction> = Vec::with_capacity(instrs.len() + 32);

    for old_i in 0..instrs.len() {
        if skip[old_i] {
            continue;
        }
        let ins = instrs[old_i].clone();
        label_map[old_i] = out.len() as u32;
        let Some(range) = plan[old_i].take() else {
            out.push(ins);
            continue;
        };
        let s = old_i - range.len();
        let lits = IDENTS[(rng.next() % 4) as usize];
        let p: [u32; 4] = [
            pool.intern(lits[0]),
            pool.intern(lits[1]),
            pool.intern(lits[2]),
            pool.intern(lits[3]),
        ];
        if std::env::var_os("SHELLSC_OBF_DEBUG").is_some() {
            eprintln!("mba: eval@{old_i} range[{s}..{old_i}) ident={}", lits[1]);
        }
        // Fall-through and jumps at the range root both must run the first
        // literal, so the root label points at it — not at the copy.
        label_map[s] = out.len() as u32;
        out.push(Instruction::new(Opcode::PushConst, p[0]));
        for (k, r) in range.iter().enumerate() {
            if k > 0 {
                label_map[s + k] = out.len() as u32;
            }
            out.push(r.clone());
        }
        // L1 E L2 E L3 E L4 — the closing suffix carries no payload copy.
        for lit in [p[1], p[2]] {
            out.push(Instruction::new(Opcode::PushConst, lit));
            for r in &range {
                out.push(r.clone());
            }
        }
        out.push(Instruction::new(Opcode::PushConst, p[3]));
        out.push(Instruction::new(Opcode::ConcatN, 7));
        // Re-anchor: a jump aimed at this ArithEvalStack skips the whole
        // insertion and evaluates whatever it brought, as before.
        label_map[old_i] = out.len() as u32;
        out.push(ins);
    }

    remap_labels(&mut out, &label_map);
    for f in bc.funcs.iter_mut() {
        f.entry_ip = label_map[f.entry_ip as usize];
    }
    bc.instructions = out;
    bc.const_pool = pool;
}

// ── Pass 6: region control-flow flattening ───────────────────────────────────

/// Hidden dispatcher state. Contains a control character, so no lexer-produced
/// variable name can collide with it.
const CFF_STATE_VAR: &str = "\u{1}sc";

/// Straight-line, side-effect-local opcodes a region may be built from.
/// Control flow, region fences, redirections (their ambiguous-redirect flag
/// has a consume window that a transition SetVar would clear) and shell
/// bracket machinery stay outside.
fn cff_eligible_op(op: &Opcode) -> bool {
    matches!(
        op,
        Opcode::PushConst
            | Opcode::PushVar
            | Opcode::ConcatN
            | Opcode::SetVar
            | Opcode::Builtin
            | Opcode::ExecExternal
            | Opcode::ExecExternalBg
            | Opcode::StatusOk
            | Opcode::StatusFail
            | Opcode::ArithEvalStack
            | Opcode::BashUnary
            | Opcode::BashBinary
            | Opcode::GlobExpand
            | Opcode::PushArgs
            | Opcode::ArrayAssign
            | Opcode::ArraySetIndex
            | Opcode::VarExpand
    )
}

fn sets_status(op: &Opcode) -> bool {
    matches!(
        op,
        Opcode::BashUnary
            | Opcode::BashBinary
            | Opcode::StatusOk
            | Opcode::StatusFail
            | Opcode::ExecExternal
            | Opcode::ExecExternalBg
            | Opcode::Builtin
    )
}

/// Variable name read by PushVar / encoded in VarExpand, if any.
fn instr_var_name<'a>(ins: &Instruction, pool: &'a ConstPool) -> Option<&'a str> {
    match ins.op {
        Opcode::PushVar => pool.get(ins.operand),
        Opcode::VarExpand => pool.get(ins.operand & 0x00FF_FFFF),
        _ => None,
    }
}

/// Statically resolve argv[0] of the exec at `i`. Returns `Some` only when
/// every root of the argc-wide argument walk is decidable and argv[0] is a
/// plain constant; `None` means "could be anything" (dynamic command word,
/// glob/`$@` splices, malformed stream).
fn argv0_const(instrs: &[Instruction], i: usize, pool: &ConstPool) -> Option<String> {
    let argc = instrs[i].operand as i32;
    if argc <= 0 {
        return None;
    }
    let mut need = argc;
    for j in (0..i).rev() {
        let ins = &instrs[j];
        match ins.op {
            Opcode::PushConst => {
                need -= 1;
                if need == 0 {
                    return Some(pool.get(ins.operand)?.to_string());
                }
            }
            Opcode::PushVar => {
                need -= 1;
                if need == 0 {
                    return None;
                }
            }
            Opcode::ConcatN => {
                let n = ins.operand as i32;
                if n == 0 {
                    return None;
                }
                need = need - 1 + n;
            }
            Opcode::ArithEvalStack => {}
            _ => return None,
        }
    }
    None
}

/// Per-function flag: the body can observe the status inherited from the
/// call site (reads `$?` before anything sets it, re-enters the shell via
/// eval/return/exit, or calls out to something that can — resolved by
/// fixpoint). Over-approximated on purpose: only soundness matters here.
fn status_observers(instrs: &[Instruction], pool: &ConstPool, funcs: &[FuncEntry]) -> Vec<bool> {
    let mut obs = vec![false; funcs.len()];
    if funcs.is_empty() {
        return obs;
    }
    // Body extent: entry to the first FuncReturn (structured lowering emits
    // early returns as jumps to the trailing FuncReturn). Scanning past a
    // mid-body FuncReturn only over-approximates, which stays sound.
    let body_range = |entry: usize| -> (usize, usize) {
        let end = instrs[entry.min(instrs.len())..]
            .iter()
            .position(|ins| ins.op == Opcode::FuncReturn)
            .map(|p| entry + p + 1)
            .unwrap_or(instrs.len());
        (entry, end)
    };
    loop {
        let mut changed = false;
        for (fi, f) in funcs.iter().enumerate() {
            if obs[fi] {
                continue;
            }
            let (start, end) = body_range(f.entry_ip as usize);
            let end = end.min(instrs.len());
            let mut hit = false;
            for (offset, ins) in instrs[start..end].iter().enumerate() {
                let idx = start + offset;
                match ins.op {
                    Opcode::StatusFlip => hit = true,
                    Opcode::PushVar | Opcode::VarExpand => {
                        if instr_var_name(ins, pool) == Some("?") {
                            hit = true;
                        }
                    }
                    Opcode::Builtin => {
                        if let Some((id, _)) = BuiltinId::decode(ins.operand) {
                            if matches!(id, BuiltinId::Eval | BuiltinId::Return | BuiltinId::Exit) {
                                hit = true;
                            }
                        }
                    }
                    Opcode::ExecExternal | Opcode::ExecExternalBg => {
                        match argv0_const(instrs, idx, pool) {
                            None => hit = true,
                            Some(name) => {
                                if let Some(k) = funcs.iter().position(|g| g.name == name) {
                                    if obs[k] {
                                        hit = true;
                                    }
                                }
                            }
                        }
                    }
                    _ => {}
                }
                if hit {
                    break;
                }
            }
            if hit {
                obs[fi] = true;
                changed = true;
            }
        }
        if !changed {
            return obs;
        }
    }
}

/// Can the call at `i` reach a status-observing function?
fn call_observes(
    instrs: &[Instruction],
    pool: &ConstPool,
    funcs: &[FuncEntry],
    i: usize,
    obs: &[bool],
) -> bool {
    match argv0_const(instrs, i, pool) {
        // Dynamic command word: only safe when no function observes status.
        None => obs.iter().any(|&o| o),
        Some(name) => match funcs.iter().position(|f| f.name == name) {
            Some(k) => obs[k],
            None => false, // external command — children never see `$?`
        },
    }
}

fn region_cff(bc: &mut Bytecode, seed: u32) {
    let mut rng = Rng::new(seed ^ 0x0CFF_1EE5);
    let instrs = std::mem::take(&mut bc.instructions);
    let mut pool: ConstPool = std::mem::take(&mut bc.const_pool);
    let targets = label_targets(&instrs, &bc.funcs);
    let observers = status_observers(&instrs, &pool, &bc.funcs);

    // Per-position eligibility. Bad instructions become boundaries, so the
    // `$?` reader / eval / return / unproven call stays outside every region
    // and never sees the dispatcher's compare results.
    let mut elig = vec![true; instrs.len()];
    for i in 0..instrs.len() {
        let op = instrs[i].op.clone();
        if targets[i] || !cff_eligible_op(&op) {
            elig[i] = false;
            continue;
        }
        if let Some(name) = instr_var_name(&instrs[i], &pool) {
            if name == "?" {
                elig[i] = false;
            }
        }
        if op == Opcode::Builtin {
            match BuiltinId::decode(instrs[i].operand) {
                Some((id, _)) => {
                    if matches!(
                        id,
                        BuiltinId::Eval | BuiltinId::Return | BuiltinId::Exit | BuiltinId::Exec
                    ) {
                        elig[i] = false;
                    }
                }
                None => elig[i] = false,
            }
        }
        if matches!(op, Opcode::ExecExternal | Opcode::ExecExternalBg)
            && call_observes(&instrs, &pool, &bc.funcs, i, &observers)
        {
            elig[i] = false;
        }
    }

    // Maximal eligible runs that are worth flattening and whose exit status
    // can survive the dispatcher:
    //  - at least one status setter, and it must be able to sit in the LAST
    //    block (no dispatch entry runs after the region's final setter), so
    //    it cannot be the region's first instruction;
    //  - a redirect whose ambiguous-redirect flag is still pending must not
    //    be consumed after a transition SetVar cleared it.
    let mut regions: Vec<(usize, usize, usize)> = Vec::new();
    let mut i = 0;
    while i < instrs.len() {
        if !elig[i] {
            i += 1;
            continue;
        }
        let s = i;
        while i < instrs.len() && elig[i] {
            i += 1;
        }
        let e = i;
        if e - s < 4 {
            continue;
        }
        let Some(last_set) = (s..e).rev().find(|&j| sets_status(&instrs[j].op)) else {
            continue;
        };
        if last_set < s + 1 {
            continue;
        }
        if s > 0
            && matches!(
                instrs[s - 1].op,
                Opcode::Redirect | Opcode::DynRedir | Opcode::DynRedirLine
            )
        {
            continue;
        }
        if !rng.chance(6, 10) {
            continue;
        }
        if std::env::var_os("SHELLSC_OBF_DEBUG").is_some() {
            eprintln!("cff: region[{s}..{e}) last_set={last_set}");
        }
        regions.push((s, e, last_set));
    }

    let mut label_map = vec![0u32; instrs.len()];
    let mut out: Vec<Instruction> = Vec::with_capacity(instrs.len() + 64);
    // New-space local labels (dispatcher head, block entries) filled in
    // after the old-space remap, same scheme as bogus_control_flow.
    let mut retargets: Vec<(usize, u32)> = Vec::new();
    let mut r = 0;
    let mut i = 0;
    while i < instrs.len() {
        if r >= regions.len() || regions[r].0 != i {
            label_map[i] = out.len() as u32;
            out.push(instrs[i].clone());
            i += 1;
            continue;
        }
        let (s, e, last_set) = regions[r];
        i = e;
        r += 1;

        let state_idx = pool.intern(CFF_STATE_VAR);
        let eq_idx = pool.intern("-eq");
        // Blocks:2..=max_k, all splits at or before the last status setter.
        let max_k = (last_set - s + 1).min(6).min(e - s);
        if max_k < 2 {
            // Emit the region unflattened (checks above make this
            // unreachable, but never emit a half-built dispatcher).
            for old in s..e {
                label_map[old] = out.len() as u32;
                out.push(instrs[old].clone());
            }
            continue;
        }
        let k = 2 + (rng.next() % (max_k as u64 - 1)) as usize;
        let mut cands: Vec<usize> = (s + 1..=last_set).collect();
        for x in 0..k - 1 {
            let j = x + (rng.next() as usize % (cands.len() - x));
            cands.swap(x, j);
        }
        let mut cuts: Vec<usize> = cands[..k - 1].to_vec();
        cuts.sort_unstable();
        let mut starts: Vec<usize> = Vec::with_capacity(k);
        starts.push(s);
        starts.extend(cuts.iter().copied());
        let ids: Vec<u32> = (0..k).map(|n| pool.intern(&n.to_string())).collect();

        // Entry: state = first block. A jump at the region root lands here
        // and re-initialises, exactly matching original fall-in order.
        label_map[s] = out.len() as u32;
        out.push(Instruction::new(Opcode::PushConst, ids[0]));
        out.push(Instruction::new(Opcode::SetVar, state_idx));
        let dispatch = out.len() as u32;
        // Dispatch entries, seed-shuffled so the compare cascade order is
        // not simply the execution order.
        let mut order: Vec<usize> = (0..k).collect();
        for x in 0..k {
            let j = x + (rng.next() as usize % (k - x));
            order.swap(x, j);
        }
        let mut entry_slots: Vec<(usize, usize)> = Vec::with_capacity(k);
        for &b in &order {
            out.push(Instruction::new(Opcode::PushVar, state_idx));
            out.push(Instruction::new(Opcode::PushConst, ids[b]));
            out.push(Instruction::new(Opcode::BashBinary, eq_idx));
            entry_slots.push((b, out.len()));
            out.push(Instruction::new(Opcode::JmpIfOk, 0));
        }
        if e < instrs.len() {
            // Unreachable safety: state always matches some entry. Falls
            // through to the region end via the old-space label.
            out.push(Instruction::new(Opcode::Jmp, e as u32));
        }
        let mut block_pos = vec![0u32; k];
        for b in 0..k {
            block_pos[b] = out.len() as u32;
            let be = if b + 1 < k { starts[b + 1] } else { e };
            for old in starts[b]..be {
                if old != s {
                    // s keeps its init label: external entries must re-run
                    // the dispatcher, not skip into block 0's body.
                    label_map[old] = out.len() as u32;
                }
                out.push(instrs[old].clone());
            }
            if b + 1 < k {
                out.push(Instruction::new(Opcode::PushConst, ids[b + 1]));
                out.push(Instruction::new(Opcode::SetVar, state_idx));
                retargets.push((out.len(), dispatch));
                out.push(Instruction::new(Opcode::Jmp, 0));
            }
        }
        for (blk, pos) in entry_slots {
            retargets.push((pos, block_pos[blk]));
        }
    }

    remap_labels(&mut out, &label_map);
    for &(pos, target) in &retargets {
        out[pos].operand = target;
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

    /// Index of the `)` closing the `(` at position 0 (depth-counting).
    fn match_paren(s: &str) -> Option<usize> {
        let b = s.as_bytes();
        if b.first() != Some(&b'(') {
            return None;
        }
        let mut depth = 0i32;
        for (i, &c) in b.iter().enumerate() {
            match c {
                b'(' => depth += 1,
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(i);
                    }
                }
                _ => {}
            }
        }
        None
    }

    /// Collapse one MBA identity back to its payload E — the value an
    /// ArithParser evaluation of the identity would produce, expressed as
    /// the payload text (which the harness then evaluates as before).
    fn mba_reduce(s: &str) -> String {
        let Some(close) = match_paren(s) else {
            return s.to_string();
        };
        let e = &s[1..close];
        let rest = &s[close..];
        let forms = [
            format!(")|(({})^({}))", e, e),
            format!(")+({}-({}))", e, e),
            format!(")&(({})|({}))", e, e),
            format!(")^({}^({}))", e, e),
        ];
        if forms.iter().any(|f| rest == f) {
            e.to_string()
        } else {
            s.to_string()
        }
    }

    fn replay_with(bc: &Bytecode, failed: bool) -> Vec<String> {
        // Simulate the VM stack protocol for PushConst/ConcatN/Jmp, plain
        // variables (SetVar/PushVar), runtime $? (StatusOk/Fail, BashBinary),
        // and the neutral RedirSave/RedirRestore decoy ops: walk until a
        // non-handled opcode; Jmp follows its operand. PipeSubshellBegin
        // arms an ip_fence — crossing it ends the walk, mirroring vm.rs
        // run(). `failed` is the initial $?; conditional jumps read the
        // runtime status from there on.
        use std::collections::HashMap;
        let mut stack: Vec<String> = vec![];
        let mut vars: HashMap<String, String> = HashMap::new();
        let mut status_failed = failed;
        let mut ip = 0usize;
        let mut steps = 0;
        let mut fence: Option<usize> = None;
        while ip < bc.instructions.len() && steps < 500 {
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
                Opcode::PushVar => {
                    let name = bc.const_pool.get(ins.operand).unwrap();
                    stack.push(vars.get(name).cloned().unwrap_or_default());
                    ip += 1;
                }
                Opcode::SetVar => {
                    let name = bc
                        .const_pool
                        .get(ins.operand & 0x7FFF_FFFF)
                        .unwrap()
                        .to_string();
                    let val = stack.pop().unwrap_or_default();
                    vars.insert(name, val);
                    ip += 1;
                }
                Opcode::Jmp => ip = ins.operand as usize,
                Opcode::JmpIfFail => {
                    if status_failed {
                        ip = ins.operand as usize;
                    } else {
                        ip += 1;
                    }
                }
                Opcode::JmpIfOk => {
                    if !status_failed {
                        ip = ins.operand as usize;
                    } else {
                        ip += 1;
                    }
                }
                Opcode::StatusOk => {
                    status_failed = false;
                    ip += 1;
                }
                Opcode::StatusFail => {
                    status_failed = true;
                    ip += 1;
                }
                Opcode::BashBinary => {
                    let op = bc.const_pool.get(ins.operand).unwrap().to_string();
                    let right = stack.pop().unwrap_or_default();
                    let left = stack.pop().unwrap_or_default();
                    let l: i64 = left.trim().parse().unwrap_or(0);
                    let r: i64 = right.trim().parse().unwrap_or(0);
                    match op.as_str() {
                        "-eq" => status_failed = l != r,
                        "-ne" => status_failed = l == r,
                        _ => {}
                    }
                    ip += 1;
                }
                Opcode::ArithEvalStack => {
                    // ArithParser value semantics: an MBA identity collapses
                    // to E, and "expr+0" evaluates like "expr".
                    let e = stack.pop().unwrap();
                    let e = mba_reduce(&e);
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
                mba: false,
                region_cff: false,
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
                    mba: false,
                    region_cff: false,
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
                    mba: false,
                    region_cff: false,
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
                mba: false,
                region_cff: false,
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
                    mba: false,
                    region_cff: false,
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
                    mba: false,
                    region_cff: false,
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
                            mba: false,
                            region_cff: false,
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
                    mba: false,
                    region_cff: false,
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
                    mba: false,
                    region_cff: false,
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
                    mba: false,
                    region_cff: false,
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
                    mba: false,
                    region_cff: false,
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
                    mba: false,
                    region_cff: false,
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

    // ── mba ──────────────────────────────────────────────────────────────────

    /// E = "3+4" built purely, then evaluated.
    fn sample_mba() -> Bytecode {
        let mut bc = Bytecode::default();
        for s in ["3", "+", "4"] {
            bc.const_pool.strings.push(s.to_string());
        }
        bc.instructions.push(Instruction::new(Opcode::PushConst, 0));
        bc.instructions.push(Instruction::new(Opcode::PushConst, 1));
        bc.instructions.push(Instruction::new(Opcode::PushConst, 2));
        bc.instructions.push(Instruction::new(Opcode::ConcatN, 3));
        bc.instructions
            .push(Instruction::new(Opcode::ArithEvalStack, 0));
        bc.instructions.push(Instruction::no_operand(Opcode::Exit));
        bc
    }

    fn mba_opts(seed: u32) -> ObfuscateOptions {
        ObfuscateOptions {
            bogus_cf: false,
            subst: false,
            condneg: false,
            arith_neutral: false,
            mba: true,
            region_cff: false,
            seed,
        }
    }

    #[test]
    fn mba_preserves_result() {
        let want = replay(&sample_mba());
        assert_eq!(want, vec!["3+4"], "baseline");
        for seed in [0u32, 1, 7, 42, 0xFFFF_FFFF] {
            let mut bc = sample_mba();
            obfuscate(&mut bc, &mba_opts(seed));
            assert_eq!(replay(&bc), want, "seed {seed} changed the result");
        }
    }

    #[test]
    fn mba_fires_and_wraps() {
        let fired = (0u32..32).any(|seed| {
            let mut bc = sample_mba();
            let before = bc.instructions.len();
            obfuscate(&mut bc, &mba_opts(seed));
            if bc.instructions.len() > before {
                assert!(
                    bc.instructions
                        .iter()
                        .any(|i| i.op == Opcode::ConcatN && i.operand == 7),
                    "seed {seed}: expansion must end in ConcatN(7)"
                );
                true
            } else {
                false
            }
        });
        assert!(fired, "mba never fired across 32 seeds");
    }

    #[test]
    fn mba_targets_in_range() {
        // Jmp aimed at the ArithEvalStack: the insert must re-anchor past
        // it, and every operand must stay inside the new stream.
        for seed in [0u32, 1, 7, 42] {
            let mut bc = sample_mba();
            // 0..4 as in sample_mba, 5: Jmp -> 4, 6: Exit
            bc.instructions.insert(5, Instruction::new(Opcode::Jmp, 4));
            obfuscate(&mut bc, &mba_opts(seed));
            let len = bc.instructions.len() as u32;
            for ins in &bc.instructions {
                if matches!(ins.op, Opcode::Jmp | Opcode::JmpIfFail | Opcode::JmpIfOk) {
                    assert!(ins.operand < len, "seed {seed}: target out of range");
                }
            }
            assert!(bc.instructions.iter().any(|i| i.op == Opcode::Exit));
        }
    }

    #[test]
    fn mba_skips_labeled_range() {
        // A jump INTO the build range would skip the leading "(" and
        // misalign ConcatN(7) — the pass must leave the stream alone.
        let build = || {
            let mut bc = sample_mba();
            // 5: Jmp -> 1 (inside the range [0..5))
            bc.instructions.insert(5, Instruction::new(Opcode::Jmp, 1));
            bc
        };
        for seed in 0u32..32 {
            let mut bc = build();
            obfuscate(&mut bc, &mba_opts(seed));
            assert_eq!(
                bc.instructions.len(),
                7,
                "seed {seed}: labeled range must not be rewritten"
            );
        }
    }

    #[test]
    fn mba_seed_varies_layout() {
        let build = |seed| {
            let mut bc = sample_mba();
            obfuscate(&mut bc, &mba_opts(seed));
            format!("{:?}", bc.instructions)
        };
        let a = build(1);
        let b = build(2);
        let c = build(3);
        assert!(a != b || a != c || b != c, "layouts identical across seeds");
    }

    // ── region_cff ───────────────────────────────────────────────────────────

    /// Straight-line region with a status setter, then a status-conditional
    /// branch that observes the region's final $?.
    ///
    /// 0..6: region body (`a`; v=s1; StatusOk; `b`+`c` concat);
    /// 7: JmpIfOk -> 10; 8-9: fail path; 10: ok path; 11: Exit.
    fn sample_cff() -> Bytecode {
        let mut bc = Bytecode::default();
        for s in ["a", "s1", "v", "b", "c", "failpath", "okpath"] {
            bc.const_pool.strings.push(s.to_string());
        }
        bc.instructions.push(Instruction::new(Opcode::PushConst, 0)); // 0: a
        bc.instructions.push(Instruction::new(Opcode::PushConst, 1)); // 1: s1
        bc.instructions.push(Instruction::new(Opcode::SetVar, 2)); // 2: v = s1
        bc.instructions
            .push(Instruction::no_operand(Opcode::StatusOk)); // 3
        bc.instructions.push(Instruction::new(Opcode::PushConst, 3)); // 4: b
        bc.instructions.push(Instruction::new(Opcode::PushConst, 4)); // 5: c
        bc.instructions.push(Instruction::new(Opcode::ConcatN, 2)); // 6: bc
        bc.instructions.push(Instruction::new(Opcode::JmpIfOk, 10)); // 7
        bc.instructions.push(Instruction::new(Opcode::PushConst, 5)); // 8
        bc.instructions.push(Instruction::new(Opcode::Jmp, 11)); // 9
        bc.instructions.push(Instruction::new(Opcode::PushConst, 6)); // 10
        bc.instructions.push(Instruction::no_operand(Opcode::Exit)); // 11
        bc
    }

    fn cff_opts(seed: u32) -> ObfuscateOptions {
        ObfuscateOptions {
            bogus_cf: false,
            subst: false,
            condneg: false,
            arith_neutral: false,
            mba: false,
            region_cff: true,
            seed,
        }
    }

    #[test]
    fn cff_preserves_replay_and_status() {
        for failed in [false, true] {
            let want = replay_with(&sample_cff(), failed);
            assert_eq!(want, vec!["a", "bc", "okpath"], "baseline failed={failed}");
            for seed in [0u32, 1, 7, 42, 0xFFFF_FFFF] {
                let mut bc = sample_cff();
                obfuscate(&mut bc, &cff_opts(seed));
                assert_eq!(
                    replay_with(&bc, failed),
                    want,
                    "seed {seed} failed={failed} changed semantics"
                );
            }
        }
    }

    #[test]
    fn cff_status_pass_through() {
        // Replace the region's setter with StatusFail at the same position:
        // whatever $? entered the region with, the exit status seen by the
        // JmpIfOk must be the region's own last setter.
        let want = vec!["a", "bc", "failpath"];
        let base = {
            let mut bc = sample_cff();
            bc.instructions[3] = Instruction::no_operand(Opcode::StatusFail);
            replay_with(&bc, false)
        };
        assert_eq!(base, want, "baseline wrong");
        let mut fired = false;
        for seed in 0u32..64 {
            let mut bc = sample_cff();
            bc.instructions[3] = Instruction::no_operand(Opcode::StatusFail);
            obfuscate(&mut bc, &cff_opts(seed));
            if bc.instructions.len() > 12 {
                fired = true;
            }
            for failed in [false, true] {
                assert_eq!(
                    replay_with(&bc, failed),
                    want,
                    "seed {seed} failed={failed}: last setter must survive"
                );
            }
        }
        assert!(fired, "region_cff never fired across 64 seeds");
    }

    #[test]
    fn cff_targets_in_range() {
        for seed in 0u32..32 {
            let mut bc = sample_cff();
            obfuscate(&mut bc, &cff_opts(seed));
            let len = bc.instructions.len() as u32;
            for ins in &bc.instructions {
                if matches!(ins.op, Opcode::Jmp | Opcode::JmpIfFail | Opcode::JmpIfOk) {
                    assert!(
                        ins.operand < len,
                        "seed {seed}: target {} out of range (len {len})",
                        ins.operand
                    );
                }
            }
            assert!(bc.instructions.iter().any(|i| i.op == Opcode::Exit));
        }
    }

    #[test]
    fn cff_no_setter_skips() {
        // Six eligible instructions but no status setter: flattening would
        // let the dispatcher's compare clobber the incoming $?, so the pass
        // must leave the stream alone.
        let build = || {
            let mut bc = Bytecode::default();
            for s in ["a", "x", "v", "b", "w", "c"] {
                bc.const_pool.strings.push(s.to_string());
            }
            bc.instructions.push(Instruction::new(Opcode::PushConst, 0));
            bc.instructions.push(Instruction::new(Opcode::PushConst, 1));
            bc.instructions.push(Instruction::new(Opcode::SetVar, 2));
            bc.instructions.push(Instruction::new(Opcode::PushConst, 3));
            bc.instructions.push(Instruction::new(Opcode::SetVar, 4));
            bc.instructions.push(Instruction::new(Opcode::PushConst, 5));
            bc.instructions.push(Instruction::no_operand(Opcode::Exit));
            bc
        };
        let orig_len = build().instructions.len();
        for seed in 0u32..32 {
            let mut bc = build();
            obfuscate(&mut bc, &cff_opts(seed));
            assert_eq!(
                bc.instructions.len(),
                orig_len,
                "seed {seed}: setter-less run must be skipped"
            );
        }
    }

    #[test]
    fn cff_seed_varies_layout() {
        let build = |seed| {
            let mut bc = sample_cff();
            obfuscate(&mut bc, &cff_opts(seed));
            format!("{:?}", bc.instructions)
        };
        let a = build(1);
        let b = build(2);
        let c = build(3);
        assert!(a != b || a != c || b != c, "layouts identical across seeds");
    }
}
