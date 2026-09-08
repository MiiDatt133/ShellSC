use crate::{
    status::ExitStatus,
    sys::{proc::spawn_command, redir::RedirSet},
};
use shell_ast::ShellError;

pub fn exec_external(argv: &[String], redirs: RedirSet) -> Result<ExitStatus, ShellError> {
    spawn_command(argv, redirs)
}
