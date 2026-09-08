use std::io::{BufRead, Cursor};

use super::BuiltinResult;
use crate::env::Env;

pub fn run(
    args: &[String],
    env: &mut Env,
    stdin_file: Option<String>,
    stdin_cursor: Option<&mut Cursor<Vec<u8>>>,
) -> BuiltinResult {
    let (_, var_args) = if args.first().map(|s| s.as_str()) == Some("-r") {
        (true, &args[1..])
    } else {
        (false, &args[..])
    };

    let mut line = String::new();

    let ok = if let Some(path) = stdin_file {
        match std::fs::File::open(&path) {
            Ok(f) => {
                let mut reader = std::io::BufReader::new(f);
                reader.read_line(&mut line).map(|n| n > 0).unwrap_or(false)
            }
            Err(_) => false,
        }
    } else if let Some(cursor) = stdin_cursor {
        // Read one line from the cursor; position advances automatically.
        cursor.read_line(&mut line).map(|n| n > 0).unwrap_or(false)
    } else {
        let stdin = std::io::stdin();
        stdin
            .lock()
            .read_line(&mut line)
            .map(|n| n > 0)
            .unwrap_or(false)
    };

    if !ok {
        return BuiltinResult::fail();
    }
    if line.ends_with('\n') {
        line.pop();
        if line.ends_with('\r') {
            line.pop();
        }
    }
    if var_args.is_empty() {
        return BuiltinResult::ok();
    }

    let ifs = env.get("IFS").unwrap_or(" \t\n").to_string();

    let fields: Vec<String> = if var_args.len() == 1 {
        vec![line.clone()]
    } else {
        line.splitn(var_args.len(), |c| ifs.contains(c))
            .map(|s| s.trim_start_matches(|c: char| ifs.contains(c)).to_string())
            .collect()
    };

    for (i, var) in var_args.iter().enumerate() {
        let val = fields.get(i).map(|s| s.as_str()).unwrap_or("");
        env.set(var.as_str(), val);
    }
    BuiltinResult::ok()
}
