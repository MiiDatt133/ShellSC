use std::io::Write;
use std::process::{Command, Stdio};

use shell_ast::ShellError;

use crate::{status::ExitStatus, sys::redir::RedirSet};

pub fn spawn_command(argv: &[String], redirs: RedirSet) -> Result<ExitStatus, ShellError> {
    if argv.is_empty() {
        return Ok(ExitStatus::OK);
    }

    let (stdin_bytes, redirs) = redirs.split_stdin()?;

    let mut cmd = Command::new(&argv[0]);
    cmd.args(&argv[1..]);
    redirs.apply_to_command(&mut cmd)?;

    if stdin_bytes.is_some() {
        cmd.stdin(Stdio::piped());
    }

    let mut child = cmd
        .spawn()
        .map_err(|e| ShellError::IoError(format!("exec '{}': {}", argv[0], e)))?;

    if let Some(data) = stdin_bytes {
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(&data);
        }
    }

    let status = child
        .wait()
        .map_err(|e| ShellError::IoError(format!("wait child '{}': {}", argv[0], e)))?;
    Ok(ExitStatus::from_code(status.code().unwrap_or(1)))
}

/// Spawn a background process and immediately reap it in a detached thread
/// so no zombie accumulates.  The child's stdin data (if any) is written
/// before the thread lets the process run freely.
pub fn spawn_background(argv: &[String], redirs: RedirSet) -> Result<(), ShellError> {
    if argv.is_empty() {
        return Ok(());
    }

    let (stdin_bytes, redirs) = redirs.split_stdin()?;

    let mut cmd = Command::new(&argv[0]);
    cmd.args(&argv[1..]);
    redirs.apply_to_command(&mut cmd)?;

    if stdin_bytes.is_some() {
        cmd.stdin(Stdio::piped());
    }

    let mut child = cmd
        .spawn()
        .map_err(|e| ShellError::IoError(format!("exec '{}': {}", argv[0], e)))?;

    if let Some(data) = stdin_bytes {
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(&data);
            // Drop stdin so the child sees EOF and doesn't block
        }
    }

    // Reap in a background thread — prevents zombie accumulation.
    // `Child` is `Send`, so this is safe.
    std::thread::spawn(move || {
        let _ = child.wait();
    });

    Ok(())
}
