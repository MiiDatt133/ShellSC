use super::BuiltinResult;
use crate::status::ExitStatus;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrapSignal {
    Exit,
    Err,
    Int,
    Term,
}

impl TrapSignal {
    pub fn from_name(s: &str) -> Option<Self> {
        match s.to_uppercase().as_str() {
            "EXIT" | "0" => Some(Self::Exit),
            "ERR" => Some(Self::Err),
            "INT" | "SIGINT" | "2" => Some(Self::Int),
            "TERM" | "SIGTERM" | "15" => Some(Self::Term),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrapDisposition {
    Default,
    Ignore,
    Command(String),
}

pub struct TrapAction {
    pub signal: TrapSignal,
    pub disposition: TrapDisposition,
}

pub fn run(args: &[String]) -> (BuiltinResult, Vec<TrapAction>) {
    if args.is_empty() {
        return (BuiltinResult::ok(), vec![]);
    }

    let cmd_arg = &args[0];
    let signals: Vec<&str> = if args.len() > 1 {
        args[1..].iter().map(|s| s.as_str()).collect()
    } else {
        vec!["EXIT"]
    };

    let disposition = if cmd_arg == "-" {
        TrapDisposition::Default
    } else if cmd_arg.is_empty() {
        TrapDisposition::Ignore
    } else {
        TrapDisposition::Command(cmd_arg.clone())
    };

    let mut actions = Vec::new();
    for sig_name in signals {
        if let Some(signal) = TrapSignal::from_name(sig_name) {
            actions.push(TrapAction {
                signal,
                disposition: disposition.clone(),
            });
        } else {
            let msg = format!("trap: {}: invalid signal specification\n", sig_name);
            return (
                BuiltinResult::with_out(msg.into_bytes(), ExitStatus::FAIL),
                vec![],
            );
        }
    }

    (BuiltinResult::ok(), actions)
}
