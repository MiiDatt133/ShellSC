//! Post-lowering IR optimizer.
//!
//! Runs on `IrChunk` between lowering and `.sbc` emission. Three passes:
//!
//! - **Constant folding** — `PushConst × n` followed by `ConcatN(n)` folds
//!   into a single `PushConst`.
//! - **Dead code elimination** — ops after a terminal `Exit` are dropped up
//!   to the next live label target (jump target or function entry).
//! - **Peephole jumps** — `Jmp` chains collapse; `Jmp` to the next
//!   instruction is removed.
//!
//! Labels are positional indices, so removals go through an old→new index
//! remap (`compact`). Conditional jumps are never folded — status is
//! runtime state.

use crate::ir::{IrChunk, IrOp};

/// Quick scan: returns true only when at least one pass could plausibly
/// shrink or simplify the chunk. Skips the fixpoint loop entirely for
/// trivial scripts (no ConcatN, no Exit, no Jmp chains).
fn needs_optimization(ops: &[IrOp]) -> bool {
    let mut has_concatn = false;
    let mut has_exit = false;
    let mut has_jmp = false;
    let mut prev_was_jmp = false;
    for op in ops {
        match op {
            IrOp::ConcatN(_) => has_concatn = true,
            IrOp::Exit => has_exit = true,
            IrOp::Jmp(_) => {
                if prev_was_jmp {
                    has_jmp = true;
                }
                prev_was_jmp = true;
                continue;
            }
            _ => {}
        }
        prev_was_jmp = false;
    }
    has_concatn || has_exit || has_jmp
}

pub fn optimize(chunk: IrChunk) -> IrChunk {
    if !needs_optimization(&chunk.ops) {
        return chunk;
    }
    let mut chunk = chunk;
    for _ in 0..4 {
        let before = chunk.ops.len();
        let ops = std::mem::take(&mut chunk.ops);
        let mut marked: Vec<Option<IrOp>> = ops.into_iter().map(Some).collect();

        fold_constants(&mut marked);
        let targets = live_targets(&marked);
        eliminate_dead_after_exit(&mut marked, &targets);
        chase_jump_chains(&mut marked);
        drop_jmp_to_next(&mut marked);
        chunk.ops = compact(marked);
        if chunk.ops.len() == before {
            break;
        }
    }
    chunk
}

// ── Pass A: constant folding ───────────────────────────────────────────

/// Fold windows of `PushConst … PushConst ConcatN(n)` where all n inputs
/// are constants into a single `PushConst(joined)`.
fn fold_constants(ops: &mut [Option<IrOp>]) {
    let mut i = 0;
    while i < ops.len() {
        let Some(Some(IrOp::ConcatN(n))) = ops.get(i) else {
            i += 1;
            continue;
        };
        let n = *n;
        if n >= 2 && i >= n {
            let window = &ops[i - n..i];
            let all_const = window
                .iter()
                .all(|op| matches!(op, Some(IrOp::PushConst(_))));
            if all_const {
                let joined: String = window
                    .iter()
                    .filter_map(|op| match op {
                        Some(IrOp::PushConst(s)) => Some(s.as_str()),
                        _ => None,
                    })
                    .collect();
                for slot in &mut ops[i - n..i] {
                    *slot = None;
                }
                ops[i] = Some(IrOp::PushConst(joined));
            }
        }
        i += 1;
    }
}

// ── Pass B: dead code after Exit ───────────────────────────────────────

/// Indices that must stay reachable: every jump target, pipeline-subshell
/// end, and function entry.
fn live_targets(ops: &[Option<IrOp>]) -> std::collections::HashSet<usize> {
    let mut set = std::collections::HashSet::new();
    for op in ops.iter().flatten() {
        match op {
            IrOp::Jmp(t) | IrOp::JmpIfFail(t) | IrOp::JmpIfOk(t) => {
                set.insert(*t);
            }
            IrOp::PipeSubshellBegin(t) => {
                set.insert(*t);
            }
            IrOp::FuncDef { entry, .. } => {
                set.insert(*entry);
            }
            _ => {}
        }
    }
    set
}

/// Drop unreachable ops after a terminal `Exit`, stopping at any live
/// target (a jump destination may be reachable from elsewhere).
fn eliminate_dead_after_exit(ops: &mut [Option<IrOp>], targets: &std::collections::HashSet<usize>) {
    let mut dead = false;
    for (i, slot) in ops.iter_mut().enumerate() {
        if dead {
            if targets.contains(&i) {
                dead = false;
                continue;
            }
            if matches!(slot, Some(IrOp::Exit)) {
                continue;
            }
            *slot = None;
            continue;
        }
        if matches!(slot, Some(IrOp::Exit)) {
            dead = true;
        }
    }
}

// ── Compaction with label remap ───────────────────────────────────────

/// Remove `None` slots, remapping every label operand through the old→new
/// index map. Panics in debug if a label points at removed code (cannot
/// happen by construction, but catches optimizer bugs early).
fn compact(ops: Vec<Option<IrOp>>) -> Vec<IrOp> {
    let mut map = vec![usize::MAX; ops.len()];
    let mut next = 0usize;
    for (i, slot) in ops.iter().enumerate() {
        if slot.is_some() {
            map[i] = next;
            next += 1;
        }
    }
    let remap = |l: &usize| {
        debug_assert!(
            map[*l] != usize::MAX,
            "optimizer dropped a live label target"
        );
        if map[*l] == usize::MAX {
            // Keep the old index in release builds rather than corrupting
            // the map — visible miscompile, never silent wrong jumps.
            *l
        } else {
            map[*l]
        }
    };
    ops.into_iter()
        .flatten()
        .map(|op| match op {
            IrOp::Jmp(t) => IrOp::Jmp(remap(&t)),
            IrOp::JmpIfFail(t) => IrOp::JmpIfFail(remap(&t)),
            IrOp::JmpIfOk(t) => IrOp::JmpIfOk(remap(&t)),
            IrOp::PipeSubshellBegin(t) => IrOp::PipeSubshellBegin(remap(&t)),
            IrOp::FuncDef { name, entry } => IrOp::FuncDef {
                name,
                entry: remap(&entry),
            },
            other => other,
        })
        .collect()
}

// ── Pass C: peephole jumps ─────────────────────────────────────────────

/// Chase `Jmp → Jmp` chains to their final target (a non-Jmp op). Two
/// iterations because chains can point forward.
fn chase_jump_chains(ops: &mut [Option<IrOp>]) {
    for _ in 0..2 {
        let targets: Vec<usize> = ops
            .iter()
            .map(|slot| match slot {
                Some(IrOp::Jmp(t)) => *t,
                _ => usize::MAX,
            })
            .collect();
        for slot in ops.iter_mut() {
            if let Some(IrOp::Jmp(t)) = slot {
                let mut t = *t;
                let mut hops = 0;
                while hops < targets.len() {
                    match targets.get(t) {
                        Some(next) if *next != usize::MAX => {
                            t = *next;
                            hops += 1;
                        }
                        _ => break,
                    }
                }
                if let Some(IrOp::Jmp(old)) = slot {
                    *old = t;
                }
            }
        }
    }
}

/// Drop `Jmp(i+1)` (fall-through) and redirect any reference to the dropped
/// op through it: `J → J(x)` where op[x] is the dropped `Jmp(x+1)` is
/// rewritten to target `x+1` so no jump ever lands on a removed slot.
fn drop_jmp_to_next(ops: &mut [Option<IrOp>]) {
    let len = ops.len();
    for i in 0..len {
        let is_fallthrough = matches!(&ops[i], Some(IrOp::Jmp(t)) if *t == i + 1 && i + 1 < len);
        if !is_fallthrough {
            continue;
        }
        let next_target = i + 2;
        // Redirect jumps that pointed at the removed op (or chains through
        // it — its own target, already chased to a real op, may still be a
        // fall-through jump removed earlier in this loop, so follow until
        // the slot is not a live Jmp).
        for slot in ops.iter_mut() {
            if let Some(op) = slot {
                let hit = match op {
                    IrOp::Jmp(t) | IrOp::JmpIfFail(t) | IrOp::JmpIfOk(t) => *t == i + 1,
                    IrOp::PipeSubshellBegin(t) => *t == i + 1,
                    IrOp::FuncDef { entry, .. } => *entry == i + 1,
                    _ => false,
                };
                if hit {
                    match op {
                        IrOp::Jmp(t)
                        | IrOp::JmpIfFail(t)
                        | IrOp::JmpIfOk(t)
                        | IrOp::PipeSubshellBegin(t) => *t = next_target,
                        IrOp::FuncDef { entry, .. } => *entry = next_target,
                        _ => unreachable!(),
                    }
                }
            }
        }
        ops[i] = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ops(chunk: &IrChunk) -> String {
        format!("{:?}", chunk.ops)
    }

    #[test]
    fn folds_const_concat() {
        let mut c = IrChunk::new();
        c.push(IrOp::PushConst("a".into()));
        c.push(IrOp::PushConst("b".into()));
        c.push(IrOp::PushConst("c".into()));
        c.push(IrOp::ConcatN(3));
        let out = optimize(c);
        assert_eq!(out.ops.len(), 1);
        assert!(matches!(&out.ops[0], IrOp::PushConst(s) if s == "abc"));
    }

    #[test]
    fn does_not_fold_when_var_present() {
        let mut c = IrChunk::new();
        c.push(IrOp::PushConst("a".into()));
        c.push(IrOp::PushVar("v".into()));
        c.push(IrOp::ConcatN(2));
        let out = optimize(c);
        assert_eq!(out.ops.len(), 3);
    }

    #[test]
    fn removes_code_after_exit_until_target() {
        // Realistic shape: an if-else where Exit is inside one arm and a
        // jump (from the condition, BEFORE the exit) targets the live code.
        // exit; <dead ops>; <target reachable from BEFORE the exit>: live
        let mut c = IrChunk::new();
        let hole = c.push_placeholder(IrOp::Jmp(0)); // 0 — from before, reachable
        c.push(IrOp::Exit); // 1
        c.push(IrOp::PushConst("dead".into())); // 2
        c.push(IrOp::Builtin(crate::builtin::BuiltinId::Echo, 1)); // 3
        let live = c.here(); // 4
        c.push(IrOp::PushConst("live".into())); // 4
        c.push(IrOp::Exit); // 5
        c.patch(hole, live);
        let out = optimize(c);
        assert!(!ops(&out).contains("dead"));
        assert!(ops(&out).contains("live"));
        let jmp_idx = out
            .ops
            .iter()
            .position(|op| matches!(op, IrOp::Jmp(_)))
            .unwrap();
        match &out.ops[jmp_idx] {
            IrOp::Jmp(t) => {
                let target = &out.ops[*t];
                assert!(matches!(target, IrOp::PushConst(s) if s == "live"));
            }
            _ => panic!("expected Jmp"),
        }
    }

    #[test]
    fn keeps_funcdef_after_exit() {
        // Function bodies are skipped over by Jmp but reachable at runtime.
        let mut c = IrChunk::new();
        c.push(IrOp::Exit); // 0
        let entry = c.here() + 1; // entry after the Jmp placeholder
        c.push_placeholder(IrOp::Jmp(0)); // 1
        let body = c.here(); // 2
        c.push(IrOp::PushConst("fn-body".into())); // 2
        c.push(IrOp::FuncReturn); // 3
        let end = c.here();
        c.patch(1, end);
        c.push(IrOp::FuncDef {
            name: "f".into(),
            entry: body,
        });
        let _ = entry;
        let out = optimize(c);
        assert!(ops(&out).contains("fn-body"));
        assert!(ops(&out).contains("FuncDef"));
    }

    #[test]
    fn collapses_jump_chains() {
        let mut c = IrChunk::new();
        // 0: Jmp -> 2 ; 1: PushConst ; 2: Jmp -> 4 ; 3: PushConst ; 4: Exit
        let _ = c.push_placeholder(IrOp::Jmp(2));
        c.push(IrOp::PushConst("x".into()));
        c.push_placeholder(IrOp::Jmp(4));
        c.push(IrOp::PushConst("y".into()));
        let end = c.here();
        c.push(IrOp::Exit);
        // point first jump at end (4)
        c.patch(0, end);
        // second jump at end too
        c.patch(2, end);
        let out = optimize(c);
        let jmps: Vec<_> = out
            .ops
            .iter()
            .filter(|op| matches!(op, IrOp::Jmp(_)))
            .collect();
        // both jumps should now target the Exit directly (single hop)
        for j in jmps {
            match j {
                IrOp::Jmp(t) => assert!(matches!(out.ops[*t], IrOp::Exit)),
                _ => unreachable!(),
            }
        }
    }

    #[test]
    fn drops_jmp_to_next() {
        let mut c = IrChunk::new();
        let hole = c.push_placeholder(IrOp::Jmp(0));
        let next = c.here();
        c.push(IrOp::PushConst("after".into()));
        c.push(IrOp::Exit);
        c.patch(hole, next);
        let out = optimize(c);
        assert!(!ops(&out).contains("Jmp"));
    }
}
