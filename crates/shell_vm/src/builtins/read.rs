use std::io::{BufRead, Cursor};

use super::BuiltinResult;
use crate::env::Env;

pub fn run(
    args: &[String],
    env: &mut Env,
    stdin_file: Option<String>,
    stdin_cursor: Option<&mut Cursor<Vec<u8>>>,
) -> BuiltinResult {
    let mut read_array = false;
    let mut rest = args;
    while let Some(flag) = rest.first() {
        match flag.as_str() {
            "-r" => rest = &rest[1..],
            "-a" => {
                read_array = true;
                rest = &rest[1..];
            }
            // Combined short flags like -ra
            f if f.len() > 2
                && f.starts_with('-')
                && f[1..].chars().all(|c| c == 'r' || c == 'a') =>
            {
                if f.contains('a') {
                    read_array = true;
                }
                rest = &rest[1..];
            }
            _ => break,
        }
    }
    let var_args = rest;

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

    // `read -a arr`: split the whole line on IFS into array elements.
    if read_array {
        let name = var_args[0].clone();
        let ifs = env.get("IFS").unwrap_or(" \t\n").to_string();
        let fields: Vec<String> = if ifs.is_empty() {
            vec![line.clone()]
        } else {
            line.split(|c: char| ifs.contains(c))
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .collect()
        };
        env.set_array(&name, fields);
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
