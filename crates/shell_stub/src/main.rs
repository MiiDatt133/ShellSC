use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

use anyhow::{bail, Context, Result};
use goblin::elf::Elf;
use shell_bc::Bytecode;
use shell_vm::{set_sigint, set_sigterm, Vm};

static SIGNALS_REGISTERED: AtomicBool = AtomicBool::new(false);

extern "C" fn sigint_handler(_: libc::c_int) {
    set_sigint();
}

extern "C" fn sigterm_handler(_: libc::c_int) {
    set_sigterm();
}

fn register_signal_handlers() {
    if SIGNALS_REGISTERED.swap(true, Ordering::SeqCst) {
        return;
    }
    unsafe {
        libc::signal(
            libc::SIGINT,
            sigint_handler as *const () as libc::sighandler_t,
        );
        libc::signal(
            libc::SIGTERM,
            sigterm_handler as *const () as libc::sighandler_t,
        );
    }
}

fn main() {
    if let Err(e) = run() {
        eprintln!("{e:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    register_signal_handlers();
    let exe = exe_path()?;
    let bytes = fs::read(&exe).with_context(|| format!("reading {}", exe.display()))?;

    let elf = Elf::parse(&bytes).map_err(|e| anyhow::anyhow!("parsing ELF: {}", e))?;

    for sh in &elf.section_headers {
        if elf.shdr_strtab.get_at(sh.sh_name) == Some(".shellsc") {
            let start = sh.sh_offset as usize;
            let end = start + sh.sh_size as usize;
            let payload = bytes
                .get(start..end)
                .context(".shellsc section out of bounds")?;

            let mut header_opt: Option<shell_pack::ProtectHeader> = None;
            let mut bc_bytes: Vec<u8> = if shell_pack::is_protected(payload) {
                let header = shell_pack::ProtectHeader::from_bytes(payload)
                    .map_err(|e| anyhow::anyhow!("protect header: {}", e))?;
                if header.flags & shell_pack::protect::FLAG_ANTIDEBUG != 0 {
                    antidebug_check()?;
                }
                if header.flags & shell_pack::protect::FLAG_ANTIHOOK != 0 {
                    antihook_check()?;
                }
                if header.flags & shell_pack::protect::FLAG_SELFDEBUG != 0 {
                    selfdebug_check()?;
                }
                let bc = shell_pack::open(payload)
                    .map_err(|e| anyhow::anyhow!("unsealing bytecode: {}", e))?
                    .context("protected payload too short")?;
                header_opt = Some(header);
                bc
            } else {
                payload.to_vec()
            };

            let bc = Bytecode::from_bytes(&bc_bytes)
                .map_err(|e| anyhow::anyhow!("deserializing bytecode: {}", e))?;
            // Scrub the transient plaintext buffer before running — with
            // SMC on, the program should only exist encrypted past this point.
            bc_bytes.iter_mut().for_each(|b| *b ^= 0xFF);
            drop(bc_bytes);
            let mut vm = Vm::new(bc);
            if let Some(h) = &header_opt {
                if h.flags & shell_pack::protect::FLAG_CFF != 0
                    || h.flags & shell_pack::protect::FLAG_OPAQUE != 0
                {
                    vm.enable_cff(h.cff_seed, h.opaque_param1, h.opaque_param2);
                }
                if h.flags & shell_pack::protect::FLAG_SMC != 0 {
                    // Re-key the in-memory program: seal it back under a
                    // derived key so the deserialized plaintext never lingers.
                    let mut k = h.key;
                    for (i, b) in k.iter_mut().enumerate() {
                        *b = b.wrapping_add(h.crc32 as u8).rotate_left(2)
                            ^ (h.cff_seed as u8).wrapping_mul(31 ^ i as u8);
                    }
                    vm.enable_smc(k);
                }
            }
            if let Some(arg0) = std::env::args().next() {
                vm.set_arg0(&arg0);
            }
            let status = vm
                .run()
                .map_err(|e| anyhow::anyhow!("running bytecode: {}", e))?;
            std::process::exit(status.code());
        }
    }

    bail!("no .shellsc section found — is this a valid .sc file?")
}

/// Refuse to run under a tracer (ptrace-based debugger / strace). Reads
/// TracerPid from /proc/self/status: 0 means untraced.
fn antidebug_check() -> Result<()> {
    let status = fs::read_to_string("/proc/self/status").context("reading /proc/self/status")?;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("TracerPid:") {
            let pid: i64 = rest.trim().parse().context("parsing TracerPid")?;
            if pid != 0 {
                bail!("refusing to run under tracer (pid {pid})");
            }
            return Ok(());
        }
    }
    bail!("TracerPid not found in /proc/self/status")
}

/// Refuse to run with library hooks in play: an injected LD_PRELOAD or a
/// known hooking framework mapped into the process.
fn antihook_check() -> Result<()> {
    if let Ok(v) = std::env::var("LD_PRELOAD") {
        if !v.trim().is_empty() {
            bail!("refusing to run with LD_PRELOAD set");
        }
    }
    let maps = fs::read_to_string("/proc/self/maps").context("reading /proc/self/maps")?;
    const HOOK_MARKERS: [&str; 6] = ["frida", "gadget", "xposed", "substrate", "lsposed", "cydia"];
    let maps_lower = maps.to_lowercase();
    for m in HOOK_MARKERS {
        if maps_lower.contains(m) {
            bail!("refusing to run: hook framework marker '{m}' in memory map");
        }
    }
    Ok(())
}

/// Fork a child that ptrace-attaches the parent, occupying the single
/// tracer slot so no external debugger can attach. The child acts as a
/// minimal tracer: it resumes the parent on every stop, forwards real
/// signals so the parent's handlers still fire, and exits as soon as
/// the parent does — otherwise the parent's exit is never reaped and
/// the invoking shell hangs on wait().
fn selfdebug_check() -> Result<()> {
    unsafe {
        let pid = libc::fork();
        if pid < 0 {
            bail!("self-debug: fork failed");
        }
        if pid == 0 {
            // Child: attach to parent, then trace until it exits.
            let ppid = libc::getppid();
            if libc::ptrace(
                libc::PTRACE_ATTACH,
                ppid,
                std::ptr::null_mut::<libc::c_void>(),
                std::ptr::null_mut::<libc::c_void>(),
            ) < 0
            {
                libc::_exit(1);
            }
            let mut status: libc::c_int = 0;
            loop {
                if libc::waitpid(ppid, &mut status, 0) <= 0 {
                    break; // parent gone (reaped elsewhere / ECHILD)
                }
                if libc::WIFEXITED(status) || libc::WIFSIGNALED(status) {
                    break; // parent exited — die with it
                }
                if libc::WIFSTOPPED(status) {
                    let sig = libc::WSTOPSIG(status);
                    // SIGSTOP (from ATTACH) and SIGTRAP are internal
                    // stops: resume without injecting. Real signals are
                    // forwarded so SIGINT/SIGTERM handlers still run.
                    let deliver = if sig == libc::SIGSTOP || sig == libc::SIGTRAP {
                        0
                    } else {
                        sig
                    };
                    libc::ptrace(
                        libc::PTRACE_CONT,
                        ppid,
                        std::ptr::null_mut::<libc::c_void>(),
                        deliver as *mut libc::c_void,
                    );
                }
            }
            libc::_exit(0);
        }
        // Parent: the child attaches and resumes us; just continue.
    }
    Ok(())
}

fn exe_path() -> Result<PathBuf> {
    std::env::current_exe().or_else(|_| {
        Path::new("/proc/self/exe")
            .canonicalize()
            .context("resolving /proc/self/exe")
    })
}
