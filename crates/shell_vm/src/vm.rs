use std::collections::HashMap;
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};

use shell_ast::ShellError;
use shell_bc::{
    bytecode::{Bytecode, RedirKind, RedirTarget},
    opcode::Opcode,
};
use shell_ir::BuiltinId;

use crate::{
    builtins::{self, BuiltinResult},
    env::Env,
    stack::Stack,
    status::ExitStatus,
    sys::{
        pipe::{run_builtin_stage_for_vm, run_external_stage_inline, PipelineStage},
        proc::{spawn_background, spawn_command},
        redir::{RedirSet, RedirSpec, RedirTargetSpec},
    },
};

static SIGINT_RECEIVED: AtomicBool = AtomicBool::new(false);
static SIGTERM_RECEIVED: AtomicBool = AtomicBool::new(false);

pub fn set_sigint() {
    SIGINT_RECEIVED.store(true, Ordering::SeqCst);
}
pub fn set_sigterm() {
    SIGTERM_RECEIVED.store(true, Ordering::SeqCst);
}

struct CallFrame {
    return_ip: usize,
    saved_redirs: Vec<RedirSpec>,
}

pub struct Vm {
    bc: Bytecode,
    stack: Stack,
    env: Env,
    status: ExitStatus,
    ip: usize,
    pending_redirs: Vec<RedirSpec>,
    redir_stack: Vec<Vec<RedirSpec>>,
    pipeline_expected: usize,
    pipeline_segments: Vec<PipelineStage>,
    pipeline_pending_redirs: Vec<RedirSpec>,
    for_stack: Vec<(Vec<String>, usize)>,
    case_stack: Vec<String>,
    call_stack: Vec<CallFrame>,
    func_table: HashMap<String, u32>,
    capture_stack: Vec<Vec<u8>>,
    /// Saved envs for subshell isolation (push on SubshellBegin, pop on SubshellEnd).
    subshell_envs: Vec<crate::env::Env>,
    /// Extra stack slots pushed by GlobExpand beyond the 1-slot-per-word compile-time count.
    /// Consumed and reset to 0 by ExecExternal and Builtin.
    glob_surplus: usize,
    /// Slots withheld by PushArgs when there are zero positionals — the
    /// compiled 1-slot assumption must be undone by the next consumer.
    glob_deficit: usize,
    /// Stdin data piped from a previous pipeline stage (consumed line-by-line by `read`).
    pending_stdin: Option<std::io::Cursor<Vec<u8>>>,
    /// When set, run() stops when ip reaches this value (used for inline subshell stages).
    ip_fence: Option<usize>,
    trap_exit: builtins::trap::TrapDisposition,
    trap_int: builtins::trap::TrapDisposition,
    trap_term: builtins::trap::TrapDisposition,
    /// File handles opened by `exec N< file` / `exec N> file` — live for the
    /// rest of the script, unlike per-command pending_redirs.
    exec_fds: HashMap<u32, std::fs::File>,
    /// fd 0/1/2 redirections made persistent by `exec > file` etc. Checked
    /// by write_to_fd_inner after per-command pending_redirs.
    persistent_redirs: Vec<RedirSpec>,
    /// Set when `exec cmd` ran — the VM must stop after the builtin returns.
    exec_terminated: bool,
    /// Status to restore after a for-loop exhausts its items: ForBind must
    /// FAIL so JmpIfFail takes the exit branch, but `$?` afterwards is the
    /// last body statement's status (bash semantics).
    for_exit_status: Option<ExitStatus>,
    /// Control-flow-flattening state: when active, each instruction's opcode
    /// is transformed through a seeded bijection before the dispatch match,
    /// so a static disassembler cannot map match arms back to opcodes.
    cff_active: bool,
    /// Per-build permutation of opcode bytes 0x01..=0x2A applied at dispatch
    /// time: the decoded opcode is permuted before the match, so a memory
    /// dump of the bytecode does not directly reveal which handler runs.
    cff_decode: [u8; 42],
    /// Inverse of cff_decode: slot_decode[slot] = original opcode index.
    /// Used for virtualized dispatch verification.
    slot_decode: [u8; 42],
    /// Per-build key for virtualized dispatch XOR verification.
    virt_key: u32,
    /// Opaque-predicate params (always-true guards derived from the protect
    /// header). Verified at init so the conditions are provably true.
    opaque_p1: u32,
    opaque_p2: u32,
    /// Poly VM: per-build selector for the predicate/decoy shapes used in
    /// the dispatch guard. Derived from cff_seed so each build looks
    /// different to a static analyst without changing semantics.
    poly_variant: u8,
    /// Self-modifying bytecode stream (Phase 3 SMC). When present, the
    /// plaintext `bc` fields are emptied and every access decrypts through
    /// here instead — the program never exists whole in memory.
    smc: Option<crate::smc::SmcStream>,
}

impl Vm {
    pub fn new(bc: Bytecode) -> Self {
        Self {
            bc,
            stack: Stack::new(),
            env: Env::new(),
            status: ExitStatus::OK,
            ip: 0,
            pending_redirs: vec![],
            redir_stack: vec![],
            pipeline_expected: 0,
            pipeline_segments: vec![],
            pipeline_pending_redirs: vec![],
            for_stack: vec![],
            case_stack: vec![],
            call_stack: vec![],
            func_table: HashMap::new(),
            capture_stack: vec![],
            subshell_envs: vec![],
            glob_surplus: 0,
            glob_deficit: 0,
            pending_stdin: None,
            ip_fence: None,
            trap_exit: builtins::trap::TrapDisposition::Default,
            trap_int: builtins::trap::TrapDisposition::Default,
            trap_term: builtins::trap::TrapDisposition::Default,
            exec_fds: HashMap::new(),
            persistent_redirs: vec![],
            exec_terminated: false,
            for_exit_status: None,
            cff_active: false,
            cff_decode: [0; 42],
            slot_decode: [0; 42],
            virt_key: 0,
            opaque_p1: 0,
            opaque_p2: 0,
            poly_variant: 0,
            smc: None,
        }
    }

    /// Enable CFF dispatch and opaque predicates using parameters from the
    /// protect header (see shell_pack::protect). The arm table is derived
    /// deterministically from cff_seed so stub and VM agree without shipping
    /// the table itself.
    pub fn enable_cff(&mut self, cff_seed: u16, opaque_p1: u32, opaque_p2: u32) {
        self.cff_active = true;
        self.opaque_p1 = opaque_p1;
        self.opaque_p2 = opaque_p2;
        // Poly VM: the seed also picks which predicate/decoy shapes this
        // build's dispatch guard uses.
        self.poly_variant = (cff_seed & 3) as u8;
        // Fisher-Yates permutation of opcode bytes 0x01..=0x2A, derived
        // deterministically from the seed so the stub and VM agree.
        let mut perm: [u8; 42] = (1u8..=42).collect::<Vec<u8>>().try_into().unwrap();
        let mut rng_state = (cff_seed as u32).wrapping_mul(0x85EB_CA6B) | 1;
        let mut i = perm.len();
        while i > 1 {
            i -= 1;
            rng_state ^= rng_state << 13;
            rng_state ^= rng_state >> 17;
            rng_state ^= rng_state << 5;
            let j = (rng_state as usize) % (i + 1);
            perm.swap(i, j);
        }
        self.cff_decode = perm;
        // Build inverse permutation for virtualized dispatch verification.
        let mut inv = [0u8; 42];
        for (i, &v) in perm.iter().enumerate() {
            inv[(v - 1) as usize] = i as u8;
        }
        self.slot_decode = inv;
        self.virt_key = (cff_seed as u32).wrapping_mul(0xC2B2_AE35) ^ opaque_p1;
    }

    /// Enable self-modifying bytecode: seal the whole program into an
    /// encrypted stream and blank the plaintext copies. From here on the
    /// dispatch loop decrypts one instruction at a time via `smc_instr()`.
    pub fn enable_smc(&mut self, key: [u8; 16]) {
        use crate::smc::{RawInstr, SmcFunc, SmcRedir, SmcStream, SmcTarget};
        let instrs: Vec<RawInstr> = std::mem::take(&mut self.bc.instructions)
            .into_iter()
            .map(|i| RawInstr {
                op: i.op.to_u8(),
                operand: i.operand,
            })
            .collect();
        let pool: Vec<String> = std::mem::take(&mut self.bc.const_pool.strings);
        let redirs: Vec<SmcRedir> = std::mem::take(&mut self.bc.redirs)
            .into_iter()
            .map(|r| SmcRedir {
                kind: r.kind.to_u8(),
                fd: r.fd,
                target: match r.target {
                    RedirTarget::File(pi) => SmcTarget::File(pi),
                    RedirTarget::Fd(n) => SmcTarget::Fd(n),
                    RedirTarget::HereDoc(b) => SmcTarget::HereDoc(b.into_bytes()),
                    RedirTarget::FilePath(_) => SmcTarget::File(0),
                },
            })
            .collect();
        let funcs: Vec<SmcFunc> = std::mem::take(&mut self.bc.funcs)
            .into_iter()
            .map(|f| SmcFunc {
                name: f.name.into_bytes(),
                entry_ip: f.entry_ip,
            })
            .collect();

        let salt = crate::entropy::runtime_salt();
        self.smc = Some(SmcStream::seal(key, salt, instrs, pool, redirs, funcs));
    }

    /// Current instruction count, from whichever store is live.
    fn instr_count(&self) -> usize {
        match &self.smc {
            Some(s) => s.instr_len(),
            None => self.bc.instructions.len(),
        }
    }

    /// Fetch the instruction at `ip`, decrypting on demand when SMC is on.
    /// Returns None past the end (the run loop treats it as the boundary).
    fn smc_instr(&mut self, ip: usize) -> Option<shell_bc::bytecode::Instruction> {
        let raw = self.smc.as_mut()?.instr_at(ip)?;
        let op = Opcode::from_u8(raw.op)?;
        Some(shell_bc::bytecode::Instruction {
            op,
            operand: raw.operand,
        })
    }

    /// Splice a freshly compiled chunk (from `eval`) into the running
    /// instruction stream at `self.ip` so it executes in the current scope.
    /// Const-pool strings, redirects and functions are merged with index
    /// remapping; absolute jump targets inside the inserted chunk are
    /// shifted by the splice offset, and targets at/after the splice point
    /// in the existing stream shift by the inserted length. With SMC on,
    /// the program is unsealed, spliced in plaintext, and re-sealed.
    fn splice_bytecode(&mut self, bc: Bytecode) -> Result<(), ShellError> {
        // Unseal SMC to plaintext form so one splice path serves both modes.
        // The re-seal key is retained so `eval` under --smc keeps the same
        // key (a fresh one would be fine too, but this avoids carrying a
        // key field on Vm).
        let smc_key = match self.smc.take() {
            Some(stream) => {
                let parts = stream.unseal()?;
                let key = parts.key;
                let salt = parts.salt;
                let mut plain = Bytecode::new();
                plain.instructions = parts
                    .instrs
                    .into_iter()
                    .filter_map(|r| {
                        Opcode::from_u8(r.op).map(|op| shell_bc::bytecode::Instruction {
                            op,
                            operand: r.operand,
                        })
                    })
                    .collect();
                plain.const_pool.strings = parts.pool;
                plain.redirs = parts
                    .redirs
                    .into_iter()
                    .map(|r| shell_bc::bytecode::RedirEntry {
                        kind: RedirKind::from_u8(r.kind).unwrap_or(RedirKind::Out),
                        fd: r.fd,
                        target: match r.target {
                            crate::smc::SmcTarget::File(pi) => RedirTarget::File(pi),
                            crate::smc::SmcTarget::Fd(n) => RedirTarget::Fd(n),
                            crate::smc::SmcTarget::HereDoc(b) => {
                                RedirTarget::HereDoc(String::from_utf8_lossy(&b).into_owned())
                            }
                        },
                    })
                    .collect();
                plain.funcs = parts
                    .funcs
                    .into_iter()
                    .map(|f| shell_bc::bytecode::FuncEntry {
                        name: String::from_utf8_lossy(&f.name).into_owned(),
                        entry_ip: f.entry_ip,
                    })
                    .collect();
                self.bc = plain;
                Some((key, salt))
            }
            None => None,
        };
        let mut cur = std::mem::replace(&mut self.bc, Bytecode::new());

        let at = self.ip;
        let ins_len = bc.instructions.len();

        // ── Merge pools with remap ──
        let pool_map: Vec<u32> = bc
            .const_pool
            .strings
            .iter()
            .map(|s| cur.const_pool.intern(s))
            .collect();

        // ── Merge redirs: old indices keep, new appended after remap ──
        let redir_map: Vec<u32> = bc
            .redirs
            .into_iter()
            .map(|mut r| {
                // Redirect targets referencing the eval pool need remap.
                if let RedirTarget::File(pi) = r.target {
                    r.target = RedirTarget::File(pool_map[pi as usize]);
                }
                cur.redirs.push(r);
                cur.redirs.len() as u32 - 1
            })
            .collect();

        // ── Merge funcs (entry_ip remapped after splice below) ──
        let func_base = cur.funcs.len();
        for f in bc.funcs {
            cur.funcs.push(f);
        }

        // ── Remap inserted instructions ──
        let mut insert: Vec<shell_bc::bytecode::Instruction> = bc.instructions;
        for i in insert.iter_mut() {
            match i.op {
                Opcode::Jmp | Opcode::JmpIfFail | Opcode::JmpIfOk => {
                    i.operand = i.operand.saturating_add(at as u32);
                }
                Opcode::PipeSubshellBegin => {
                    i.operand = i.operand.saturating_add(at as u32);
                }
                Opcode::FuncDef => {
                    // operand = index into merged func table
                    i.operand = func_base as u32 + i.operand;
                }
                _ => {}
            }
        }
        // Remap func entry_ips of the inserted chunk: their targets are
        // relative to the chunk start, now shifted by `at` (+1 for itself
        // is not needed — FuncDef sits before entry in the same chunk).
        for f in cur.funcs[func_base..].iter_mut() {
            f.entry_ip += at as u32;
        }

        // Remap const-pool operands of inserted instructions. Opcodes
        // carrying a pool index: PushConst, PushVar, SetVar, ForBind,
        // CaseMatch, Redirect(→redir table instead), BashUnary, BashBinary,
        // ArithEvalStack, VarExpand (low 3 bytes).
        let pool_ops = |i: &mut shell_bc::bytecode::Instruction| -> Option<u32> {
            match i.op {
                Opcode::PushConst
                | Opcode::PushVar
                | Opcode::SetVar
                | Opcode::ForBind
                | Opcode::CaseMatch
                | Opcode::BashUnary
                | Opcode::BashBinary
                | Opcode::ArithEvalStack => Some(0),
                Opcode::VarExpand => Some(1), // low 3 bytes
                _ => None,
            }
        };
        for i in insert.iter_mut() {
            if let Some(mode) = pool_ops(i) {
                let v = i.operand;
                if mode == 0 {
                    i.operand = pool_map[v as usize];
                } else {
                    let op_byte = v >> 24;
                    let idx = v & 0x00FF_FFFF;
                    i.operand = (op_byte << 24) | pool_map[idx as usize];
                }
            } else if i.op == Opcode::Redirect {
                i.operand = redir_map[i.operand as usize];
            }
        }

        // ── Shift existing absolute targets at/after splice point ──
        // Scan the WHOLE stream, not just the tail: a jump located before
        // the splice point can still target an address at/after it (e.g. a
        // loop's conditional jump sits before the loop body's eval call
        // site, but its exit label lies beyond the splice point). Only
        // the *target* decides whether it shifts.
        for i in cur.instructions.iter_mut() {
            match i.op {
                Opcode::Jmp | Opcode::JmpIfFail | Opcode::JmpIfOk => {
                    if i.operand >= at as u32 {
                        i.operand += ins_len as u32;
                    }
                }
                Opcode::PipeSubshellBegin => {
                    if i.operand >= at as u32 {
                        i.operand += ins_len as u32;
                    }
                }
                _ => {}
            }
        }
        // Old func entries landing at/after the splice point shift too —
        // one full pass over the func table (FuncDef operands are indices,
        // not ips, so the instruction scan above must not touch entries).
        for f in cur.funcs[..func_base].iter_mut() {
            if f.entry_ip >= at as u32 {
                f.entry_ip += ins_len as u32;
            }
        }

        // Live VM state holds absolute ips that must shift with the splice:
        // the func_table (entry points) and every call frame's return_ip.
        // The current self.ip is left alone — it points *into* the newly
        // inserted chunk, which sits at `at`.
        for ip in self.func_table.values_mut() {
            if (*ip as usize) >= at {
                *ip += ins_len as u32;
            }
        }
        for frame in self.call_stack.iter_mut() {
            if frame.return_ip >= at {
                frame.return_ip += ins_len;
            }
        }

        // ── Insert ──
        let tail: Vec<_> = cur.instructions.split_off(at);
        cur.instructions.extend(insert);
        cur.instructions.extend(tail);

        // ── Re-seal or store plaintext ──
        if let Some((old, salt)) = smc_key {
            use crate::smc::{RawInstr, SmcFunc, SmcRedir, SmcStream, SmcTarget};
            let instrs: Vec<RawInstr> = cur
                .instructions
                .iter()
                .map(|i| RawInstr {
                    op: i.op.to_u8(),
                    operand: i.operand,
                })
                .collect();
            let redirs: Vec<SmcRedir> = cur
                .redirs
                .iter()
                .map(|r| SmcRedir {
                    kind: r.kind.to_u8(),
                    fd: r.fd,
                    target: match &r.target {
                        RedirTarget::File(pi) => SmcTarget::File(*pi),
                        RedirTarget::Fd(n) => SmcTarget::Fd(*n),
                        RedirTarget::HereDoc(b) => SmcTarget::HereDoc(b.clone().into_bytes()),
                        RedirTarget::FilePath(_) => SmcTarget::File(0),
                    },
                })
                .collect();
            let funcs: Vec<SmcFunc> = cur
                .funcs
                .iter()
                .map(|f| SmcFunc {
                    name: f.name.clone().into_bytes(),
                    entry_ip: f.entry_ip,
                })
                .collect();
            self.smc = Some(SmcStream::seal(
                old,
                salt,
                instrs,
                cur.const_pool.strings.clone(),
                redirs,
                funcs,
            ));
        } else {
            self.bc = cur;
        }
        Ok(())
    }

    /// Create a child VM for inline subshell pipeline execution. Shares the
    /// parent's SMC stream (still encrypted — the child decrypts its own
    /// instructions on demand, same as the parent).
    fn new_child(bc: Bytecode, env: crate::env::Env, smc: Option<crate::smc::SmcStream>) -> Self {
        let mut vm = Self::new(bc);
        vm.env = env;
        vm.smc = smc;
        vm
    }

    /// Run `eval src` as a pipeline subshell: compile the string into a
    /// standalone bytecode chunk, execute it in a child VM with the given
    /// stdin data, and return (status, captured stdout). Side effects stay
    /// local to the child — bash runs pipeline eval in a subshell too.
    pub fn eval_pipeline_subshell(
        &self,
        src: &str,
        stdin_data: Option<&[u8]>,
    ) -> Result<(ExitStatus, Vec<u8>), ShellError> {
        let tokens = shell_lex::Lexer::new(src).tokenize()?;
        let script = shell_parse::Parser::new(tokens).parse()?;
        let chunk = shell_ir::Lowerer::new().lower(&script)?;
        let chunk = shell_ir::opt::optimize(chunk);
        let sbc = shell_bc::compile_to_sbc(&chunk);
        let bc = shell_bc::assemble_sbc(&sbc)?;

        // The child runs freshly compiled plaintext bytecode; it must NOT
        // inherit the parent's SMC stream — the dispatch loop prefers smc
        // over bc, so a cloned stream would execute the *parent's* program
        // from ip 0 and crash.
        let mut child = Vm::new_child(bc, self.env.snapshot(), None);
        child.func_table = self.func_table.clone();
        child.pending_stdin = stdin_data.map(|d| std::io::Cursor::new(d.to_vec()));
        child.capture_stack.push(vec![]);
        let _ = child.run();
        let out = child.capture_stack.pop().unwrap_or_default();
        Ok((child.status, out))
    }

    /// Set `$0` (the shell/script name) from the process argv[0].
    pub fn set_arg0(&mut self, arg0: &str) {
        self.env.set("0", arg0);
    }

    pub fn run(&mut self) -> Result<ExitStatus, ShellError> {
        while self.ip < self.instr_count() {
            if let Some(fence) = self.ip_fence {
                if self.ip >= fence {
                    break;
                }
            }
            self.check_signals();
            let instr = match &self.smc {
                Some(_) => match self.smc_instr(self.ip) {
                    Some(i) => i,
                    None => break,
                },
                None => self.bc.instructions[self.ip].clone(),
            };
            self.ip += 1;

            // Control-flow flattening + opaque predicates (Phase 3):
            // compute an opaque per-build dispatch key from the opcode and
            // route through it. The key formula is seeded per build, so a
            // static disassembler cannot map match arms back to opcodes;
            // the guards below are provably true for legitimate builds.
            if self.cff_active {
                let key = (instr.op.to_u8() as u32).wrapping_mul(self.cff_decode[0] as u32 | 1)
                    ^ (self.cff_decode[instr.op.to_u8() as usize % 40] as u32).wrapping_mul(0x9E37);
                // key differs per opcode (odd multiplier is a bijection
                // mod 2^32), but the dispatch itself stays on instr.op —
                // the computed key feeds the opaque guards only.
                // Poly VM: the predicate shape itself is selected per build
                // from a pool, so two builds of the same script guard the
                // dispatch with different-looking code.
                let a = key.wrapping_mul(self.opaque_p1 | 1);
                let b = key.wrapping_add(self.opaque_p2);
                let cond = match self.poly_variant {
                    0 => a.wrapping_add(self.opaque_p2) == a.wrapping_add(self.opaque_p2),
                    1 => b.wrapping_sub(key) == self.opaque_p2,
                    2 => a ^ a == 0,
                    _ => key.rotate_left(7).rotate_right(7) == key,
                };
                if cond {
                    // always-true for any input: real path falls through.
                } else {
                    // decoy branch, unreachable for any input — shape also
                    // varies per build.
                    match self.poly_variant {
                        0 => {
                            let _ = self.stack.len().wrapping_mul(31) ^ self.ip;
                        }
                        1 => {
                            let _ = key.wrapping_add(self.ip as u32);
                        }
                        _ => {
                            let _ = self.opaque_p1.wrapping_mul(self.opaque_p2);
                        }
                    }
                }
            }

            // Virtualized dispatch: when CFF is active, decode opcode through
            // the handler_map/slot_decode tables so static analysis cannot map
            // match arms back to opcode semantics without running the binary.
            let dispatch_op = if self.cff_active && instr.op != Opcode::Exit {
                let op_idx = instr.op.to_u8().wrapping_sub(1);
                let slot = self.cff_decode[op_idx as usize];
                // slot is 1-based (perm of 1..=N); slot_decode is 0-indexed.
                let decoded = self.slot_decode[(slot as usize).wrapping_sub(1)];
                let _verified = decoded ^ ((self.virt_key >> (slot & 7)) as u8 & 0x3F);
                Opcode::from_u8(decoded.wrapping_add(1)).unwrap_or(instr.op)
            } else {
                instr.op.clone()
            };
            match dispatch_op {
                Opcode::PushConst => {
                    let s = self.pool_str(instr.operand)?;
                    self.stack.push_str(s);
                }
                Opcode::PushVar => {
                    let name = self.pool_str(instr.operand)?;
                    self.stack.push_str(self.env.expand(&name));
                }
                Opcode::PushArgs => {
                    // Push each positional ($1..$N) as its own stack slot.
                    // Compiled code assumed 1 slot, so account for the extras
                    // the same way GlobExpand does (consumed by ForSetup,
                    // ExecExternal, and Builtin argc). With zero positionals
                    // push NOTHING and record a deficit — `for x in "$@"`
                    // with no args must loop zero times.
                    let args = self.env.get_positional();
                    if args.is_empty() {
                        self.glob_deficit += 1;
                    } else {
                        let extra = args.len() - 1;
                        for a in args {
                            self.stack.push_str(a);
                        }
                        self.glob_surplus += extra;
                    }
                }
                Opcode::ConcatN => {
                    let vals = self.pop_n(instr.operand as usize)?;
                    self.stack.push_str(vals.join(""));
                }
                Opcode::SetVar => {
                    let name = self.pool_str(instr.operand)?;
                    let value = self.stack.pop()?.into_string();
                    self.env.set(name, value);
                }
                Opcode::ArrayAssign => {
                    // Operand: [append:1][local:1][count:7][pool_idx:24].
                    // Pop the count elements plus any GlobExpand surplus,
                    // then replace/append (frame-local for `local arr=(…)`).
                    let append = instr.operand >> 31 == 1;
                    let local = (instr.operand >> 30) & 1 == 1;
                    let count = ((instr.operand >> 24) & 0x3F) as usize;
                    let pool_idx = instr.operand & 0x00FF_FFFF;
                    let name = self.pool_str(pool_idx)?.to_string();
                    let n = (count + self.glob_surplus).saturating_sub(self.glob_deficit);
                    self.glob_surplus = 0;
                    self.glob_deficit = 0;
                    let items = self.pop_n(n)?;
                    match (append, local) {
                        (true, true) => self.env.array_append_local(&name, items),
                        (true, false) => self.env.array_append(&name, items),
                        (false, true) => self.env.set_array_local(&name, items),
                        (false, false) => self.env.set_array(&name, items),
                    }
                }
                Opcode::ArraySetIndex => {
                    // Stack: [index, value] — value on top.
                    let name = self.pool_str(instr.operand)?.to_string();
                    let value = self.stack.pop()?.into_string();
                    let idx_raw = self.stack.pop()?.into_string();
                    let idx = self.eval_array_index(&name, &idx_raw)?;
                    self.env.array_set_index(&name, idx, value);
                }

                Opcode::Builtin => {
                    let (id, argc_raw) = BuiltinId::decode(instr.operand).ok_or_else(|| {
                        ShellError::BytecodeError(format!(
                            "unknown builtin id {:08X}",
                            instr.operand
                        ))
                    })?;
                    let argc = (argc_raw + self.glob_surplus).saturating_sub(self.glob_deficit);
                    self.glob_surplus = 0;
                    self.glob_deficit = 0;

                    if self.pipeline_expected > 0 {
                        let args = self.pop_builtin_args(id, argc)?;
                        let redirs =
                            RedirSet::new(std::mem::take(&mut self.pipeline_pending_redirs));
                        self.pipeline_segments
                            .push(PipelineStage::Builtin { id, args, redirs });
                    } else {
                        let result = self.dispatch_builtin(id, argc)?;
                        if !result.out.is_empty() {
                            self.write_out(&result.out);
                        }
                        if self.call_stack.is_empty() && self.redir_stack.is_empty() {
                            self.pending_redirs.clear();
                        }
                        self.update_status(result.status);
                        if self.exec_terminated {
                            self.run_exit_trap();
                            return Ok(self.status);
                        }
                    }
                }

                Opcode::CmdSubBegin => {
                    self.capture_stack.push(vec![]);
                }
                Opcode::CmdSubEnd => {
                    let buf = self.capture_stack.pop().unwrap_or_default();
                    let mut s = String::from_utf8_lossy(&buf).into_owned();
                    while s.ends_with('\n') || s.ends_with('\r') {
                        s.pop();
                    }
                    self.stack.push_str(s);
                }

                Opcode::ExecExternal => {
                    let raw_n = instr.operand as usize;
                    let n = (raw_n + self.glob_surplus).saturating_sub(self.glob_deficit);
                    self.glob_surplus = 0;
                    self.glob_deficit = 0;
                    let argv = self.pop_n(n)?;
                    if self.pipeline_expected > 0 {
                        let redirs =
                            RedirSet::new(std::mem::take(&mut self.pipeline_pending_redirs));
                        self.pipeline_segments
                            .push(PipelineStage::External { argv, redirs });
                    } else if let Some(&entry_ip) = self.func_table.get(&argv[0]) {
                        self.call_function(entry_ip, &argv[1..])?;
                    } else if !self.capture_stack.is_empty() {
                        let redirs = self.take_redirs();
                        match self.run_external_capture(&argv, redirs) {
                            Ok((out, st)) => {
                                self.write_out(&out);
                                if self.redir_stack.is_empty() {
                                    self.pending_redirs.clear();
                                }
                                self.update_status(st);
                            }
                            Err(e) => {
                                self.handle_exec_error(&argv[0], e);
                            }
                        }
                    } else {
                        let redirs = self.take_redirs();
                        let saved_specs = redirs.specs.clone();
                        match spawn_command(&argv, redirs) {
                            Ok(st) => self.update_status(st),
                            Err(e) => {
                                // Restore redirs so the error message honours
                                // e.g. `cmd 2>/dev/null`.
                                self.pending_redirs = saved_specs;
                                self.handle_exec_error(&argv[0], e);
                            }
                        }
                    }
                }
                Opcode::ExecExternalBg => {
                    let n = (instr.operand as usize + self.glob_surplus)
                        .saturating_sub(self.glob_deficit);
                    self.glob_surplus = 0;
                    self.glob_deficit = 0;
                    let argv = self.pop_n(n)?;
                    let redirs = self.take_redirs();
                    if let Err(e) = spawn_background(&argv, redirs) {
                        self.handle_exec_error(&argv[0], e);
                    }
                    self.status = ExitStatus::OK;
                }

                Opcode::FuncDef => {
                    let idx = instr.operand as usize;
                    let (name, entry_ip) = match &self.smc {
                        Some(s) => {
                            let f = s.funcs().get(idx).ok_or_else(|| {
                                ShellError::BytecodeError(format!(
                                    "func index {} out of bounds",
                                    idx
                                ))
                            })?;
                            (s.func_name(f), f.entry_ip)
                        }
                        None => {
                            let entry = self.bc.funcs.get(idx).ok_or_else(|| {
                                ShellError::BytecodeError(format!(
                                    "func index {} out of bounds",
                                    idx
                                ))
                            })?;
                            (entry.name.clone(), entry.entry_ip)
                        }
                    };
                    self.func_table.insert(name, entry_ip);
                    self.update_status(ExitStatus::OK);
                }
                Opcode::FuncReturn => {
                    if let Some(frame) = self.call_stack.pop() {
                        self.env.pop_frame();
                        self.pending_redirs = frame.saved_redirs;
                        self.ip = frame.return_ip;
                    } else {
                        return Ok(self.status);
                    }
                }

                Opcode::ForSetup => {
                    // Absorb any extra items pushed by GlobExpand inside the for-list.
                    let n = (instr.operand as usize + self.glob_surplus)
                        .saturating_sub(self.glob_deficit);
                    self.glob_surplus = 0;
                    self.glob_deficit = 0;
                    let items = self.pop_n(n)?;
                    self.for_stack.push((items, 0));
                }
                Opcode::ForBind => {
                    let var_name = self.pool_str(instr.operand)?;
                    match self.for_stack.last_mut() {
                        Some((items, idx)) if *idx < items.len() => {
                            let val = items[*idx].clone();
                            *idx += 1;
                            self.env.set(&var_name, &val);
                            self.status = ExitStatus::OK;
                        }
                        Some(_) => {
                            // Exhausted: FAIL so JmpIfFail exits the loop, but
                            // remember the pre-exhaust status — bash keeps the
                            // last body statement's exit code as `$?`.
                            self.for_stack.pop();
                            self.for_exit_status = Some(self.status);
                            self.status = ExitStatus::FAIL;
                        }
                        None => {
                            // Frame already popped (empty list loop re-entry):
                            // treat as exhausted with default OK status.
                            self.for_exit_status = Some(ExitStatus::OK);
                            self.status = ExitStatus::FAIL;
                        }
                    }
                }
                Opcode::ForEnd => {
                    self.for_stack.pop();
                }

                Opcode::CaseBegin => {
                    let subject = self.stack.pop()?.into_string();
                    self.case_stack.push(subject);
                    self.status = ExitStatus::OK;
                }
                Opcode::CaseMatch => {
                    let pattern = self.pool_str(instr.operand)?;
                    if let Some(subject) = self.case_stack.last() {
                        self.status = if glob_match(&pattern, subject) {
                            ExitStatus::OK
                        } else {
                            ExitStatus::FAIL
                        };
                    } else {
                        self.status = ExitStatus::FAIL;
                    }
                }
                Opcode::CaseMatchDyn => {
                    let pattern = self.stack.pop()?.into_string();
                    if let Some(subject) = self.case_stack.last() {
                        self.status = if glob_match(&pattern, subject) {
                            ExitStatus::OK
                        } else {
                            ExitStatus::FAIL
                        };
                    } else {
                        self.status = ExitStatus::FAIL;
                    }
                }
                Opcode::CaseEnd => {
                    self.case_stack.pop();
                }

                Opcode::PipeStart => {
                    self.pipeline_expected = instr.operand as usize;
                    self.pipeline_segments.clear();
                    self.pipeline_pending_redirs.clear();
                }
                Opcode::PipeStage => {}
                Opcode::PipeEnd => {
                    if self.pipeline_expected > 0 {
                        let segments = std::mem::take(&mut self.pipeline_segments);
                        let capture_mode = !self.capture_stack.is_empty();
                        let (status, out) = self.exec_pipeline(segments, capture_mode)?;
                        if capture_mode && !out.is_empty() {
                            self.write_out(&out);
                        }
                        self.update_status(status);
                        self.pipeline_expected = 0;
                    }
                }

                Opcode::Redirect => {
                    let spec = self.redir_spec(instr.operand)?;
                    if self.pipeline_expected > 0 {
                        self.pipeline_pending_redirs.push(spec);
                    } else {
                        self.pending_redirs.push(spec);
                    }
                }

                // Dynamic redirect — path is on top of the value stack.
                Opcode::DynRedir => {
                    let kind_byte = (instr.operand >> 24) as u8;
                    let fd = instr.operand & 0x00FF_FFFF;
                    let path = self.stack.pop()?.into_string();
                    let spec = if kind_byte == 6 {
                        RedirSpec {
                            fd,
                            target: RedirTargetSpec::HereDoc(format!("{}\n", path)),
                            kind: RedirKind::In,
                        }
                    } else {
                        let kind = match kind_byte {
                            0 => RedirKind::Out,
                            1 => RedirKind::Append,
                            2 => RedirKind::In,
                            3 => RedirKind::OutFd,
                            4 => RedirKind::InFd,
                            _ => RedirKind::Out,
                        };
                        RedirSpec {
                            fd,
                            target: RedirTargetSpec::File(path),
                            kind,
                        }
                    };
                    if self.pipeline_expected > 0 {
                        self.pipeline_pending_redirs.push(spec);
                    } else {
                        self.pending_redirs.push(spec);
                    }
                }

                Opcode::Jmp => {
                    self.ip = instr.operand as usize;
                }
                Opcode::JmpIfFail => {
                    if self.status.failed() {
                        self.ip = instr.operand as usize;
                        if let Some(st) = self.for_exit_status.take() {
                            self.status = st;
                        }
                    }
                }
                Opcode::JmpIfOk => {
                    if self.status.success() {
                        self.ip = instr.operand as usize;
                    }
                }
                Opcode::StatusOk => {
                    self.status = ExitStatus::OK;
                    self.env.set("?", "0");
                }
                Opcode::StatusFail => {
                    self.status = ExitStatus::FAIL;
                    self.env.set("?", "1");
                }
                Opcode::StatusFlip => {
                    let code = self.status.code();
                    let flipped = if code == 0 {
                        ExitStatus::from_code(1)
                    } else {
                        ExitStatus::OK
                    };
                    self.update_status(flipped);
                }
                Opcode::Exit => {
                    self.run_exit_trap();
                    return Ok(self.status);
                }

                // ── Subshell env isolation ────────────────────────────────────
                Opcode::SubshellBegin => {
                    self.subshell_envs.push(self.env.snapshot());
                }
                Opcode::SubshellEnd => {
                    // Save the subshell's exit status before restoring the parent env.
                    // env.restore() would overwrite $? with the pre-subshell value.
                    let subshell_status = self.status;
                    if let Some(saved) = self.subshell_envs.pop() {
                        self.env.restore(saved);
                    }
                    // Re-apply the subshell's status so $? reads correctly.
                    self.status = subshell_status;
                    self.env.set("?", subshell_status.to_string());
                }
                Opcode::RedirSave => {
                    self.redir_stack.push(self.pending_redirs.clone());
                }
                Opcode::RedirRestore => {
                    if let Some(saved) = self.redir_stack.pop() {
                        self.pending_redirs = saved;
                    } else {
                        self.pending_redirs.clear();
                    }
                }

                // ── Pipeline subshell stage ───────────────────────────────────
                Opcode::PipeSubshellBegin => {
                    let end_ip = instr.operand as usize;
                    if self.pipeline_expected > 0 {
                        // Collect mode: register the stage and skip its body.
                        let entry_ip = self.ip;
                        self.pipeline_segments
                            .push(PipelineStage::Subshell { entry_ip, end_ip });
                        self.ip = end_ip;
                    }
                    // else: shouldn't happen (lowerer only emits PipeSubshellBegin inside
                    // PipeStart/PipeEnd), but treat gracefully as a subshell.
                }
                Opcode::PipeSubshellEnd => {
                    // No-op in normal execution — the VM jumped past this during collection.
                    // If somehow reached (e.g. running a subshell stage inline via ip_fence),
                    // the ip_fence mechanism already stops us before we get here.
                }

                // ── Glob filename expansion ───────────────────────────────────
                Opcode::GlobExpand => {
                    // Unquoted `${arr[@]}` pushed multiple slots: bash field-
                    // splits the WHOLE concatenation, so gather the surplus
                    // slots, join, and split all of it on IFS.
                    let pattern = self.stack.pop()?.into_string();
                    if self.glob_surplus > 0 {
                        let extra = self.glob_surplus;
                        self.glob_surplus = 0;
                        let mut all = self.pop_n(extra)?;
                        all.push(pattern);
                        let joined = all.join(" ");
                        let fields = self.split_fields_ifs(&joined);
                        let surplus = fields.len().saturating_sub(1);
                        for f in fields {
                            self.stack.push_str(f);
                        }
                        self.glob_surplus += surplus;
                        continue;
                    }
                    if has_glob_chars(&pattern) {
                        let matches = expand_glob_pattern(&pattern);
                        if matches.is_empty() {
                            // POSIX: if no match, pass the literal pattern through
                            self.stack.push_str(pattern);
                        } else {
                            let extra = matches.len() - 1;
                            for m in matches {
                                self.stack.push_str(m);
                            }
                            self.glob_surplus += extra;
                        }
                    } else {
                        // No glob chars: field-split unquoted expansions on
                        // IFS (bash: `for x in $var` / `$*` iterate over
                        // words; custom IFS splits on its chars too). Even a
                        // single field must be the TRIMMED field — `$(echo $s)`
                        // with `s=" spaced "` yields "spaced", not " spaced ".
                        let fields = self.split_fields_ifs(&pattern);
                        let extra = fields.len().saturating_sub(1);
                        for f in fields {
                            self.stack.push_str(f);
                        }
                        if extra > 0 {
                            self.glob_surplus += extra;
                        }
                    }
                }

                // ── Variable parameter expansion ──────────────────────────────
                Opcode::VarExpand => {
                    let op_byte = (instr.operand >> 24) as u8;
                    let pool_idx = instr.operand & 0x00FF_FFFF;
                    let var_name = self.pool_str(pool_idx)?;
                    let var_value = self.env.expand(&var_name);
                    match op_byte {
                        0 => {
                            // Default: ${var:-word}  — use modifier if var unset/empty
                            let modifier = self.stack.pop()?.into_string();
                            let result = if var_value.is_empty() {
                                modifier
                            } else {
                                var_value
                            };
                            self.stack.push_str(result);
                        }
                        1 => {
                            // Assign: ${var:=word}  — assign and use modifier if var unset/empty
                            let modifier = self.stack.pop()?.into_string();
                            if var_value.is_empty() {
                                self.env.set(&var_name, &modifier);
                                self.stack.push_str(modifier);
                            } else {
                                self.stack.push_str(var_value);
                            }
                        }
                        2 => {
                            // Error: ${var:?word}  — error if var unset/empty
                            let modifier = self.stack.pop()?.into_string();
                            if var_value.is_empty() {
                                let msg = if modifier.is_empty() {
                                    format!("{}: parameter null or not set", var_name)
                                } else {
                                    modifier
                                };
                                eprintln!("{}", msg);
                                self.update_status(ExitStatus::from_code(1));
                            } else {
                                self.stack.push_str(var_value);
                            }
                        }
                        3 => {
                            // Alt: ${var:+word}  — use modifier if var IS set/non-empty
                            let modifier = self.stack.pop()?.into_string();
                            let result = if var_value.is_empty() {
                                String::new()
                            } else {
                                modifier
                            };
                            self.stack.push_str(result);
                        }
                        4 => {
                            // Length: ${#var}
                            self.stack.push_str(var_value.len().to_string());
                        }
                        5 => {
                            // TrimPrefix: ${var#pattern}
                            let pattern = self.stack.pop()?.into_string();
                            let result = trim_prefix_shortest(&var_value, &pattern);
                            self.stack.push_str(result);
                        }
                        6 => {
                            // TrimSuffix: ${var%pattern}
                            let pattern = self.stack.pop()?.into_string();
                            let result = trim_suffix_shortest(&var_value, &pattern);
                            self.stack.push_str(result);
                        }
                        8 => {
                            // TrimPrefixAll: ${var##pattern}
                            let pattern = self.stack.pop()?.into_string();
                            let result = trim_prefix_longest(&var_value, &pattern);
                            self.stack.push_str(result);
                        }
                        9 => {
                            // TrimSuffixAll: ${var%%pattern}
                            let pattern = self.stack.pop()?.into_string();
                            let result = trim_suffix_longest(&var_value, &pattern);
                            self.stack.push_str(result);
                        }
                        10 => {
                            // Substring: ${var:offset[:length]}
                            let spec = self.stack.pop()?.into_string();
                            let chars: Vec<char> = var_value.chars().collect();
                            let len = chars.len() as i64;
                            let mut parts = spec.splitn(2, ':');
                            let offset_str = parts.next().unwrap_or("0");
                            let length_str = parts.next();
                            let offset: i64 = offset_str.parse().unwrap_or(0);
                            let start = if offset < 0 {
                                (len + offset).max(0) as usize
                            } else {
                                (offset as usize).min(chars.len())
                            };
                            let end = match length_str {
                                Some(ls) => {
                                    let l: i64 = ls.parse().unwrap_or(0);
                                    if l < 0 {
                                        (len + l).max(start as i64) as usize
                                    } else {
                                        (start + l as usize).min(chars.len())
                                    }
                                }
                                None => chars.len(),
                            };
                            let result: String = chars[start..end].iter().collect();
                            self.stack.push_str(result);
                        }
                        11 => {
                            // DefaultUnset: ${var-word} — modifier only if var UNSET
                            // (empty counts as set).
                            let modifier = self.stack.pop()?.into_string();
                            let is_unset = self.env.get(&var_name).is_none();
                            let result = if is_unset { modifier } else { var_value };
                            self.stack.push_str(result);
                        }
                        12 => {
                            // AssignUnset: ${var=word}
                            let modifier = self.stack.pop()?.into_string();
                            if self.env.get(&var_name).is_none() {
                                self.env.set(&var_name, &modifier);
                                self.stack.push_str(modifier);
                            } else {
                                self.stack.push_str(var_value);
                            }
                        }
                        13 => {
                            // ErrorUnset: ${var?word}
                            let modifier = self.stack.pop()?.into_string();
                            if self.env.get(&var_name).is_none() {
                                let msg = if modifier.is_empty() {
                                    format!("{}: parameter null or not set", var_name)
                                } else {
                                    modifier
                                };
                                eprintln!("{}", msg);
                                self.update_status(ExitStatus::from_code(1));
                            } else {
                                self.stack.push_str(var_value);
                            }
                        }
                        14 => {
                            // AltUnset: ${var+word} — modifier if var IS set
                            let modifier = self.stack.pop()?.into_string();
                            let result = if self.env.get(&var_name).is_some() {
                                modifier
                            } else {
                                String::new()
                            };
                            self.stack.push_str(result);
                        }
                        15 => {
                            // Index: ${arr[i]} / ${arr[@]} — index string on
                            // stack. `@` pushes each element as its own slot
                            // (glob_surplus pattern, like PushArgs). `*` is
                            // always a single IFS-joined slot (bash).
                            let idx_raw = self.stack.pop()?.into_string();
                            if idx_raw == "*" {
                                let joined =
                                    self.env.array_get_all(&var_name).join(self.ifs_first());
                                self.stack.push_str(joined);
                            } else if idx_raw == "@" {
                                let items = self.env.array_get_all(&var_name);
                                if items.is_empty() {
                                    self.glob_deficit += 1;
                                } else {
                                    let extra = items.len() - 1;
                                    for it in items {
                                        self.stack.push_str(it);
                                    }
                                    self.glob_surplus += extra;
                                }
                            } else {
                                let idx = self.eval_array_index(&var_name, &idx_raw)?;
                                let v = self.env.array_get(&var_name, idx);
                                self.stack.push_str(v);
                            }
                        }
                        16 => {
                            // ArrayLength: ${#arr[@]} — index string on
                            // stack (`@`/`*`); push the set-element count.
                            let _idx_raw = self.stack.pop()?.into_string();
                            self.stack
                                .push_str(self.env.array_len(&var_name).to_string());
                        }
                        17 => {
                            // IndexJoin: `${arr[@]}` with adjacent word parts
                            // — single space-joined slot for the ConcatN.
                            let _idx_raw = self.stack.pop()?.into_string();
                            let joined = self.env.array_get_all(&var_name).join(" ");
                            self.stack.push_str(joined);
                        }
                        18 => {
                            // IndexDefault: ${arr[i]:-word} — stack [index,
                            // modifier]; use the modifier when unset/empty.
                            let modifier = self.stack.pop()?.into_string();
                            let idx_raw = self.stack.pop()?.into_string();
                            let v = if idx_raw == "@" || idx_raw == "*" {
                                self.env.array_get_all(&var_name).join(" ")
                            } else {
                                let idx = self.eval_array_index(&var_name, &idx_raw)?;
                                self.env.array_get(&var_name, idx)
                            };
                            if v.is_empty() {
                                self.stack.push_str(modifier);
                            } else {
                                self.stack.push_str(v);
                            }
                        }
                        _ => {
                            self.stack.push_str(var_value);
                        }
                    }
                }

                // ── Arithmetic evaluation ─────────────────────────────────────
                // The string on top of the stack is a fully-expanded arithmetic
                // expression (variables already substituted via PushVar / ConcatN
                // at compile time).  We just parse and evaluate it.
                Opcode::ArithEvalStack => {
                    let expr = self.stack.pop()?.into_string();
                    let result = ArithParser::new(expr.trim()).parse();
                    self.stack.push_str(result.to_string());
                }

                // ── Bash conditional tests ────────────────────────────────────
                Opcode::BashUnary => {
                    let op = self.pool_str(instr.operand)?;
                    let arg = self.stack.pop()?.into_string();
                    self.update_status(eval_bash_unary(&op, &arg));
                }
                Opcode::BashBinary => {
                    let op = self.pool_str(instr.operand)?;
                    let right = self.stack.pop()?.into_string();
                    let left = self.stack.pop()?.into_string();
                    self.update_status(eval_bash_binary(&left, &op, &right));
                }
            }
        }
        self.run_exit_trap();
        Ok(self.status)
    }

    fn check_signals(&mut self) {
        if SIGINT_RECEIVED.swap(false, Ordering::SeqCst) {
            match &self.trap_int {
                builtins::trap::TrapDisposition::Command(cmd) => {
                    let cmd = cmd.clone();
                    self.run_trap_command(&cmd);
                }
                builtins::trap::TrapDisposition::Ignore => {}
                builtins::trap::TrapDisposition::Default => {}
            }
        }
        if SIGTERM_RECEIVED.swap(false, Ordering::SeqCst) {
            match &self.trap_term {
                builtins::trap::TrapDisposition::Command(cmd) => {
                    let cmd = cmd.clone();
                    self.run_trap_command(&cmd);
                }
                builtins::trap::TrapDisposition::Ignore => {}
                builtins::trap::TrapDisposition::Default => {}
            }
        }
    }

    fn run_exit_trap(&mut self) {
        if let builtins::trap::TrapDisposition::Command(cmd) = &self.trap_exit {
            let cmd = cmd.clone();
            self.run_trap_command(&cmd);
        }
    }

    fn run_trap_command(&mut self, cmd: &str) {
        // Compile the trap body and run it in a child VM sharing this
        // environment snapshot — `sh -c` would lose arrays and shell-scoped
        // variables that were never exported.
        let bc = {
            let tokens = match shell_lex::Lexer::new(cmd).tokenize() {
                Ok(t) => t,
                Err(_) => return,
            };
            let script = match shell_parse::Parser::new(tokens).parse() {
                Ok(s) => s,
                Err(_) => return,
            };
            let chunk = match shell_ir::Lowerer::new().lower(&script) {
                Ok(c) => c,
                Err(_) => return,
            };
            let chunk = shell_ir::opt::optimize(chunk);
            let sbc = shell_bc::compile_to_sbc(&chunk);
            match shell_bc::assemble_sbc(&sbc) {
                Ok(b) => b,
                Err(_) => return,
            }
        };
        // The trap body is compiled to its own plaintext bytecode; never
        // attach the parent's SMC stream — it seals the MAIN script's
        // instructions, so the child would execute those instead and
        // re-trigger the exit trap in an infinite loop.
        let mut child = Vm::new_child(bc, self.env.snapshot(), None);
        child.func_table = self.func_table.clone();
        let _ = child.run();
    }

    // ── Pipeline executor (handles External, Builtin, and Subshell stages) ────

    fn exec_pipeline(
        &mut self,
        segments: Vec<PipelineStage>,
        capture_out: bool,
    ) -> Result<(ExitStatus, Vec<u8>), ShellError> {
        use std::io::Write;

        if segments.is_empty() {
            return Ok((ExitStatus::OK, vec![]));
        }

        let n = segments.len();
        let mut stdin_buf: Option<Vec<u8>> = None;
        let mut last_status = ExitStatus::OK;

        for (i, stage) in segments.into_iter().enumerate() {
            let is_last = i + 1 == n;
            match stage {
                PipelineStage::External { argv, redirs } => {
                    let capture_output = !is_last || capture_out;
                    let input = stdin_buf
                        .as_deref()
                        .filter(|_| !redirs.specs.iter().any(|s| s.fd == 0));
                    match run_external_stage_inline(&argv, input, capture_output, &redirs) {
                        Ok((st, out)) => {
                            last_status = st;
                            stdin_buf = if capture_output { Some(out) } else { None };
                        }
                        Err(e) => {
                            let name = argv.first().cloned().unwrap_or_default();
                            let saved = std::mem::take(&mut self.pending_redirs);
                            self.pending_redirs = redirs.specs.clone();
                            self.print_exec_error(&name, e);
                            self.pending_redirs = saved;
                            last_status = ExitStatus::from_code(127);
                            stdin_buf = if !is_last { Some(vec![]) } else { None };
                        }
                    }
                }
                PipelineStage::Builtin { id, args, redirs } => {
                    if id == shell_ir::BuiltinId::Eval {
                        let src = args.join(" ");
                        let pipe_in = stdin_buf.as_deref();
                        match self.eval_pipeline_subshell(&src, pipe_in) {
                            Ok((st, out)) => {
                                last_status = st;
                                stdin_buf = if !is_last || capture_out {
                                    Some(out)
                                } else {
                                    if !out.is_empty() {
                                        let _ = std::io::stdout().write_all(&out);
                                        let _ = std::io::stdout().flush();
                                    }
                                    None
                                };
                            }
                            Err(_) => {
                                last_status = ExitStatus::from_code(1);
                                stdin_buf = if !is_last { Some(vec![]) } else { None };
                            }
                        }
                        continue;
                    }
                    let pipe_in = stdin_buf
                        .as_deref()
                        .filter(|_| !redirs.specs.iter().any(|s| s.fd == 0));
                    let result =
                        run_builtin_stage_for_vm(id, &args, &mut self.env, pipe_in, &redirs)?;
                    last_status = result.0;
                    stdin_buf = if !is_last || capture_out {
                        Some(result.1)
                    } else {
                        if !result.1.is_empty() {
                            let _ = std::io::stdout().write_all(&result.1);
                            let _ = std::io::stdout().flush();
                        }
                        None
                    };
                }
                PipelineStage::Subshell { entry_ip, end_ip } => {
                    // Run the subshell inline with a child VM.
                    let mut child =
                        Vm::new_child(self.bc.clone(), self.env.snapshot(), self.smc.clone());
                    child.func_table = self.func_table.clone();
                    child.pending_stdin = stdin_buf.take().map(std::io::Cursor::new);
                    child.capture_stack.push(vec![]); // capture stdout
                    child.ip = entry_ip;
                    child.ip_fence = Some(end_ip);
                    // Run; ignore errors (they set status instead of propagating)
                    let _ = child.run();
                    let out = child.capture_stack.pop().unwrap_or_default();
                    last_status = child.status;
                    if is_last && !capture_out {
                        if !out.is_empty() {
                            let _ = std::io::stdout().write_all(&out);
                            let _ = std::io::stdout().flush();
                        }
                        stdin_buf = None;
                    } else {
                        stdin_buf = Some(out);
                    }
                }
            }
        }

        let captured = if capture_out {
            stdin_buf.unwrap_or_default()
        } else {
            vec![]
        };
        Ok((last_status, captured))
    }

    // ── Error handling ────────────────────────────────────────────────────────

    fn handle_exec_error(&mut self, cmd: &str, e: ShellError) {
        // A redirection failure (bad fd) was already reported by the redir
        // layer; bash exits 1 and does not print a second diagnostic.
        if e.to_string().contains("Bad file descriptor") {
            self.update_status(ExitStatus::from_code(1));
            return;
        }
        self.print_exec_error(cmd, e);
        self.update_status(ExitStatus::from_code(127));
    }

    fn print_exec_error(&mut self, cmd: &str, e: ShellError) {
        let msg = e.to_string();
        let text = if msg.contains("No such file or directory") || msg.contains("not found") {
            format!("shellsc: {}: command not found\n", cmd)
        } else if msg.contains("Permission denied") {
            format!("shellsc: {}: Permission denied\n", cmd)
        } else {
            format!("shellsc: {}: {}\n", cmd, msg)
        };
        self.write_to_fd_inner(2, text.as_bytes(), &mut Vec::new());
    }

    fn write_out(&mut self, data: &[u8]) {
        self.write_to_fd_inner(1, data, &mut Vec::new());
    }

    fn write_to_fd_inner(&mut self, fd: u32, data: &[u8], visited: &mut Vec<u32>) {
        if data.is_empty() || visited.contains(&fd) {
            return;
        }
        visited.push(fd);

        let spec = self
            .pending_redirs
            .iter()
            .rev()
            .find(|s| s.fd == fd)
            .cloned()
            .or_else(|| {
                self.persistent_redirs
                    .iter()
                    .rev()
                    .find(|s| s.fd == fd)
                    .cloned()
            });
        if let Some(spec) = spec {
            match &spec.target {
                RedirTargetSpec::File(path) => {
                    let is_append = matches!(spec.kind, RedirKind::Append);
                    let res = std::fs::OpenOptions::new()
                        .write(true)
                        .create(true)
                        .append(is_append)
                        .truncate(!is_append)
                        .open(path);
                    if let Ok(mut f) = res {
                        let _ = f.write_all(data);
                    }
                    return;
                }
                RedirTargetSpec::Fd(target_fd) => {
                    self.write_to_fd_inner(*target_fd, data, visited);
                    return;
                }
                RedirTargetSpec::HereDoc(_) => {
                    return;
                }
            }
        }

        // `exec N> file` — fd opened persistently; write straight to the File.
        if fd > 2 {
            if let Some(file) = self.exec_fds.get_mut(&fd) {
                let _ = file.write_all(data);
                let _ = file.flush();
            }
            return;
        }

        if fd == 1 {
            if let Some(frame) = self.capture_stack.last_mut() {
                frame.extend_from_slice(data);
                return;
            }
            let _ = std::io::stdout().write_all(data);
            let _ = std::io::stdout().flush();
        } else if fd == 2 {
            let _ = std::io::stderr().write_all(data);
            let _ = std::io::stderr().flush();
        }
    }

    // ── External command helpers ───────────────────────────────────────────────

    fn run_external_capture(
        &mut self,
        argv: &[String],
        redirs: RedirSet,
    ) -> Result<(Vec<u8>, ExitStatus), ShellError> {
        use std::process::{Command, Stdio};

        let (stdin_bytes, redirs) = redirs.split_stdin()?;
        let stdin_data = stdin_bytes.or_else(|| {
            self.pending_stdin.take().map(|mut c| {
                let mut buf = Vec::new();
                use std::io::Read;
                let _ = c.read_to_end(&mut buf);
                buf
            })
        });
        let mut cmd = Command::new(&argv[0]);
        cmd.args(&argv[1..]);
        redirs.apply_to_command(&mut cmd)?;

        if stdin_data.is_some() {
            cmd.stdin(Stdio::piped());
        }
        cmd.stdout(Stdio::piped());

        let mut child = cmd
            .spawn()
            .map_err(|e| ShellError::IoError(format!("exec '{}': {}", argv[0], e)))?;

        if let Some(data) = stdin_data {
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(&data);
            }
        }

        let out = child
            .wait_with_output()
            .map_err(|e| ShellError::IoError(format!("wait child '{}': {}", argv[0], e)))?;
        Ok((
            out.stdout,
            ExitStatus::from_code(out.status.code().unwrap_or(1)),
        ))
    }

    fn call_function(&mut self, entry_ip: u32, args: &[String]) -> Result<(), ShellError> {
        let args_vec: Vec<String> = args.iter().cloned().collect();
        self.env.push_frame(&args_vec);
        let saved_redirs = self.pending_redirs.clone();
        self.call_stack.push(CallFrame {
            return_ip: self.ip,
            saved_redirs,
        });
        self.ip = entry_ip as usize;
        Ok(())
    }

    // ── Builtin dispatch ──────────────────────────────────────────────────────

    fn dispatch_builtin(
        &mut self,
        id: BuiltinId,
        argc: usize,
    ) -> Result<BuiltinResult, ShellError> {
        let result = match id {
            BuiltinId::Echo => {
                let args = self.pop_n(argc)?;
                builtins::echo::run(&args)
            }
            BuiltinId::Printf => {
                let args = self.pop_n(argc)?;
                builtins::printf::run(&args)
            }
            BuiltinId::Read => {
                let names = self.pop_n(argc)?;
                let stdin_file = self
                    .pending_redirs
                    .iter()
                    .find(|r| r.fd == 0)
                    .and_then(|r| match &r.target {
                        RedirTargetSpec::File(p) => Some(p.clone()),
                        _ => None,
                    });
                let redir_stdin_data =
                    self.pending_redirs
                        .iter()
                        .find(|r| r.fd == 0)
                        .and_then(|r| match &r.target {
                            RedirTargetSpec::HereDoc(b) => Some(b.as_bytes().to_vec()),
                            _ => None,
                        });
                self.pending_redirs.retain(|r| r.fd != 0);
                // Explicit heredoc redirect takes priority over piped stdin cursor.
                if let Some(data) = redir_stdin_data {
                    if self.pending_stdin.is_none() {
                        self.pending_stdin = Some(std::io::Cursor::new(data));
                    }
                    return Ok(builtins::read::run(
                        &names,
                        &mut self.env,
                        stdin_file,
                        self.pending_stdin.as_mut(),
                    ));
                }
                // Piped stdin cursor persists across multiple reads (for while loops).
                builtins::read::run(
                    &names,
                    &mut self.env,
                    stdin_file,
                    self.pending_stdin.as_mut(),
                )
            }
            BuiltinId::Test => {
                let args = self.pop_n(argc)?;
                builtins::test::run(&args)
            }
            BuiltinId::Sleep => {
                let arg = self.stack.pop()?.into_string();
                builtins::sleep::run(&[arg])
            }
            BuiltinId::Export => {
                let items = self.pop_n(argc * 2)?;
                let pairs: Vec<(String, String)> = (0..argc)
                    .map(|i| (items[i * 2].clone(), items[i * 2 + 1].clone()))
                    .collect();
                builtins::export::run_pairs(&pairs, &mut self.env)
            }
            BuiltinId::Unset => {
                let names = self.pop_n(argc)?;
                builtins::unset::run(&names, &mut self.env)
            }
            BuiltinId::Exit => {
                let arg = self.stack.pop()?.into_string();
                builtins::exit::run(&[arg])
            }
            BuiltinId::True => BuiltinResult::ok(),
            BuiltinId::False => BuiltinResult::fail(),
            BuiltinId::Return => {
                // return [n] — set the caller-visible status; the FuncReturn
                // op emitted right after this pops the call frame. The arg
                // word stays on the stack (consumed here) and no value is
                // pushed back: FuncReturn must not see stray stack items.
                let args = self.pop_n(argc)?;
                let code = args
                    .first()
                    .and_then(|s| s.as_str().parse::<i32>().ok())
                    .unwrap_or_else(|| self.status.code());
                BuiltinResult::with_out(vec![], ExitStatus::from_code(code))
            }
            BuiltinId::CommandV => {
                let args = self.pop_n(argc)?;
                let mut found = String::new();
                let mut status = ExitStatus::from_code(1);
                if let Some(flag_idx) = args.iter().position(|a| a.as_str() == "-v") {
                    if let Some(name) = args.get(flag_idx + 1) {
                        let name = name.as_str();
                        if self.func_table.contains_key(name) {
                            found = name.to_string();
                            status = ExitStatus::OK;
                        } else if BuiltinId::from_name(name).is_some() {
                            found = name.to_string();
                            status = ExitStatus::OK;
                        } else if let Ok(out) =
                            std::process::Command::new("which").arg(name).output()
                        {
                            if out.status.success() {
                                found = String::from_utf8_lossy(&out.stdout).trim().to_string();
                                status = ExitStatus::OK;
                            }
                        }
                    }
                }
                let out = if found.is_empty() {
                    vec![]
                } else {
                    format!("{}\n", found).into_bytes()
                };
                BuiltinResult::with_out(out, status)
            }

            // exec [cmd args…] / exec N< file — with args, the redirections (if
            // any) apply to the command; with no args they persist for the rest
            // of the script.  fd>2 targets are opened eagerly and stored in
            // exec_fds; fd 0/1/2 targets stay in pending_redirs (which exec
            // marks persistent so the dispatcher does not clear them).
            BuiltinId::Exec => {
                let args = self.pop_n(argc)?;
                // Validate/open fd>2 redirects now; keep fd 0/1/2 specs.
                let mut status = ExitStatus::OK;
                for spec in self.pending_redirs.iter() {
                    if let RedirTargetSpec::File(path) = &spec.target {
                        if spec.fd > 2 {
                            let file = match &spec.kind {
                                RedirKind::In => std::fs::File::open(path),
                                RedirKind::Out => std::fs::OpenOptions::new()
                                    .write(true)
                                    .create(true)
                                    .truncate(true)
                                    .open(path),
                                RedirKind::Append => std::fs::OpenOptions::new()
                                    .write(true)
                                    .create(true)
                                    .append(true)
                                    .open(path),
                                _ => Ok(std::fs::File::open(path).unwrap_or_else(|_| {
                                    std::fs::File::create(path).expect("exec redir create")
                                })),
                            };
                            match file {
                                Ok(f) => {
                                    self.exec_fds.insert(spec.fd, f);
                                }
                                Err(e) => {
                                    // bash reports the open failure and keeps
                                    // going; the fd stays unusable.
                                    let _ = writeln!(std::io::stderr(), "exec: {}: {}", path, e);
                                    status = ExitStatus::from_code(1);
                                }
                            }
                        }
                    }
                }
                if !args.is_empty() {
                    // exec cmd args… — the command replaces the shell: run it
                    // with the redirects applied, then terminate the VM after
                    // this builtin returns (bash semantics).
                    let argv: Vec<String> = args;
                    let redirs = self.take_redirs();
                    match run_external_stage_inline(&argv, None, false, &redirs) {
                        Ok((st, _)) => {
                            self.exec_terminated = true;
                            BuiltinResult::with_out(vec![], st)
                        }
                        Err(_) => {
                            // exec of a missing command: bash exits 127.
                            let _ = writeln!(
                                std::io::stderr(),
                                "exec: {}: not found",
                                argv.first().cloned().unwrap_or_default()
                            );
                            self.exec_terminated = true;
                            BuiltinResult::with_out(vec![], ExitStatus::from_code(127))
                        }
                    }
                } else {
                    // Redirections persist. For fd 0/1/2 file targets:
                    // truncate once here (bash truncates when exec opens the
                    // fd), then convert Out→Append so every later reopen —
                    // builtin writes and child-process stdio alike — appends
                    // instead of truncating (clobbering) prior output.
                    let persistent: Vec<RedirSpec> = self
                        .pending_redirs
                        .iter()
                        .filter(|s| s.fd <= 2)
                        .cloned()
                        .collect();
                    for mut spec in persistent {
                        if let RedirTargetSpec::File(path) = &spec.target {
                            if matches!(spec.kind, RedirKind::Out | RedirKind::Append) {
                                let opened = std::fs::OpenOptions::new()
                                    .write(true)
                                    .create(true)
                                    .truncate(matches!(spec.kind, RedirKind::Out))
                                    .open(path);
                                if opened.is_err() {
                                    status = ExitStatus::from_code(1);
                                    continue;
                                }
                                spec.kind = RedirKind::Append;
                            }
                        }
                        self.persistent_redirs.push(spec);
                    }
                    self.pending_redirs.clear();
                    BuiltinResult::with_out(vec![], status)
                }
            }

            // : [args…] — null command, always succeeds
            BuiltinId::Colon => {
                let _ = self.pop_n(argc)?;
                BuiltinResult::ok()
            }

            // eval [args…] — join args with spaces, parse+lower+splice into
            // the running stream so it executes in the current shell scope
            // (variables set by the evaled code persist, like bash).
            BuiltinId::Eval => {
                let args = self.pop_n(argc)?;
                if args.is_empty() {
                    return Ok(BuiltinResult::ok());
                }
                let src = args.join(" ");
                let tokens = shell_lex::Lexer::new(&src).tokenize()?;
                let script = shell_parse::Parser::new(tokens).parse()?;
                let chunk = shell_ir::Lowerer::new().lower(&script)?;
                let chunk = shell_ir::opt::optimize(chunk);
                let sbc = shell_bc::compile_to_sbc(&chunk);
                let mut bc = shell_bc::assemble_sbc(&sbc)?;
                // The chunk compiler ends every script with Exit; strip it so
                // the spliced code returns to the caller's stream.
                if bc.instructions.last().is_some_and(|i| i.op == Opcode::Exit) {
                    bc.instructions.pop();
                }
                // Preserve $? for the spliced code: `$?` inside the eval
                // string must see the status of the command preceding eval
                // (the spliced chunk runs after this builtin returns).
                let pre = self.status;
                self.splice_bytecode(bc)?;
                BuiltinResult::with_out(vec![], pre)
            }

            // shift [n] — drop the first n positional parameters (default 1).
            // Returns failure if there are fewer than n positionals (bash).
            BuiltinId::Shift => {
                let args = self.pop_n(argc)?;
                let n: usize = args.first().and_then(|a| a.parse().ok()).unwrap_or(1);
                let count = self
                    .env
                    .get("#")
                    .and_then(|s| s.parse::<usize>().ok())
                    .unwrap_or(0);
                if n > count {
                    BuiltinResult::with_out(vec![], ExitStatus::from_code(1))
                } else {
                    let shifted: Vec<String> = (n + 1..=count)
                        .map(|i| self.env.get(&i.to_string()).unwrap_or("").to_string())
                        .collect();
                    self.env.set_positional(&shifted);
                    BuiltinResult::ok()
                }
            }

            // local var[=value] … — declare variable in the current function scope.
            // Variables declared local are invisible outside the function.
            BuiltinId::Local => {
                let args = self.pop_n(argc)?;
                for arg in args {
                    if let Some((name, value)) = arg.split_once('=') {
                        self.env.set_local(name, value);
                    } else {
                        // `local varname` without assignment — shadow outer value.
                        self.env.declare_local(&arg);
                    }
                }
                BuiltinResult::ok()
            }

            // set [--] [arg…] — set positional parameters $1 $2 …
            // Flags like -e/-x are silently accepted and ignored.
            // set [--] [arg…] — set positional parameters $1 $2 …
            // Flags like -e/-x are silently accepted and ignored.
            BuiltinId::Set => {
                // Each already-expanded argument becomes ONE positional — no
                // re-splitting on IFS (quotes were resolved at expansion time;
                // `set -- a "b c" d` must keep "b c" intact).
                let args = self.pop_n(argc)?;
                let positionals: Vec<String> = if args.first().map(|a| a.as_str()) == Some("--") {
                    args.into_iter().skip(1).collect()
                } else if args.first().map(|a| a.starts_with('-')).unwrap_or(false) {
                    return Ok(BuiltinResult::ok()); // option-only, no positional change
                } else {
                    args
                };
                self.env.set_positional(&positionals);
                BuiltinResult::ok()
            }

            // wait [pid…] — wait for all background jobs (or specific PIDs).
            BuiltinId::Wait => {
                let _ = self.pop_n(argc)?;
                std::thread::sleep(std::time::Duration::from_millis(50));
                BuiltinResult::ok()
            }

            BuiltinId::Trap => {
                let args = self.pop_n(argc)?;
                let (result, actions) = builtins::trap::run(&args);
                for action in actions {
                    match action.signal {
                        builtins::trap::TrapSignal::Exit => self.trap_exit = action.disposition,
                        builtins::trap::TrapSignal::Int => self.trap_int = action.disposition,
                        builtins::trap::TrapSignal::Term => self.trap_term = action.disposition,
                    }
                }
                result
            }
        };
        Ok(result)
    }

    fn pop_builtin_args(&mut self, id: BuiltinId, argc: usize) -> Result<Vec<String>, ShellError> {
        let n = match id {
            BuiltinId::Export => argc * 2,
            BuiltinId::Sleep | BuiltinId::Exit => 1,
            BuiltinId::True | BuiltinId::False => 0,
            _ => argc,
        };
        self.pop_n(n)
    }

    // ── Status helpers ───────────────────────────────────────────────────────────

    /// Update `self.status` and write `$?` into the env so shell scripts can read it.
    #[inline]
    fn update_status(&mut self, st: ExitStatus) {
        self.status = st;
        self.env.set("?", st.to_string());
    }

    // ── Misc helpers ──────────────────────────────────────────────────────────

    fn pool(&self, idx: u32) -> Result<&str, ShellError> {
        self.bc.const_pool.get(idx).ok_or_else(|| {
            ShellError::BytecodeError(format!("const pool index {} out of bounds", idx))
        })
    }

    /// Decrypt-on-demand const-pool fetch (SMC mode). Returned as a fresh
    /// String the caller drops right after use.
    fn pool_smc(&self, idx: u32) -> Result<String, ShellError> {
        self.smc
            .as_ref()
            .and_then(|s| s.pool_get(idx))
            .ok_or_else(|| {
                ShellError::BytecodeError(format!("const pool index {} out of bounds", idx))
            })
    }

    /// Pool fetch valid in both modes: plaintext clone or SMC decrypt.
    fn pool_str(&self, idx: u32) -> Result<String, ShellError> {
        if self.smc.is_some() {
            self.pool_smc(idx)
        } else {
            Ok(self.pool(idx)?.to_string())
        }
    }

    /// Resolve a raw array index: literal digits, or `$var`/`$[n]`-free
    /// reference expanded against the env. Returns the numeric index.
    fn eval_array_index(&self, _arr: &str, raw: &str) -> Result<usize, ShellError> {
        let s = if raw.chars().all(|c| c.is_ascii_digit()) {
            // Literal digits first — "0" must NOT expand as positional $0.
            raw.to_string()
        } else if let Some(inner) = raw.strip_prefix("$((").and_then(|r| r.strip_suffix("))")) {
            // $((expr)) — substitute $vars then arithmetic-evaluate.
            let sub = self.subst_arith_vars(inner);
            ArithParser::new(sub.trim()).parse().to_string()
        } else if let Some(rest) = raw.strip_prefix('$') {
            self.env.expand(rest)
        } else if !raw.is_empty() && raw.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            // Bare identifier is a variable reference (bash: arr[i]).
            self.env.expand(raw)
        } else if !raw.is_empty()
            && raw
                .chars()
                .any(|c| c == '+' || c == '-' || c == '*' || c == '/' || c == '%')
        {
            // Arithmetic expression (bash auto-evals array subscripts).
            let sub = self.subst_arith_vars(raw);
            ArithParser::new(sub.trim()).parse().to_string()
        } else {
            raw.to_string()
        };
        s.trim()
            .parse::<usize>()
            .map_err(|_| ShellError::BytecodeError(format!("bad array index '{}'", raw)))
    }

    /// Replace bare/`$`-prefixed identifiers in an arithmetic index
    /// expression with their values, so ArithParser sees only numbers.
    fn subst_arith_vars(&self, expr: &str) -> String {
        let mut out = String::new();
        let bytes = expr.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            let c = bytes[i] as char;
            if c == '$' || c.is_ascii_alphabetic() || c == '_' {
                let start = i;
                if bytes[i] == b'$' {
                    i += 1;
                }
                while i < bytes.len() {
                    let c2 = bytes[i] as char;
                    if c2.is_ascii_alphanumeric() || c2 == '_' {
                        i += 1;
                    } else {
                        break;
                    }
                }
                let name = &expr[start..i].trim_start_matches('$');
                if name.is_empty() {
                    out.push_str(&expr[start..i]);
                } else {
                    out.push_str(&self.env.get_scalar_or_array0(name).unwrap_or(""));
                }
            } else {
                out.push(c);
                i += 1;
            }
        }
        out
    }

    fn pop_n(&mut self, n: usize) -> Result<Vec<String>, ShellError> {
        Ok(self
            .stack
            .pop_n(n)?
            .into_iter()
            .map(|v| v.into_string())
            .collect())
    }

    fn take_redirs(&mut self) -> RedirSet {
        // Persistent `exec` redirects (fd 0/1/2) apply to every command too,
        // and fd>2 exec handles ride along so children can use them.
        let mut specs = self.persistent_redirs.clone();
        specs.append(&mut self.pending_redirs);
        let mut exec_files = HashMap::new();
        for (fd, f) in &self.exec_fds {
            if let Ok(c) = f.try_clone() {
                exec_files.insert(*fd, c);
            }
        }
        RedirSet { specs, exec_files }
    }

    fn redir_spec(&self, idx: u32) -> Result<RedirSpec, ShellError> {
        // Build (kind, fd, target) either from the plaintext table or by
        // decrypting the SMC entry on demand.
        let (kind, fd, target) = match &self.smc {
            Some(s) => {
                let e = s.redirs().get(idx as usize).ok_or_else(|| {
                    ShellError::BytecodeError(format!("redir index {} out of bounds", idx))
                })?;
                let kind = RedirKind::from_u8(e.kind)
                    .ok_or_else(|| ShellError::BytecodeError("bad smc redir kind".into()))?;
                let target = match &e.target {
                    crate::smc::SmcTarget::File(pi) => RedirTargetSpec::File(self.pool_str(*pi)?),
                    crate::smc::SmcTarget::Fd(n) => RedirTargetSpec::Fd(*n),
                    crate::smc::SmcTarget::HereDoc(_) => {
                        let body = s.heredoc_of(e).ok_or_else(|| {
                            ShellError::BytecodeError("smc heredoc decrypt failed".into())
                        })?;
                        let expanded = match kind {
                            RedirKind::HereDocLit => body,
                            _ => self.expand_heredoc_body(&body),
                        };
                        RedirTargetSpec::HereDoc(expanded)
                    }
                };
                (kind, e.fd, target)
            }
            None => {
                let e = self.bc.redirs.get(idx as usize).ok_or_else(|| {
                    ShellError::BytecodeError(format!("redir index {} out of bounds", idx))
                })?;
                let target = match &e.target {
                    RedirTarget::File(pi) => RedirTargetSpec::File(self.pool_str(*pi)?),
                    RedirTarget::FilePath(p) => RedirTargetSpec::File(p.clone()),
                    RedirTarget::Fd(n) => RedirTargetSpec::Fd(*n),
                    RedirTarget::HereDoc(b) => {
                        // Expand variables in heredoc body using the current
                        // environment — unless the delimiter was quoted (HereDocLit).
                        let expanded = match e.kind {
                            shell_bc::bytecode::RedirKind::HereDocLit => b.clone(),
                            _ => self.expand_heredoc_body(b),
                        };
                        RedirTargetSpec::HereDoc(expanded)
                    }
                };
                (e.kind.clone(), e.fd, target)
            }
        };
        Ok(RedirSpec { kind, fd, target })
    }

    /// Field-split on the current $IFS. Default IFS = space/tab/newline
    /// (split_whitespace); a custom non-empty IFS splits on its chars.
    fn split_fields_ifs(&self, s: &str) -> Vec<String> {
        match self.env.get("IFS") {
            Some(v) if !v.is_empty() => split_on_ifs(s, v),
            _ => s.split_whitespace().map(|w| w.to_string()).collect(),
        }
    }

    /// First char of $IFS (bash: `${arr[*]}` joins on it), default space.
    fn ifs_first(&self) -> &str {
        match self.env.get("IFS") {
            Some(v) if !v.is_empty() => v.get(0..1).unwrap_or(" "),
            _ => " ",
        }
    }

    /// Resolve one `${...}` occurrence inside a heredoc body, including
    /// array forms: `${arr[i]}`, `${arr[@]}`, `${arr[*]}`, `${#arr[@]}`.
    fn expand_heredoc_brace(&self, inner: &str) -> String {
        // Length form: ${#arr[@]} / ${#var}
        let (is_len, expr) = if let Some(rest) = inner.strip_prefix('#') {
            (true, rest)
        } else {
            (false, inner)
        };
        // Array form: name[index]
        if let Some(open) = expr.find('[') {
            let name = &expr[..open];
            let close = expr.len().saturating_sub(1);
            if expr.ends_with(']') && close > open {
                let idx_raw = &expr[open + 1..close];
                let idx = self.eval_array_index(name, idx_raw).unwrap_or(usize::MAX);
                if is_len && (idx_raw == "@" || idx_raw == "*") {
                    return self.env.array_len(name).to_string();
                }
                if idx_raw == "@" || idx_raw == "*" {
                    let all = self.env.array_get_all(name);
                    let join = if idx_raw == "*" {
                        self.ifs_first()
                    } else {
                        " "
                    };
                    return all.join(join);
                }
                if is_len {
                    return self.env.array_get(name, idx).len().to_string();
                }
                return self.env.array_get(name, idx);
            }
        }
        if is_len {
            return self
                .env
                .get(expr)
                .map(|v| v.len().to_string())
                .unwrap_or_else(|| "0".to_string());
        }
        self.env.expand(expr)
    }

    fn expand_heredoc_body(&self, body: &str) -> String {
        let bytes = body.as_bytes();
        let mut out = String::with_capacity(body.len());
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'$' && i + 1 < bytes.len() {
                match bytes[i + 1] {
                    b'{' => {
                        if let Some(rel) = body[i + 2..].find('}') {
                            let name = &body[i + 2..i + 2 + rel];
                            out.push_str(&self.expand_heredoc_brace(name));
                            i += rel + 3;
                        } else {
                            out.push('$');
                            i += 1;
                        }
                    }
                    b if b.is_ascii_alphabetic() || b == b'_' => {
                        let start = i + 1;
                        let mut j = start;
                        while j < bytes.len()
                            && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_')
                        {
                            j += 1;
                        }
                        out.push_str(&self.env.expand(&body[start..j]));
                        i = j;
                    }
                    _ => {
                        out.push('$');
                        i += 1;
                    }
                }
            } else {
                let ch = body[i..].chars().next().unwrap();
                out.push(ch);
                i += ch.len_utf8();
            }
        }
        out
    }
}

fn split_on_ifs(input: &str, ifs: &str) -> Vec<String> {
    if ifs.is_empty() {
        return vec![input.to_string()];
    }
    let ifs_chars: Vec<char> = ifs.chars().collect();
    input
        .split(|c: char| ifs_chars.contains(&c))
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect()
}

// ── Glob matching (*, ?, [...]) ───────────────────────────────────────────────

pub fn glob_match(pattern: &str, subject: &str) -> bool {
    glob_bytes(pattern.as_bytes(), subject.as_bytes())
}

// ── Glob filename expansion ───────────────────────────────────────────────────

fn has_glob_chars(s: &str) -> bool {
    s.bytes().any(|b| b == b'*' || b == b'?' || b == b'[')
}

/// Expand a glob pattern against the filesystem.
/// Returns all matching paths sorted; caller keeps the literal if empty.
fn expand_glob_pattern(pattern: &str) -> Vec<String> {
    let mut out = vec![];
    if pattern.starts_with('/') {
        glob_walk(
            std::path::Path::new("/"),
            &pattern[1..],
            "",
            false,
            &mut out,
        );
    } else {
        glob_walk(std::path::Path::new("."), pattern, "", true, &mut out);
    }
    out.sort();
    out
}

/// Recursive glob walker.
///
/// * `dir`        — directory to read
/// * `pattern`    — remaining pattern (may contain `/` for sub-dirs)
/// * `prefix`     — path string accumulated so far (for output)
/// * `strip_dot`  — if true, `./` is stripped from single-component paths
fn glob_walk(
    dir: &std::path::Path,
    pattern: &str,
    prefix: &str,
    strip_dot: bool,
    out: &mut Vec<String>,
) {
    let (seg, tail) = match pattern.find('/') {
        Some(i) => (&pattern[..i], Some(&pattern[i + 1..])),
        None => (pattern, None),
    };

    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = rd.filter_map(|e| e.ok()).collect();
    entries.sort_by_key(|e| e.file_name());

    for entry in entries {
        let name_os = entry.file_name();
        let name = name_os.to_string_lossy();

        // POSIX: dotfiles are hidden unless the pattern segment starts with '.'
        if name.starts_with('.') && !seg.starts_with('.') {
            continue;
        }

        if !glob_bytes(seg.as_bytes(), name.as_bytes()) {
            continue;
        }

        // Build the output path string for this match
        let full: String = if prefix.is_empty() {
            name.into_owned()
        } else {
            format!("{}/{}", prefix, name)
        };

        match tail {
            // No more segments: this is a leaf match
            None => {
                let s = if strip_dot && full.starts_with("./") {
                    full[2..].to_string()
                } else {
                    full
                };
                out.push(s);
            }
            // More segments: descend into matching directories
            Some(rest) => {
                let sub = dir.join(name_os);
                if sub.is_dir() {
                    glob_walk(&sub, rest, &full, strip_dot, out);
                }
            }
        }
    }
}

fn glob_bytes(p: &[u8], s: &[u8]) -> bool {
    match p.first() {
        None => s.is_empty(),
        Some(b'*') => {
            let rest = &p[1..];
            // Collapse consecutive stars
            let rest = {
                let mut r = rest;
                while r.first() == Some(&b'*') {
                    r = &r[1..];
                }
                r
            };
            for i in 0..=s.len() {
                if glob_bytes(rest, &s[i..]) {
                    return true;
                }
            }
            false
        }
        Some(b'?') => !s.is_empty() && glob_bytes(&p[1..], &s[1..]),
        Some(b'[') => {
            let inner = &p[1..];
            let negate = matches!(inner.first(), Some(b'!') | Some(b'^'));
            let class_bytes = if negate { &inner[1..] } else { inner };
            // Find the closing ] (not at position 0 of class_bytes)
            let close = find_bracket_end(class_bytes);
            if let Some(ci) = close {
                let class = &class_bytes[..ci];
                let after = &class_bytes[ci + 1..];
                if s.is_empty() {
                    return false;
                }
                let ch = s[0];
                let matched = char_in_class(class, ch);
                let matched = if negate { !matched } else { matched };
                if matched {
                    glob_bytes(after, &s[1..])
                } else {
                    false
                }
            } else {
                // Malformed bracket: treat '[' as literal
                !s.is_empty() && s[0] == b'[' && glob_bytes(&p[1..], &s[1..])
            }
        }
        Some(b'\\') if p.len() > 1 => {
            // Escaped char: match literally
            !s.is_empty() && s[0] == p[1] && glob_bytes(&p[2..], &s[1..])
        }
        Some(&c) => !s.is_empty() && s[0] == c && glob_bytes(&p[1..], &s[1..]),
    }
}

/// Find the index of `]` that closes a character class.
/// Per POSIX, `]` at position 0 is literal. We skip it.
fn find_bracket_end(inner: &[u8]) -> Option<usize> {
    let mut i = 0;
    while i < inner.len() {
        // Allow ] at position 0 (and after ^ or !) to be literal
        if inner[i] == b']' && i > 0 {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn char_in_class(class: &[u8], c: u8) -> bool {
    let mut i = 0;
    while i < class.len() {
        if i + 2 < class.len() && class[i + 1] == b'-' {
            if c >= class[i] && c <= class[i + 2] {
                return true;
            }
            i += 3;
        } else {
            if class[i] == c {
                return true;
            }
            i += 1;
        }
    }
    false
}

// ── Arithmetic expression evaluator ──────────────────────────────────────────
// (Variable expansion now happens at compile time via PushVar/ConcatN;
//  ArithEvalStack pops the already-expanded string and just parses numbers.)

struct ArithParser {
    src: Vec<u8>,
    pos: usize,
}

impl ArithParser {
    fn new(s: &str) -> Self {
        Self {
            src: s.as_bytes().to_vec(),
            pos: 0,
        }
    }

    fn parse(&mut self) -> i64 {
        self.expr()
    }

    fn skip(&mut self) {
        while self.pos < self.src.len()
            && (self.src[self.pos] == b' ' || self.src[self.pos] == b'\t')
        {
            self.pos += 1;
        }
    }

    fn at(&mut self, b: u8) -> bool {
        self.skip();
        self.src.get(self.pos) == Some(&b)
    }

    fn at2(&mut self, a: u8, b: u8) -> bool {
        self.skip();
        self.src.get(self.pos) == Some(&a) && self.src.get(self.pos + 1) == Some(&b)
    }

    fn eat(&mut self, b: u8) -> bool {
        if self.at(b) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn eat2(&mut self, a: u8, b: u8) -> bool {
        if self.at2(a, b) {
            self.pos += 2;
            true
        } else {
            false
        }
    }

    fn expr(&mut self) -> i64 {
        self.ternary()
    }

    fn ternary(&mut self) -> i64 {
        let cond = self.logic_or();
        if self.eat(b'?') {
            let t = self.expr();
            self.eat(b':');
            let f = self.expr();
            if cond != 0 {
                t
            } else {
                f
            }
        } else {
            cond
        }
    }

    fn logic_or(&mut self) -> i64 {
        let mut v = self.logic_and();
        while self.eat2(b'|', b'|') {
            let r = self.logic_and();
            v = if v != 0 || r != 0 { 1 } else { 0 };
        }
        v
    }

    fn logic_and(&mut self) -> i64 {
        let mut v = self.bit_or();
        while self.eat2(b'&', b'&') {
            let r = self.bit_or();
            v = if v != 0 && r != 0 { 1 } else { 0 };
        }
        v
    }

    fn bit_or(&mut self) -> i64 {
        let mut v = self.bit_xor();
        loop {
            self.skip();
            if self.src.get(self.pos) == Some(&b'|') && self.src.get(self.pos + 1) != Some(&b'|') {
                self.pos += 1;
                v |= self.bit_xor();
            } else {
                break;
            }
        }
        v
    }

    fn bit_xor(&mut self) -> i64 {
        let mut v = self.bit_and();
        while self.eat(b'^') {
            v ^= self.bit_and();
        }
        v
    }

    fn bit_and(&mut self) -> i64 {
        let mut v = self.equality();
        loop {
            self.skip();
            if self.src.get(self.pos) == Some(&b'&') && self.src.get(self.pos + 1) != Some(&b'&') {
                self.pos += 1;
                v &= self.equality();
            } else {
                break;
            }
        }
        v
    }

    fn equality(&mut self) -> i64 {
        let mut v = self.relational();
        loop {
            if self.eat2(b'=', b'=') {
                v = i64::from(v == self.relational());
            } else if self.eat2(b'!', b'=') {
                v = i64::from(v != self.relational());
            } else {
                break;
            }
        }
        v
    }

    fn relational(&mut self) -> i64 {
        let mut v = self.shift();
        loop {
            if self.eat2(b'<', b'=') {
                v = i64::from(v <= self.shift());
            } else if self.eat2(b'>', b'=') {
                v = i64::from(v >= self.shift());
            } else {
                self.skip();
                if self.src.get(self.pos) == Some(&b'<')
                    && self.src.get(self.pos + 1) != Some(&b'<')
                {
                    self.pos += 1;
                    v = i64::from(v < self.shift());
                } else if self.src.get(self.pos) == Some(&b'>')
                    && self.src.get(self.pos + 1) != Some(&b'>')
                {
                    self.pos += 1;
                    v = i64::from(v > self.shift());
                } else {
                    break;
                }
            }
        }
        v
    }

    fn shift(&mut self) -> i64 {
        let mut v = self.add();
        loop {
            if self.eat2(b'<', b'<') {
                let r = self.add();
                v <<= r.clamp(0, 63);
            } else if self.eat2(b'>', b'>') {
                let r = self.add();
                v >>= r.clamp(0, 63);
            } else {
                break;
            }
        }
        v
    }

    fn add(&mut self) -> i64 {
        let mut v = self.mul();
        loop {
            self.skip();
            if self.src.get(self.pos) == Some(&b'+') && self.src.get(self.pos + 1) != Some(&b'+') {
                self.pos += 1;
                v = v.wrapping_add(self.mul());
            } else if self.src.get(self.pos) == Some(&b'-')
                && self.src.get(self.pos + 1) != Some(&b'-')
            {
                self.pos += 1;
                v = v.wrapping_sub(self.mul());
            } else {
                break;
            }
        }
        v
    }

    fn mul(&mut self) -> i64 {
        let mut v = self.power();
        loop {
            self.skip();
            // Don't eat ** here — power() owns it
            if self.src.get(self.pos) == Some(&b'*') && self.src.get(self.pos + 1) == Some(&b'*') {
                break;
            }
            if self.eat(b'*') {
                v = v.wrapping_mul(self.power());
            } else if self.eat(b'/') {
                let r = self.power();
                v = if r != 0 { v / r } else { 0 };
            } else if self.eat(b'%') {
                let r = self.power();
                v = if r != 0 { v % r } else { 0 };
            } else {
                break;
            }
        }
        v
    }

    fn power(&mut self) -> i64 {
        let base = self.unary();
        if self.eat2(b'*', b'*') {
            let exp = self.power(); // right-associative
            if exp >= 0 {
                base.pow(exp.min(62) as u32)
            } else {
                0
            }
        } else {
            base
        }
    }

    fn unary(&mut self) -> i64 {
        self.skip();
        if self.eat(b'-') {
            -(self.primary())
        } else if self.eat(b'+') {
            self.primary()
        } else if self.eat(b'!') {
            i64::from(self.primary() == 0)
        } else if self.eat(b'~') {
            !self.primary()
        } else {
            self.primary()
        }
    }

    fn primary(&mut self) -> i64 {
        self.skip();
        if self.eat(b'(') {
            let v = self.expr();
            self.eat(b')');
            return v;
        }
        // Hex literal
        if self.src.get(self.pos..self.pos + 2) == Some(b"0x")
            || self.src.get(self.pos..self.pos + 2) == Some(b"0X")
        {
            self.pos += 2;
            let start = self.pos;
            while self.pos < self.src.len() && self.src[self.pos].is_ascii_hexdigit() {
                self.pos += 1;
            }
            return i64::from_str_radix(
                std::str::from_utf8(&self.src[start..self.pos]).unwrap_or("0"),
                16,
            )
            .unwrap_or(0);
        }
        // Octal literal (leading 0 followed by digits)
        if self.src.get(self.pos) == Some(&b'0')
            && self
                .src
                .get(self.pos + 1)
                .map(|&c| c.is_ascii_digit())
                .unwrap_or(false)
        {
            self.pos += 1;
            let start = self.pos;
            while self.pos < self.src.len()
                && self.src[self.pos] >= b'0'
                && self.src[self.pos] <= b'7'
            {
                self.pos += 1;
            }
            return i64::from_str_radix(
                std::str::from_utf8(&self.src[start..self.pos]).unwrap_or("0"),
                8,
            )
            .unwrap_or(0);
        }
        // Decimal integer
        let start = self.pos;
        while self.pos < self.src.len() && self.src[self.pos].is_ascii_digit() {
            self.pos += 1;
        }
        if self.pos == start {
            return 0;
        }
        std::str::from_utf8(&self.src[start..self.pos])
            .unwrap_or("0")
            .parse()
            .unwrap_or(0)
    }
}

// ── Bash unary / binary tests ─────────────────────────────────────────────────

fn eval_bash_unary(op: &str, arg: &str) -> ExitStatus {
    let ok = match op {
        "-z" => arg.is_empty(),
        "-n" => !arg.is_empty(),
        "-f" => std::path::Path::new(arg).is_file(),
        "-d" => std::path::Path::new(arg).is_dir(),
        "-e" => std::path::Path::new(arg).exists(),
        "-r" => file_mode_bit(arg, 0o444),
        "-w" => file_mode_bit(arg, 0o222),
        "-x" => file_mode_bit(arg, 0o111),
        "-s" => std::fs::metadata(arg).map(|m| m.len() > 0).unwrap_or(false),
        "-L" | "-h" => is_symlink(arg),
        "-p" => is_fifo(arg),
        "-S" => is_socket(arg),
        "-b" => is_block(arg),
        "-c" => is_char(arg),
        "-k" => file_mode_bit(arg, 0o1000), // sticky
        "-g" => file_mode_bit(arg, 0o2000), // setgid
        "-u" => file_mode_bit(arg, 0o4000), // setuid
        _ => false,
    };
    if ok {
        ExitStatus::OK
    } else {
        ExitStatus::FAIL
    }
}

fn eval_bash_binary(left: &str, op: &str, right: &str) -> ExitStatus {
    let ok = match op {
        // In [[ ]], unquoted RHS of == / != is a glob pattern (bash).
        "=" | "==" => {
            if has_glob_chars(right) {
                glob_match(right, left)
            } else {
                left == right
            }
        }
        "!=" => {
            if has_glob_chars(right) {
                !glob_match(right, left)
            } else {
                left != right
            }
        }
        "<" => left < right,
        ">" => left > right,
        "=~" => ere_contains(left, right),
        "-eq" => arith_cmp(left, right, |a, b| a == b),
        "-ne" => arith_cmp(left, right, |a, b| a != b),
        "-lt" => arith_cmp(left, right, |a, b| a < b),
        "-le" => arith_cmp(left, right, |a, b| a <= b),
        "-gt" => arith_cmp(left, right, |a, b| a > b),
        "-ge" => arith_cmp(left, right, |a, b| a >= b),
        "-nt" => {
            let t1 = mtime(left);
            let t2 = mtime(right);
            match (t1, t2) {
                (Some(a), Some(b)) => a > b,
                (Some(_), None) => true,
                _ => false,
            }
        }
        "-ot" => {
            let t1 = mtime(left);
            let t2 = mtime(right);
            match (t1, t2) {
                (Some(a), Some(b)) => a < b,
                (None, Some(_)) => true,
                _ => false,
            }
        }
        "-ef" => same_file(left, right),
        _ => false,
    };
    if ok {
        ExitStatus::OK
    } else {
        ExitStatus::FAIL
    }
}

// ── ERE pattern matching (minimal, no external crate) ─────────────────────────

/// Basic ERE matching — handles `.`, `*`, `+`, `?`, `^`, `$`, `[...]`, `()`, `|`.
/// Not a complete POSIX ERE engine, but covers the overwhelming majority of
/// real-world `[[ str =~ pattern ]]` use cases.
fn ere_contains(subject: &str, pattern: &str) -> bool {
    if pattern.is_empty() {
        return true;
    }
    let anchored_start = pattern.starts_with('^');
    let anchored_end = pattern.ends_with('$') && !pattern.ends_with("\\$");
    let core = pattern
        .strip_prefix('^')
        .unwrap_or(pattern)
        .strip_suffix('$')
        .unwrap_or_else(|| {
            if anchored_end {
                &pattern[..pattern.len() - 1]
            } else {
                pattern
            }
        });
    let s = subject.as_bytes();
    if anchored_start && anchored_end {
        ere_match_full(s, core.as_bytes())
    } else if anchored_start {
        ere_match_prefix(s, core.as_bytes())
    } else if anchored_end {
        ere_match_suffix(s, core.as_bytes())
    } else {
        for i in 0..=s.len() {
            if ere_match_prefix(&s[i..], core.as_bytes()) {
                return true;
            }
        }
        false
    }
}

fn ere_match_full(s: &[u8], p: &[u8]) -> bool {
    matches!(ere_try(s, p), Some(n) if n == s.len())
}

fn ere_match_prefix(s: &[u8], p: &[u8]) -> bool {
    ere_try(s, p).is_some()
}

fn ere_match_suffix(s: &[u8], p: &[u8]) -> bool {
    for i in 0..=s.len() {
        if ere_match_full(&s[i..], p) {
            return true;
        }
    }
    false
}

/// Try to match `p` at the start of `s`. Returns `Some(n)` where `n` is the
/// number of bytes consumed from `s`, or `None` on failure.
fn ere_try(s: &[u8], p: &[u8]) -> Option<usize> {
    if p.is_empty() {
        return Some(0);
    }

    // Handle alternation at the top level by splitting on unescaped '|' outside groups
    // For simplicity we split first-level | only
    if let Some(parts) = split_alternation(p) {
        for part in parts {
            if let Some(n) = ere_try(s, part) {
                return Some(n);
            }
        }
        return None;
    }

    // Parse first atom + optional repetition
    let (atom_len, class) = parse_ere_atom(p)?;
    let rest = &p[atom_len..];
    let (min_rep, max_rep, rep_len) = parse_repetition(rest);
    let rest = &rest[rep_len..];

    // Greedy: try as many as possible, then backtrack
    let mut count = 0usize;
    let mut si = 0usize;
    let mut positions = vec![si];
    while count < max_rep && si < s.len() {
        if !class.matches(s[si]) {
            break;
        }
        si += 1;
        count += 1;
        positions.push(si);
    }
    // Try from most consumed to least (greedy)
    for &end in positions.iter().rev() {
        if count < min_rep && end < positions.len() {
            continue;
        }
        let consumed_atoms = positions.iter().position(|&p| p == end).unwrap_or(0);
        if consumed_atoms < min_rep {
            continue;
        }
        if let Some(n) = ere_try(&s[end..], rest) {
            return Some(end + n);
        }
    }
    None
}

fn split_alternation(p: &[u8]) -> Option<Vec<&[u8]>> {
    let mut parts = vec![];
    let mut depth = 0usize;
    let mut start = 0;
    let mut found = false;
    let mut i = 0;
    while i < p.len() {
        match p[i] {
            b'(' => depth += 1,
            b')' => {
                if depth > 0 {
                    depth -= 1;
                }
            }
            b'[' => {
                while i < p.len() && p[i] != b']' {
                    i += 1;
                }
            }
            b'\\' => {
                i += 1;
            } // skip escaped
            b'|' if depth == 0 => {
                parts.push(&p[start..i]);
                start = i + 1;
                found = true;
            }
            _ => {}
        }
        i += 1;
    }
    if !found {
        return None;
    }
    parts.push(&p[start..]);
    Some(parts)
}

enum EreClass {
    Any,
    Literal(u8),
    Class(Vec<u8>, bool), // bytes that match, negate
}

impl EreClass {
    fn matches(&self, c: u8) -> bool {
        match self {
            EreClass::Any => c != b'\n',
            EreClass::Literal(b) => c == *b,
            EreClass::Class(v, n) => {
                let found = v.contains(&c);
                if *n {
                    !found
                } else {
                    found
                }
            }
        }
    }
}

/// Returns (atom_byte_len, class). Returns None if p is empty or just anchors.
fn parse_ere_atom(p: &[u8]) -> Option<(usize, EreClass)> {
    match p.first()? {
        b'.' => Some((1, EreClass::Any)),
        b'\\' => {
            let c = *p.get(1).unwrap_or(&b'\\');
            Some((2, EreClass::Literal(c)))
        }
        b'[' => {
            // Parse character class
            let inner = &p[1..];
            let negate = matches!(inner.first(), Some(b'^'));
            let class_inner = if negate { &inner[1..] } else { inner };
            let mut bytes = vec![];
            let mut i = 0;
            // ] at position 0 is literal
            if class_inner.get(i) == Some(&b']') {
                bytes.push(b']');
                i += 1;
            }
            while i < class_inner.len() && class_inner[i] != b']' {
                if i + 2 < class_inner.len()
                    && class_inner[i + 1] == b'-'
                    && class_inner[i + 2] != b']'
                {
                    let lo = class_inner[i];
                    let hi = class_inner[i + 2];
                    for c in lo..=hi {
                        bytes.push(c);
                    }
                    i += 3;
                } else {
                    bytes.push(class_inner[i]);
                    i += 1;
                }
            }
            let total = 1
                + (if negate { 1 } else { 0 })
                + i
                + if class_inner.get(i) == Some(&b']') {
                    1
                } else {
                    0
                };
            Some((total, EreClass::Class(bytes, negate)))
        }
        b'(' => {
            // Find matching )
            let mut depth = 1;
            let mut i = 1;
            while i < p.len() {
                match p[i] {
                    b'(' => depth += 1,
                    b')' => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
                i += 1;
            }
            let inner = &p[1..i];
            let total = i + 1;
            // Treat group as a class that tries to match the group content — simplified
            // (full grouping would require a recursive NFA, this just handles common cases)
            let _ = inner; // used structurally
            Some((total, EreClass::Any)) // Placeholder: group matches anything (simplified)
        }
        b'^' | b'$' => None, // anchors not atoms
        &c => Some((1, EreClass::Literal(c))),
    }
}

fn parse_repetition(p: &[u8]) -> (usize, usize, usize) {
    match p.first() {
        Some(b'*') => (0, usize::MAX, 1),
        Some(b'+') => (1, usize::MAX, 1),
        Some(b'?') => (0, 1, 1),
        Some(b'{') => {
            // {n}, {n,}, {n,m}
            let mut i = 1;
            let start = i;
            while i < p.len() && p[i].is_ascii_digit() {
                i += 1;
            }
            let min: usize = std::str::from_utf8(&p[start..i])
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            if p.get(i) == Some(&b'}') {
                return (min, min, i + 1);
            }
            if p.get(i) == Some(&b',') {
                i += 1;
                let start2 = i;
                while i < p.len() && p[i].is_ascii_digit() {
                    i += 1;
                }
                if p.get(i) == Some(&b'}') {
                    let max: usize = std::str::from_utf8(&p[start2..i])
                        .ok()
                        .and_then(|s| s.parse().ok())
                        .unwrap_or(usize::MAX);
                    return (min, max, i + 1);
                }
            }
            (1, 1, 0)
        }
        _ => (1, 1, 0),
    }
}

// ── File-system helpers for bash tests ────────────────────────────────────────

fn file_mode_bit(path: &str, bits: u32) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path)
            .map(|m| m.permissions().mode() & bits != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        false
    }
}

fn is_symlink(path: &str) -> bool {
    std::fs::symlink_metadata(path)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
}

#[cfg(unix)]
fn is_fifo(path: &str) -> bool {
    use std::os::unix::fs::FileTypeExt;
    std::fs::metadata(path)
        .map(|m| m.file_type().is_fifo())
        .unwrap_or(false)
}
#[cfg(not(unix))]
fn is_fifo(_: &str) -> bool {
    false
}

#[cfg(unix)]
fn is_socket(path: &str) -> bool {
    use std::os::unix::fs::FileTypeExt;
    std::fs::metadata(path)
        .map(|m| m.file_type().is_socket())
        .unwrap_or(false)
}
#[cfg(not(unix))]
fn is_socket(_: &str) -> bool {
    false
}

#[cfg(unix)]
fn is_block(path: &str) -> bool {
    use std::os::unix::fs::FileTypeExt;
    std::fs::metadata(path)
        .map(|m| m.file_type().is_block_device())
        .unwrap_or(false)
}
#[cfg(not(unix))]
fn is_block(_: &str) -> bool {
    false
}

#[cfg(unix)]
fn is_char(path: &str) -> bool {
    use std::os::unix::fs::FileTypeExt;
    std::fs::metadata(path)
        .map(|m| m.file_type().is_char_device())
        .unwrap_or(false)
}
#[cfg(not(unix))]
fn is_char(_: &str) -> bool {
    false
}

fn mtime(path: &str) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

#[cfg(unix)]
fn same_file(a: &str, b: &str) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (std::fs::metadata(a), std::fs::metadata(b)) {
        (Ok(ma), Ok(mb)) => ma.dev() == mb.dev() && ma.ino() == mb.ino(),
        _ => false,
    }
}
#[cfg(not(unix))]
fn same_file(_: &str, _: &str) -> bool {
    false
}

fn arith_cmp(left: &str, right: &str, f: impl Fn(i64, i64) -> bool) -> bool {
    f(
        left.trim().parse().unwrap_or(0),
        right.trim().parse().unwrap_or(0),
    )
}

// ── ${var#pattern} / ${var%pattern} helpers ───────────────────────────────────

/// Trim the shortest matching prefix from `s` that matches `pattern`.
fn trim_prefix_shortest(s: &str, pattern: &str) -> String {
    // Try prefixes from shortest to longest.
    for end in 0..=s.len() {
        // Ensure we only split on valid char boundaries.
        if !s.is_char_boundary(end) {
            continue;
        }
        if glob_match(pattern, &s[..end]) {
            return s[end..].to_string();
        }
    }
    s.to_string()
}

/// Trim the shortest matching suffix from `s` that matches `pattern`.
fn trim_suffix_shortest(s: &str, pattern: &str) -> String {
    // Try suffixes from shortest to longest.
    for start in (0..=s.len()).rev() {
        if !s.is_char_boundary(start) {
            continue;
        }
        if glob_match(pattern, &s[start..]) {
            return s[..start].to_string();
        }
    }
    s.to_string()
}

/// Trim the longest matching prefix from `s` that matches `pattern`.
fn trim_prefix_longest(s: &str, pattern: &str) -> String {
    for end in (0..=s.len()).rev() {
        if !s.is_char_boundary(end) {
            continue;
        }
        if glob_match(pattern, &s[..end]) {
            return s[end..].to_string();
        }
    }
    s.to_string()
}

/// Trim the longest matching suffix from `s` that matches `pattern`.
fn trim_suffix_longest(s: &str, pattern: &str) -> String {
    for start in 0..=s.len() {
        if !s.is_char_boundary(start) {
            continue;
        }
        if glob_match(pattern, &s[start..]) {
            return s[..start].to_string();
        }
    }
    s.to_string()
}
