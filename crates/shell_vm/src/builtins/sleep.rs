use super::BuiltinResult;
use crate::status::ExitStatus;
use std::time::Duration;

pub fn run(args: &[String]) -> BuiltinResult {
    if args.is_empty() {
        return BuiltinResult {
            out: vec![],
            status: ExitStatus::FAIL,
        };
    }
    let raw = args[0].as_str();
    let (num_str, multiplier): (&str, f64) = if let Some(s) = raw.strip_suffix('s') {
        (s, 1.0)
    } else if let Some(s) = raw.strip_suffix('m') {
        (s, 60.0)
    } else if let Some(s) = raw.strip_suffix('h') {
        (s, 3600.0)
    } else if let Some(s) = raw.strip_suffix('d') {
        (s, 86400.0)
    } else {
        (raw, 1.0)
    };
    let secs: f64 = match num_str.parse() {
        Ok(v) => v,
        Err(_) => {
            return BuiltinResult {
                out: vec![],
                status: ExitStatus::FAIL,
            }
        }
    };
    if secs >= 0.0 {
        std::thread::sleep(Duration::from_secs_f64(secs * multiplier));
    }
    BuiltinResult::ok()
}
