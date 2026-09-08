use std::io::Write;
use std::process::{Command, Stdio};

use shell_ast::ShellError;
use shell_ir::BuiltinId;

use crate::{
    builtins,
    env::Env,
    status::ExitStatus,
    sys::redir::{RedirSet, RedirTargetSpec},
};

#[derive(Debug, Clone)]
pub enum PipelineStage {
    External {
        argv: Vec<String>,
        redirs: RedirSet,
    },
    Builtin {
        id: BuiltinId,
        args: Vec<String>,
        redirs: RedirSet,
    },
    /// An inline subshell stage: bytecode entry_ip..end_ip in the parent VM.
    /// Handled by vm.rs directly; run_pipeline never sees this variant.
    Subshell {
        entry_ip: usize,
        end_ip: usize,
    },
}

pub fn run_pipeline(
    segments: Vec<PipelineStage>,
    env: &mut Env,
    _redirs: Vec<crate::sys::redir::RedirSpec>,
    capture_out: bool,
) -> Result<(ExitStatus, Vec<u8>), ShellError> {
    if segments.is_empty() {
        return Ok((ExitStatus::OK, vec![]));
    }
    run_mixed_pipeline(segments, env, capture_out)
}

/// Exposed for use by vm.rs exec_pipeline (handles one Builtin stage with piped stdin).
pub fn run_builtin_stage_for_vm(
    id: BuiltinId,
    args: &[String],
    env: &mut Env,
    stdin_data: Option<&[u8]>,
    redirs: &RedirSet,
) -> Result<(ExitStatus, Vec<u8>), ShellError> {
    let result = run_builtin_stage(id, args, env, stdin_data, redirs)?;
    Ok((result.status, result.out))
}

/// Exposed for use by vm.rs pipeline executor.
pub fn run_external_stage_inline(
    argv: &[String],
    input: Option<&[u8]>,
    capture_output: bool,
    redirs: &RedirSet,
) -> Result<(ExitStatus, Vec<u8>), ShellError> {
    run_external_stage(argv, input, capture_output, redirs)
}

fn has_fd(redirs: &RedirSet, fd: u32) -> bool {
    redirs.specs.iter().any(|s| s.fd == fd)
}

fn run_mixed_pipeline(
    segments: Vec<PipelineStage>,
    env: &mut Env,
    capture_out: bool,
) -> Result<(ExitStatus, Vec<u8>), ShellError> {
    let mut stdin_buf: Option<Vec<u8>> = None;
    let mut last_status: ExitStatus = ExitStatus::OK;

    for (i, stage) in segments.iter().enumerate() {
        let is_last = i + 1 == segments.len();
        match stage {
            PipelineStage::External { argv, redirs } => {
                let capture_output = (!is_last || capture_out) && !has_fd(redirs, 1);
                let input = stdin_buf.as_deref().filter(|_| !has_fd(redirs, 0));
                let (status, out) = run_external_stage(argv, input, capture_output, redirs)?;
                last_status = status;
                stdin_buf = if capture_output { Some(out) } else { None };
            }
            PipelineStage::Builtin { id, args, redirs } => {
                // Pass upstream pipe data as stdin for builtins (fixes `read` in pipelines)
                let pipe_input = stdin_buf.as_deref().filter(|_| !has_fd(redirs, 0));
                let result = run_builtin_stage(*id, args, env, pipe_input, redirs)?;
                last_status = result.status;
                if has_fd(redirs, 1) {
                    let text = String::from_utf8_lossy(&result.out);
                    redirs.write_output(1, &text)?;
                    stdin_buf = None;
                } else {
                    stdin_buf = if !is_last || capture_out {
                        Some(result.out)
                    } else {
                        None
                    };
                }
            }
            PipelineStage::Subshell { .. } => {
                // Subshell stages are run inline by vm.rs exec_pipeline.
                unreachable!("Subshell stage must be dispatched by vm.rs");
            }
        } // end match stage
    } // end for loop

    let captured = if capture_out {
        stdin_buf.unwrap_or_default()
    } else {
        vec![]
    };
    Ok((last_status, captured))
}

fn run_external_stage(
    argv: &[String],
    input: Option<&[u8]>,
    capture_output: bool,
    redirs: &RedirSet,
) -> Result<(ExitStatus, Vec<u8>), ShellError> {
    if argv.is_empty() {
        return Ok((ExitStatus::OK, vec![]));
    }

    let (mut stdin_bytes, redirs_for_child) = redirs.split_stdin()?;
    if stdin_bytes.is_none() {
        stdin_bytes = input.map(|d| d.to_vec());
    }

    let mut cmd = Command::new(&argv[0]);
    cmd.args(&argv[1..]);
    redirs_for_child.apply_to_command(&mut cmd)?;

    if stdin_bytes.is_some() {
        cmd.stdin(Stdio::piped());
    }
    if capture_output {
        cmd.stdout(Stdio::piped());
    }

    let mut child = cmd
        .spawn()
        .map_err(|e| ShellError::IoError(format!("exec '{}': {}", argv[0], e)))?;

    if let Some(data) = stdin_bytes {
        if let Some(mut s) = child.stdin.take() {
            let _ = s.write_all(&data);
            drop(s);
        }
    }

    if capture_output {
        let out = child
            .wait_with_output()
            .map_err(|e| ShellError::IoError(format!("wait '{}': {}", argv[0], e)))?;
        Ok((
            ExitStatus::from_code(out.status.code().unwrap_or(1)),
            out.stdout,
        ))
    } else {
        let st = child
            .wait()
            .map_err(|e| ShellError::IoError(format!("wait '{}': {}", argv[0], e)))?;
        Ok((ExitStatus::from_code(st.code().unwrap_or(1)), vec![]))
    }
}

fn run_builtin_stage(
    id: BuiltinId,
    args: &[String],
    env: &mut Env,
    stdin_data: Option<&[u8]>, // data piped from previous stage
    redirs: &RedirSet,
) -> Result<crate::builtins::BuiltinResult, ShellError> {
    let result = match id {
        BuiltinId::Echo => builtins::echo::run(args),
        BuiltinId::Printf => builtins::printf::run(args),

        BuiltinId::Read => {
            // Prefer explicit file redirect, then heredoc, then upstream pipe data.
            let stdin_file =
                redirs
                    .specs
                    .iter()
                    .find(|r| r.fd == 0)
                    .and_then(|r| match &r.target {
                        RedirTargetSpec::File(p) => Some(p.clone()),
                        _ => None,
                    });
            let redir_data =
                redirs
                    .specs
                    .iter()
                    .find(|r| r.fd == 0)
                    .and_then(|r| match &r.target {
                        RedirTargetSpec::HereDoc(body) => Some(body.as_bytes().to_vec()),
                        _ => None,
                    });
            // Build a one-shot cursor from the available data.
            let raw = redir_data.or_else(|| stdin_data.map(|d| d.to_vec()));
            let mut opt_cursor = raw.map(std::io::Cursor::new);
            builtins::read::run(args, env, stdin_file, opt_cursor.as_mut())
        }

        BuiltinId::Test => builtins::test::run(args),

        BuiltinId::Sleep => builtins::sleep::run(&[args.first().cloned().unwrap_or_default()]),

        BuiltinId::Export => {
            let pairs: Vec<(String, String)> = args
                .chunks(2)
                .map(|c| (c[0].clone(), c.get(1).cloned().unwrap_or_default()))
                .collect();
            builtins::export::run_pairs(&pairs, env)
        }

        BuiltinId::Unset => builtins::unset::run(args, env),

        BuiltinId::Exit => {
            builtins::exit::run(&[args.first().cloned().unwrap_or_else(|| "0".to_string())])
        }

        BuiltinId::True => crate::builtins::BuiltinResult::ok(),
        BuiltinId::False => crate::builtins::BuiltinResult::fail(),
        BuiltinId::Colon => crate::builtins::BuiltinResult::ok(),
        BuiltinId::Local => {
            for arg in args {
                if let Some((name, value)) = arg.split_once('=') {
                    env.set_local(name, value);
                } else {
                    env.declare_local(arg);
                }
            }
            crate::builtins::BuiltinResult::ok()
        }
        BuiltinId::Set => {
            // set -- positionals: handled inline, no pipe output
            crate::builtins::BuiltinResult::ok()
        }
        BuiltinId::Wait => {
            std::thread::sleep(std::time::Duration::from_millis(50));
            crate::builtins::BuiltinResult::ok()
        }
        BuiltinId::Trap => crate::builtins::BuiltinResult::ok(),
        BuiltinId::Return => {
            let code = args
                .first()
                .and_then(|s| s.parse::<i32>().ok())
                .unwrap_or(0);
            crate::builtins::BuiltinResult::with_out(
                vec![],
                crate::status::ExitStatus::from_code(code),
            )
        }
        BuiltinId::CommandV => {
            let mut found = String::new();
            let mut status = ExitStatus::from_code(1);
            if let Some(flag_idx) = args.iter().position(|a| a.as_str() == "-v") {
                if let Some(name) = args.get(flag_idx + 1) {
                    let name = name.as_str();
                    if BuiltinId::from_name(name).is_some() {
                        found = name.to_string();
                        status = ExitStatus::OK;
                    } else if let Ok(out) = std::process::Command::new("which").arg(name).output() {
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
            crate::builtins::BuiltinResult::with_out(out, status)
        }
        BuiltinId::Exec => {
            // In a pipeline context exec with args runs the command; with no
            // args just apply fd redirections to the stage (fd>2 unsupported here).
            if args.is_empty() {
                crate::builtins::BuiltinResult::ok()
            } else {
                run_external_stage(args, stdin_data, false, redirs)
                    .map(|(st, _)| crate::builtins::BuiltinResult::with_out(vec![], st))
                    .unwrap_or_else(|_| crate::builtins::BuiltinResult::fail())
            }
        }
        // eval inside a pipeline stage runs in a child VM, which cannot
        // splice into the parent stream; run it as an external bash-free
        // subprocess is not possible either — treat like `:` with the args
        // consumed (bash itself runs pipeline eval in a subshell; the
        // side effects stay local to the stage either way).
        BuiltinId::Eval => crate::builtins::BuiltinResult::ok(),
        // shift in a pipeline stage affects the subshell's positionals —
        // nothing to do in the child, but don't fall through to "unknown".
        BuiltinId::Shift => crate::builtins::BuiltinResult::ok(),
    };
    Ok(result)
}
