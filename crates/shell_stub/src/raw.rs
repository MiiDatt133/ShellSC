//! Raw-syscall helpers: read security-critical proc files and do
//! fork/ptrace plumbing without going through libc symbol dispatch.
//!
//! Why: every libc wrapper (`open`, `read`, `fork`, `ptrace`, ...) is a
//! interposable symbol. A preload library can unsetenv LD_PRELOAD in its
//! constructor and then interpose the wrappers so the stub's checks read
//! a filtered /proc. Raw `syscall` instructions have no symbol to
//! interpose — only a kernel patch (or ptrace, which self-debug blocks)
//! can fake their result.
//!
//! The ELF is loaded by the kernel before any constructor runs, so the
//! syscall instruction itself is already mapped when these run.

// ── Syscall numbers per architecture ───────────────────────────────────────

#[cfg(target_arch = "aarch64")]
mod nr {
    pub const READ: i64 = 63;
    pub const READV: i64 = 65;
    pub const OPENAT: i64 = 56;
    pub const CLOSE: i64 = 57;
    pub const EXIT: i64 = 93;
    pub const EXIT_GROUP: i64 = 94;
    pub const GETPPID: i64 = 173;
    pub const WRITE: i64 = 64;
    pub const PIPE: i64 = 59;
    pub const CLONE: i64 = 220;
    pub const WAIT4: i64 = 260;
    pub const PTRACE: i64 = 117;
}

#[cfg(target_arch = "arm")]
mod nr {
    pub const READ: i64 = 3;
    pub const READV: i64 = 145;
    pub const OPENAT: i64 = 322;
    pub const CLOSE: i64 = 6;
    pub const EXIT: i64 = 1;
    pub const EXIT_GROUP: i64 = 248;
    pub const GETPPID: i64 = 64;
    pub const WRITE: i64 = 4;
    pub const PIPE: i64 = 42;
    pub const FORK: i64 = 2;
    pub const CLONE: i64 = 120;
    pub const WAIT4: i64 = 114;
    pub const PTRACE: i64 = 26;
}

#[cfg(target_arch = "x86_64")]
mod nr {
    pub const READ: i64 = 0;
    pub const READV: i64 = 19;
    pub const OPENAT: i64 = 257;
    pub const CLOSE: i64 = 3;
    pub const EXIT: i64 = 60;
    pub const EXIT_GROUP: i64 = 231;
    pub const GETPPID: i64 = 110;
    pub const WRITE: i64 = 1;
    pub const PIPE: i64 = 22;
    pub const FORK: i64 = 57;
    pub const CLONE: i64 = 56;
    pub const WAIT4: i64 = 61;
    pub const PTRACE: i64 = 101;
}

#[cfg(target_arch = "x86")]
mod nr {
    pub const READ: i64 = 3;
    pub const READV: i64 = 145;
    pub const OPENAT: i64 = 295;
    pub const CLOSE: i64 = 6;
    pub const EXIT: i64 = 1;
    pub const EXIT_GROUP: i64 = 252;
    pub const GETPPID: i64 = 64;
    pub const WRITE: i64 = 4;
    pub const PIPE: i64 = 42;
    pub const FORK: i64 = 2;
    pub const CLONE: i64 = 120;
    pub const WAIT4: i64 = 114;
    pub const PTRACE: i64 = 26;
}

// ── Syscall dispatch ────────────────────────────────────────────────────────

#[cfg(target_arch = "aarch64")]
#[inline(always)]
pub unsafe fn syscall5(n: i64, a: usize, b: usize, c: usize, d: usize, e: usize) -> i64 {
    let ret;
    core::arch::asm!(
        "svc #0",
        in("x8") n,
        in("x0") a, in("x1") b, in("x2") c, in("x3") d, in("x4") e,
        lateout("x0") ret,
        options(nostack)
    );
    ret
}

// aarch32 EABI syscall: number in r7, args in r0-r5. r7 is the frame
// pointer under Thumb so Rust refuses to name it directly; shuffle
// registers inside a naked function instead.
//
// C ABI on entry: r0 = syscall nr, r1..r3 = args 0..2, [sp+0]/[sp+4]
// (after the call pushes the return address) = args 3/4.
#[cfg(target_arch = "arm")]
#[unsafe(naked)]
unsafe extern "C" fn syscall5_arm(
    _n: usize,
    _a: usize,
    _b: usize,
    _c: usize,
    _d: usize,
    _e: usize,
) -> usize {
    core::arch::naked_asm!(
        "push {{r4, r7}}",
        "mov r7, r0",
        "mov r0, r1",
        "mov r1, r2",
        "mov r2, r3",
        "ldr r3, [sp, #12]",
        "ldr r4, [sp, #16]",
        "svc #0",
        "pop {{r4, r7}}",
        "bx lr",
    );
}

#[cfg(target_arch = "arm")]
#[inline(always)]
pub unsafe fn syscall5(n: i64, a: usize, b: usize, c: usize, d: usize, e: usize) -> i64 {
    // The kernel returns -errno in r0; coming back through a 32-bit
    // usize would leave it a huge positive value, so sign-extend.
    syscall5_arm(n as usize, a, b, c, d, e) as i32 as i64
}

#[cfg(target_arch = "x86_64")]
#[inline(always)]
pub unsafe fn syscall5(n: i64, a: usize, b: usize, c: usize, d: usize, e: usize) -> i64 {
    let ret;
    core::arch::asm!(
        "syscall",
        in("rax") n,
        in("rdi") a, in("rsi") b, in("rdx") c, in("r10") d, in("r8") e,
        lateout("rax") ret,
        options(nostack)
    );
    ret
}

// i386 syscall: number in eax, args in ebx,ecx,edx,esi,edi. LLVM
// reserves esi internally on 32-bit and rejects naming it in inline
// asm, so shuffle registers inside a naked function. PIC builds also
// reserve ebx — save/restore it explicitly.
//
// C ABI on entry: [sp+0] = return address, [sp+4..] = args 0..4.
#[cfg(target_arch = "x86")]
#[unsafe(naked)]
unsafe extern "C" fn syscall5_x86(
    _n: usize,
    _a: usize,
    _b: usize,
    _c: usize,
    _d: usize,
    _e: usize,
) -> usize {
    core::arch::naked_asm!(
        "push %ebp",
        "mov %esp, %ebp",
        "push %ebx",
        "push %esi",
        "mov 8(%ebp), %eax",
        "mov 12(%ebp), %ebx",
        "mov 16(%ebp), %ecx",
        "mov 20(%ebp), %edx",
        "mov 24(%ebp), %esi",
        "mov 28(%ebp), %edi",
        "int $0x80",
        "pop %esi",
        "pop %ebx",
        "pop %ebp",
        "ret",
    );
}

#[cfg(target_arch = "x86")]
#[inline(always)]
pub unsafe fn syscall5(n: i64, a: usize, b: usize, c: usize, d: usize, e: usize) -> i64 {
    // The kernel returns -errno in eax; sign-extend the 32-bit result.
    syscall5_x86(n as usize, a, b, c, d, e) as i32 as i64
}

// ── Thin wrappers ──────────────────────────────────────────────────────────

const O_RDONLY: usize = 0;
const AT_FDCWD: i64 = -100;
const SIGCHLD: usize = 17;

pub unsafe fn raw_open_read(path: &str) -> Option<Vec<u8>> {
    let mut c = path.as_bytes().to_vec();
    c.push(0);
    let fd = syscall5(
        nr::OPENAT,
        AT_FDCWD as usize,
        c.as_ptr() as usize,
        O_RDONLY,
        0,
        0,
    );
    if fd < 0 {
        return None;
    }
    // /proc files are small; a 1 MiB buffer covers maps on huge processes.
    let mut buf = vec![0u8; 1 << 20];
    let mut off = 0usize;
    loop {
        let r = syscall5(
            nr::READ,
            fd as usize,
            buf.as_mut_ptr().add(off) as usize,
            buf.len() - off,
            0,
            0,
        );
        if r <= 0 {
            break;
        }
        off += r as usize;
        if off == buf.len() {
            buf.resize(buf.len() * 2, 0);
        }
    }
    syscall5(nr::CLOSE, fd as usize, 0, 0, 0, 0);
    if off == 0 {
        return None;
    }
    buf.truncate(off);
    Some(buf)
}

pub unsafe fn raw_read(fd: usize, buf: &mut [u8]) -> i64 {
    syscall5(nr::READ, fd, buf.as_mut_ptr() as usize, buf.len(), 0, 0)
}

pub unsafe fn raw_write(fd: usize, buf: &[u8]) -> i64 {
    syscall5(nr::WRITE, fd, buf.as_ptr() as usize, buf.len(), 0, 0)
}

pub unsafe fn raw_close(fd: usize) {
    syscall5(nr::CLOSE, fd, 0, 0, 0, 0);
}

pub unsafe fn raw_exit_group(code: i32) -> ! {
    syscall5(nr::EXIT_GROUP, code as usize, 0, 0, 0, 0);
    unreachable!("exit_group returned")
}

pub unsafe fn raw_getppid() -> i32 {
    syscall5(nr::GETPPID, 0, 0, 0, 0, 0) as i32
}

pub unsafe fn raw_pipe(fds: &mut [i32; 2]) -> i64 {
    syscall5(nr::PIPE, fds.as_mut_ptr() as usize, 0, 0, 0, 0)
}

/// fork(2) with the classic child-return-0 ABI.
pub unsafe fn raw_fork() -> i64 {
    // clone(SIGCHLD, 0, ...) == fork() semantics on all four arches.
    syscall5(nr::CLONE, SIGCHLD, 0, 0, 0, 0)
}

pub unsafe fn raw_ptrace(request: i64, pid: i32, addr: usize, data: usize) -> i64 {
    syscall5(nr::PTRACE, request as usize, pid as usize, addr, data, 0)
}

pub unsafe fn raw_waitpid(pid: i32, status: &mut i32) -> i64 {
    syscall5(
        nr::WAIT4,
        pid as usize,
        status as *mut i32 as usize,
        0,
        0,
        0,
    )
}

// ── Kernel-ABI constants (matches linux/ptrace.h, linux/signal.h) ──────────

pub const PTRACE_CONT: i64 = 7;
pub const PTRACE_ATTACH: i64 = 16;
pub const SIG_TRAP: i32 = 5;
pub const SIG_STOP: i32 = 19;

/// wait(2) status macros in pure arithmetic — no libc macros to interpose.
pub fn wif_stopped(status: i32) -> bool {
    (status & 0xFF) == 0x7F
}
pub fn wif_exited(status: i32) -> bool {
    (status & 0x7F) == 0
}
pub fn wif_signaled(status: i32) -> bool {
    let t = (status & 0x7F) as u8;
    t != 0x7F && t != 0
}
pub fn w_stop_sig(status: i32) -> i32 {
    (status >> 8) & 0xFF
}
