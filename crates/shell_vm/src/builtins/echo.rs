use super::BuiltinResult;

pub fn run(args: &[String]) -> BuiltinResult {
    let mut no_newline = false;
    let mut interpret = false;
    let mut i = 0usize;

    while i < args.len() {
        let a = args[i].as_str();
        if a == "--" {
            i += 1;
            break;
        } else if a == "-n" {
            no_newline = true;
        } else if a == "-e" {
            interpret = true;
        } else if a == "-E" {
            interpret = false;
        } else {
            break;
        }
        i += 1;
    }

    let joined = args[i..].join(" ");
    let text = if interpret {
        process_escapes(&joined)
    } else {
        joined
    };
    let mut out = text.into_bytes();
    if !no_newline {
        out.push(b'\n');
    }
    BuiltinResult {
        out,
        status: crate::status::ExitStatus::OK,
    }
}

fn process_escapes(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('\\') => out.push('\\'),
            Some('a') => out.push('\x07'),
            Some('b') => out.push('\x08'),
            Some('f') => out.push('\x0c'),
            Some('v') => out.push('\x0b'),
            Some('0') => {
                let mut val: u32 = 0;
                for _ in 0..2 {
                    match chars.clone().next() {
                        Some(d @ '0'..='7') => {
                            val = val * 8 + d.to_digit(8).unwrap();
                            chars.next();
                        }
                        _ => break,
                    }
                }
                val *= 8;
                if let Some(ch) = char::from_u32(val.min(0o377)) {
                    out.push(ch);
                }
            }
            Some(c2) => {
                out.push('\\');
                out.push(c2);
            }
            None => out.push('\\'),
        }
    }
    out
}
