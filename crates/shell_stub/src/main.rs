mod raw;

use std::{
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
    // Read the ELF through a raw syscall: the anti-tamper key is derived
    // from these bytes, so an interposed libc read must not be able to
    // hand back modified contents.
    let bytes = unsafe { raw::raw_open_read(&exe.to_string_lossy()) }
        .with_context(|| format!("reading {}", exe.display()))?;

    let elf = Elf::parse(&bytes).map_err(|e| anyhow::anyhow!("parsing ELF: {}", e))?;

    for sh in &elf.section_headers {
        if elf.shdr_strtab.get_at(sh.sh_name) == Some(".shellsc") {
            let start = sh.sh_offset as usize;
            let end = start + sh.sh_size as usize;
            let payload = bytes
                .get(start..end)
                .context(".shellsc section out of bounds")?;

            let mut header_opt: Option<shell_pack::ProtectHeader> = None;
            let is_protected = shell_pack::is_protected(payload);

            // Derive the key from clean ELF bytes BEFORE anti-dump hardening
            // zeroes the ELF magic — otherwise text_crc_from_elf cannot parse
            // and the .text integrity check silently degrades to crc(0).
            let text_crc = if is_protected {
                shell_pack::protect::text_crc_from_elf(&bytes).unwrap_or(0)
            } else {
                0
            };

            let mut bc_bytes: Vec<u8> = if is_protected {
                eprintln!("Protected by ShellSC");
                let header = shell_pack::ProtectHeader::from_bytes(payload)
                    .map_err(|e| anyhow::anyhow!("protect header: {}", e))?;
                // PR_SET_DUMPABLE=0 makes the kernel deny reads of this
                // process's own /proc/self/environ and PTRACE_ATTACH — run
                // the checks before hardening, harden before decryption.
                if header.flags & shell_pack::protect::FLAG_ANTIDEBUG != 0 {
                    antidebug_check()?;
                }
                if header.flags & shell_pack::protect::FLAG_ANTIHOOK != 0 {
                    antihook_check()?;
                }
                if header.flags & shell_pack::protect::FLAG_SELFDEBUG != 0 {
                    selfdebug_check()?;
                }
                anti_dump_harden(&bytes);
                let bc = shell_pack::open(payload, text_crc)
                    .map_err(|e| anyhow::anyhow!("unsealing bytecode: {}", e))?
                    .context("protected payload too short")?;
                header_opt = Some(header);
                bc
            } else {
                payload.to_vec()
            };

            let bc = Bytecode::from_bytes(&bc_bytes)
                .map_err(|e| anyhow::anyhow!("deserializing bytecode: {}", e))?;
            bc_bytes.iter_mut().for_each(|b| *b ^= 0xFF);
            drop(bc_bytes);

            let mut vm = Vm::new(bc);
            if let Some(h) = &header_opt {
                // v4 headers store the seeds XOR-masked with the .text CRC —
                // derive the runtime values the same way open() does.
                let real_version = payload.get(12).copied().unwrap_or(0);
                let seeds = shell_pack::derive_seeds(h, real_version, text_crc);
                if h.flags & shell_pack::protect::FLAG_CFF != 0
                    || h.flags & shell_pack::protect::FLAG_OPAQUE != 0
                {
                    vm.enable_cff(seeds.cff_seed, h.opaque_param1, h.opaque_param2);
                }
                if h.flags & shell_pack::protect::FLAG_SMC != 0 {
                    let mut k = h.key;
                    for (i, b) in k.iter_mut().enumerate() {
                        *b = b.wrapping_add(h.crc32 as u8).rotate_left(2)
                            ^ (seeds.cff_seed as u8).wrapping_mul(31 ^ i as u8);
                    }
                    vm.enable_smc(k);
                    k.iter_mut().for_each(|b| *b = 0);
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

/// Refuse to run under a tracer (ptrace-based debugger / strace).
/// Reads /proc/self/status via a raw syscall — an interposed libc
/// `open`/`read` cannot fake the TracerPid line.
fn antidebug_check() -> Result<()> {
    let status =
        unsafe { raw::raw_open_read("/proc/self/status").context("reading /proc/self/status")? };
    let text = String::from_utf8_lossy(&status);
    for line in text.lines() {
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

/// Refuse to run with library hooks in play. Three signals:
/// 1. LD_PRELOAD read from the kernel's own /proc/self/environ snapshot —
///    an unsetenv() in a preload constructor does not clean this copy.
/// 2. /proc/self/maps via raw syscall — interposition cannot filter lines.
/// 3. frida-style gadget libraries visible in the map.
fn antihook_check() -> Result<()> {
    if let Some(env) = unsafe { raw::raw_open_read("/proc/self/environ") } {
        for kv in env.split(|b| *b == 0) {
            if let Ok(s) = std::str::from_utf8(kv) {
                if let Some(v) = s.strip_prefix("LD_PRELOAD=") {
                    if !v.trim().is_empty() {
                        bail!("refusing to run with LD_PRELOAD set");
                    }
                }
            }
        }
    }
    let maps = unsafe { raw::raw_open_read("/proc/self/maps").context("reading /proc/self/maps")? };
    const HOOK_MARKERS: [&str; 6] = ["frida", "gadget", "xposed", "substrate", "lsposed", "cydia"];
    let maps_lower = String::from_utf8_lossy(&maps).to_lowercase();
    for m in HOOK_MARKERS {
        if maps_lower.contains(m) {
            bail!("refusing to run: hook framework marker '{m}' in memory map");
        }
    }
    Ok(())
}

/// Fork a child that ptrace-attaches the parent, occupying the single
/// tracer slot so no external debugger can attach. Uses a pipe to block
/// the parent until the child has completed PTRACE_ATTACH, closing the
/// race window where an external debugger could slip in. All syscalls
/// are raw — a preloaded library cannot interpose fork/ptrace/wait to
/// fake the attach or hand the slot to its own tracer.
fn selfdebug_check() -> Result<()> {
    unsafe {
        let mut pipefd = [0i32; 2];
        if raw::raw_pipe(&mut pipefd) < 0 {
            bail!("self-debug: pipe failed");
        }
        let pid = raw::raw_fork();
        if pid < 0 {
            bail!("self-debug: fork failed");
        }
        if pid == 0 {
            raw::raw_close(pipefd[0] as usize);
            let ppid = raw::raw_getppid();
            if raw::raw_ptrace(raw::PTRACE_ATTACH, ppid, 0, 0) < 0 {
                raw::raw_exit_group(1);
            }
            let mut status: i32 = 0;
            if raw::raw_waitpid(ppid, &mut status) > 0 && raw::wif_stopped(status) {
                raw::raw_ptrace(raw::PTRACE_CONT, ppid, 0, 0);
                let buf = [1u8; 1];
                let _ = raw::raw_write(pipefd[1] as usize, &buf);
            } else {
                raw::raw_exit_group(1);
            }
            raw::raw_close(pipefd[1] as usize);
            loop {
                if raw::raw_waitpid(ppid, &mut status) <= 0 {
                    break;
                }
                if raw::wif_exited(status) || raw::wif_signaled(status) {
                    break;
                }
                if raw::wif_stopped(status) {
                    let sig = raw::w_stop_sig(status);
                    let deliver = if sig == raw::SIG_STOP || sig == raw::SIG_TRAP {
                        0
                    } else {
                        sig
                    };
                    raw::raw_ptrace(raw::PTRACE_CONT, ppid, 0, deliver as usize);
                }
            }
            raw::raw_exit_group(0);
        }
        raw::raw_close(pipefd[1] as usize);
        let mut buf = [0u8; 1];
        let _ = raw::raw_read(pipefd[0] as usize, &mut buf);
        raw::raw_close(pipefd[0] as usize);
    }
    Ok(())
}

/// Anti-dump hardening: lock .text in memory, exclude from core dumps,
/// disable dumpable flag, and zero ELF headers so tools like readelf/objdump
/// cannot parse the in-memory image. Best-effort, non-fatal on failure.
fn anti_dump_harden(elf_bytes: &[u8]) {
    unsafe {
        libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0);
    }

    let elf = match Elf::parse(elf_bytes) {
        Ok(e) => e,
        Err(_) => return,
    };
    for sh in &elf.section_headers {
        if elf.shdr_strtab.get_at(sh.sh_name) == Some(".text") {
            let start = sh.sh_offset as usize;
            let len = sh.sh_size as usize;
            if start + len > elf_bytes.len() || len == 0 {
                break;
            }
            let ptr = elf_bytes.as_ptr() as *mut libc::c_void;
            let text_ptr = unsafe { ptr.add(start) };
            let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) } as usize;
            if page_size == 0 {
                break;
            }
            let aligned = (text_ptr as usize) & !(page_size - 1);
            let aligned_len = len + (text_ptr as usize - aligned);
            let aligned_ptr = aligned as *mut libc::c_void;
            unsafe {
                libc::mlock(aligned_ptr, aligned_len);
                #[cfg(target_os = "linux")]
                libc::madvise(aligned_ptr, aligned_len, libc::MADV_DONTDUMP);
            }
            break;
        }
    }

    let ptr = elf_bytes.as_ptr() as *mut u8;
    if elf_bytes.len() >= 4 {
        unsafe {
            std::ptr::write_bytes(ptr, 0, 4);
        }
    }
    if elf_bytes.len() >= 48 && elf.header.e_shoff != 0 {
        let shoff_ptr = unsafe { ptr.add(40) };
        unsafe {
            std::ptr::write_bytes(shoff_ptr, 0, 8);
        }
    }
}

fn exe_path() -> Result<PathBuf> {
    std::env::current_exe().or_else(|_| {
        Path::new("/proc/self/exe")
            .canonicalize()
            .context("resolving /proc/self/exe")
    })
}
