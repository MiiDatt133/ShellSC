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
            let is_protected = shell_pack::is_protected(payload);

            // Harden BEFORE decryption so no plaintext window exists
            // between open() and harden — closes crash-dump exposure.
            if is_protected {
                anti_dump_harden(&bytes);
            }

            let mut bc_bytes: Vec<u8> = if is_protected {
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
                let text_crc = shell_pack::protect::text_crc_from_elf(&bytes).unwrap_or(0);
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
                if h.flags & shell_pack::protect::FLAG_CFF != 0
                    || h.flags & shell_pack::protect::FLAG_OPAQUE != 0
                {
                    vm.enable_cff(h.cff_seed, h.opaque_param1, h.opaque_param2);
                }
                if h.flags & shell_pack::protect::FLAG_SMC != 0 {
                    let mut k = h.key;
                    for (i, b) in k.iter_mut().enumerate() {
                        *b = b.wrapping_add(h.crc32 as u8).rotate_left(2)
                            ^ (h.cff_seed as u8).wrapping_mul(31 ^ i as u8);
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

/// Refuse to run with library hooks in play.
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
/// tracer slot so no external debugger can attach. Uses a pipe to block
/// the parent until the child has completed PTRACE_ATTACH, closing the
/// race window where an external debugger could slip in.
fn selfdebug_check() -> Result<()> {
    unsafe {
        let mut pipefd = [0i32; 2];
        if libc::pipe(pipefd.as_mut_ptr()) < 0 {
            bail!("self-debug: pipe failed");
        }
        let pid = libc::fork();
        if pid < 0 {
            bail!("self-debug: fork failed");
        }
        if pid == 0 {
            libc::close(pipefd[0]);
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
            if libc::waitpid(ppid, &mut status, 0) > 0 && libc::WIFSTOPPED(status) {
                libc::ptrace(
                    libc::PTRACE_CONT,
                    ppid,
                    std::ptr::null_mut::<libc::c_void>(),
                    std::ptr::null_mut::<libc::c_void>(),
                );
                let buf = [1u8; 1];
                let _ = libc::write(pipefd[1], buf.as_ptr() as *const libc::c_void, 1);
            } else {
                libc::_exit(1);
            }
            libc::close(pipefd[1]);
            loop {
                if libc::waitpid(ppid, &mut status, 0) <= 0 {
                    break;
                }
                if libc::WIFEXITED(status) || libc::WIFSIGNALED(status) {
                    break;
                }
                if libc::WIFSTOPPED(status) {
                    let sig = libc::WSTOPSIG(status);
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
        libc::close(pipefd[1]);
        let mut buf = [0u8; 1];
        let _ = libc::read(pipefd[0], buf.as_mut_ptr() as *mut libc::c_void, 1);
        libc::close(pipefd[0]);
    }
    Ok(())
}

/// Anti-dump hardening: lock .text in memory, exclude from core dumps,
/// disable dumpable flag, and zero ELF headers so tools like readelf/objdump
/// cannot parse the in-memory image. Best-effort, non-fatal on failure.
fn anti_dump_harden(elf_bytes: &[u8]) {
    unsafe { libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0); }

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
        unsafe { std::ptr::write_bytes(ptr, 0, 4); }
    }
    if elf_bytes.len() >= 48 && elf.header.e_shoff != 0 {
        let shoff_ptr = unsafe { ptr.add(40) };
        unsafe { std::ptr::write_bytes(shoff_ptr, 0, 8); }
    }
}

fn exe_path() -> Result<PathBuf> {
    std::env::current_exe().or_else(|_| {
        Path::new("/proc/self/exe")
            .canonicalize()
            .context("resolving /proc/self/exe")
    })
}

