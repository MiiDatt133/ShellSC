#[path = "../src/raw.rs"]
mod raw;

fn main() {
    unsafe {
        eprintln!("test openat/status…");
        let s = raw::raw_open_read("/proc/self/status");
        eprintln!("status: {} bytes", s.as_ref().map(|v| v.len()).unwrap_or(0));
        let m = raw::raw_open_read("/proc/self/maps");
        eprintln!("maps: {} bytes", m.as_ref().map(|v| v.len()).unwrap_or(0));
        let e = raw::raw_open_read("/proc/self/environ");
        eprintln!(
            "environ: {} bytes",
            e.as_ref().map(|v| v.len()).unwrap_or(0)
        );
        eprintln!("getppid: {}", raw::raw_getppid());
        let mut fds = [0i32; 2];
        eprintln!("pipe: {}", raw::raw_pipe(&mut fds));
        eprintln!("done");
    }
}
