use std::io::{BufRead, Cursor};

use super::BuiltinResult;
use crate::env::Env;

pub fn run(
    args: &[String],
    env: &mut Env,
    stdin_file: Option<String>,
    mut stdin_cursor: Option<&mut Cursor<Vec<u8>>>,
) -> BuiltinResult {
    let mut read_array = false;
    let mut raw_mode = false;
    let mut rest = args;
    while let Some(flag) = rest.first() {
        match flag.as_str() {
            "-r" => {
                raw_mode = true;
                rest = &rest[1..];
            }
            "-a" => {
                read_array = true;
                rest = &rest[1..];
            }
            f if f.len() > 2
                && f.starts_with('-')
                && f[1..].chars().all(|c| c == 'r' || c == 'a') =>
            {
                if f.contains('a') {
                    read_array = true;
                }
                if f.contains('r') {
                    raw_mode = true;
                }
                rest = &rest[1..];
            }
            _ => break,
        }
    }
    let var_args = rest;

    let mut line = String::new();

    let n_read = if let Some(path) = stdin_file {
        match std::fs::File::open(&path) {
            Ok(f) => {
                let mut reader = std::io::BufReader::new(f);
                reader.read_line(&mut line).unwrap_or(0)
            }
            Err(_) => return BuiltinResult::fail(),
        }
    } else if let Some(cursor) = stdin_cursor.as_deref_mut() {
        cursor.read_line(&mut line).unwrap_or(0)
    } else {
        let stdin = std::io::stdin();
        stdin.lock().read_line(&mut line).unwrap_or(0)
    };

    if n_read == 0 {
        return BuiltinResult::fail();
    }
    let eof_without_newline = !line.ends_with('\n');
    if line.ends_with('\n') {
        line.pop();
        if line.ends_with('\r') {
            line.pop();
        }
    }

    const ESC_MARK: char = '\u{1}';
    let mut has_marks = false;
    if !raw_mode {
        let mut processed = String::with_capacity(line.len());
        let mut queue: std::collections::VecDeque<char> = line.chars().collect();
        while let Some(c) = queue.pop_front() {
            if c == '\\' {
                match queue.pop_front() {
                    Some('\n') => {
                        let mut extra = String::new();
                        let read_more = match stdin_cursor.as_deref_mut() {
                            Some(cur) => cur.read_line(&mut extra).unwrap_or(0) > 0,
                            None => false,
                        };
                        if !read_more {
                            break;
                        }
                        if extra.ends_with('\n') {
                            extra.pop();
                            if extra.ends_with('\r') {
                                extra.pop();
                            }
                        }
                        for ec in extra.chars().rev() {
                            queue.push_front(ec);
                        }
                    }
                    Some(next) => {
                        processed.push(ESC_MARK);
                        processed.push(next);
                        has_marks = true;
                    }
                    None => break,
                }
            } else {
                processed.push(c);
            }
        }
        line = processed;
    }

    if var_args.is_empty() {
        if eof_without_newline {
            return BuiltinResult::fail();
        }
        return BuiltinResult::ok();
    }

    let ifs = env.get("IFS").unwrap_or(" \t\n").to_string();
    let is_marked = |bytes: &[u8], i: usize| i > 0 && bytes[i - 1] == ESC_MARK as u8;
    let split_point = |bytes: &[u8], i: usize| -> bool {
        if ifs.is_empty() {
            return false;
        }
        let c = bytes[i] as char;
        ifs.contains(c) && !is_marked(bytes, i)
    };

    let strip_marks = |s: &str| -> String {
        if has_marks {
            s.chars().filter(|c| *c != ESC_MARK).collect()
        } else {
            s.to_string()
        }
    };

    if read_array {
        let name = var_args[0].clone();
        let bytes = line.as_bytes();
        let mut fields: Vec<String> = Vec::new();
        let mut cur = String::new();
        let mut idx = 0;
        while idx < bytes.len() {
            if split_point(bytes, idx) {
                if !cur.is_empty() {
                    fields.push(strip_marks(&cur));
                    cur.clear();
                }
            } else {
                cur.push(bytes[idx] as char);
            }
            idx += 1;
        }
        if !cur.is_empty() {
            fields.push(strip_marks(&cur));
        }
        env.set_array(&name, fields);
        if eof_without_newline {
            return BuiltinResult::fail();
        }
        return BuiltinResult::ok();
    }

    let ifs_ws = |c: char| c == ' ' || c == '\t' || c == '\n';

    let fields: Vec<String> = if var_args.len() == 1 {
        let mut w = &line[..];
        if !ifs.is_empty() {
            let only_ws = ifs.chars().all(ifs_ws);
            if only_ws {
                w = w.trim_matches(ifs_ws);
            }
        } else {
            w = w.trim_matches(ifs_ws);
        }
        vec![strip_marks(w)]
    } else {
        let take = var_args.len() - 1;
        let mut rest_raw = line.trim_matches(ifs_ws).to_string();
        let mut head: Vec<String> = Vec::new();
        for _ in 0..take {
            if rest_raw.is_empty() {
                head.push(String::new());
                continue;
            }
            let rb = rest_raw.as_bytes();
            let mut pos = None;
            for i in 0..rb.len() {
                if split_point(rb, i) {
                    pos = Some(i);
                    break;
                }
            }
            match pos {
                Some(p) => {
                    let f: String = rest_raw[..p].to_string();
                    let mut after = p;
                    let ab = rest_raw.as_bytes();
                    let mut took_nonws = false;
                    while after < ab.len() {
                        let c = ab[after] as char;
                        if ifs.is_empty() || !ifs.contains(c) || is_marked(ab, after) {
                            break;
                        }
                        if ifs_ws(c) {
                            after += 1;
                        } else if !took_nonws {
                            took_nonws = true;
                            after += 1;
                        } else {
                            break;
                        }
                    }
                    rest_raw = rest_raw[after..].to_string();
                    head.push(f);
                }
                None => {
                    head.push(rest_raw.clone());
                    rest_raw.clear();
                }
            }
        }
        let rbytes = rest_raw.as_bytes();
        let mut cut = rbytes.len();
        while cut > 0 {
            let c = rbytes[cut - 1] as char;
            if ifs_ws(c) && ifs.contains(c) {
                cut -= 1;
            } else {
                break;
            }
        }
        let has_internal_delim =
            rbytes[..cut.saturating_sub(1)]
                .iter()
                .enumerate()
                .any(|(i, &b)| {
                    !ifs.is_empty()
                        && ifs.contains(b as char)
                        && !ifs_ws(b as char)
                        && !is_marked(rbytes, i)
                });
        if !has_internal_delim
            && cut > 0
            && !ifs.is_empty()
            && ifs.contains(rbytes[cut - 1] as char)
            && !ifs_ws(rbytes[cut - 1] as char)
            && !is_marked(rbytes, cut - 1)
        {
            cut -= 1;
        }
        rest_raw.truncate(cut);
        head.push(rest_raw);
        head.iter().map(|s| strip_marks(s)).collect()
    };

    for (i, var) in var_args.iter().enumerate() {
        let val = fields.get(i).map(|s| s.as_str()).unwrap_or("");
        env.set(var.as_str(), val);
    }
    if eof_without_newline {
        return BuiltinResult::fail();
    }
    BuiltinResult::ok()
}
