use super::BuiltinResult;
use crate::status::ExitStatus;

pub fn run(args: &[String]) -> BuiltinResult {
    if args.is_empty() {
        return BuiltinResult::ok();
    }
    // POSIX printf: the format is reused as needed to consume all arguments.
    let fmt = &args[0];
    let vals = &args[1..];
    let mut out = String::new();
    let mut idx = 0usize;
    loop {
        let consumed_before = out.len();
        let used = format_printf_into(&mut out, fmt, vals, idx);
        let more = idx + used < vals.len();
        idx += used;
        if !more {
            break;
        }
        // Guard against a format that consumes nothing (all %s consumed 0 args
        // already accounted): avoid infinite loop.
        if used == 0 {
            break;
        }
        let _ = consumed_before;
    }
    BuiltinResult {
        out: out.into_bytes(),
        status: ExitStatus::OK,
    }
}

/// Formats one pass of `fmt` against `vals` starting at `val_idx`.
/// Returns how many values were consumed.
fn format_printf_into(out: &mut String, fmt: &str, vals: &[String], val_idx: usize) -> usize {
    let mut chars = fmt.chars().peekable();
    let mut used = 0usize;
    let mut local_idx = val_idx;
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('r') => out.push('\r'),
                Some('\\') => out.push('\\'),
                Some(c2) => {
                    out.push('\\');
                    out.push(c2);
                }
                None => out.push('\\'),
            },
            '%' => match chars.next() {
                Some('%') => out.push('%'),
                Some(spec)
                    if spec == '-'
                        || spec == '+'
                        || spec == ' '
                        || spec == '0'
                        || spec == '#'
                        || spec.is_ascii_digit()
                        || spec == '.' =>
                {
                    // Parse flags / width / precision, then the conversion.
                    let mut flags = String::new();
                    let mut cur = spec;
                    loop {
                        if matches!(cur, '-' | '+' | ' ' | '0' | '#') {
                            flags.push(cur);
                            cur = match chars.next() {
                                Some(c2) => c2,
                                None => break,
                            };
                        } else {
                            break;
                        }
                    }
                    let mut width_str = String::new();
                    while cur.is_ascii_digit() {
                        width_str.push(cur);
                        cur = match chars.next() {
                            Some(c2) => c2,
                            None => break,
                        };
                    }
                    let mut prec: Option<usize> = None;
                    if cur == '.' {
                        let mut p = String::new();
                        cur = match chars.next() {
                            Some(c2) => c2,
                            None => break,
                        };
                        while cur.is_ascii_digit() {
                            p.push(cur);
                            cur = match chars.next() {
                                Some(c2) => c2,
                                None => break,
                            };
                        }
                        prec = Some(p.parse().unwrap_or(0));
                    }
                    let left = flags.contains('-');
                    let width: usize = width_str.parse().unwrap_or(0);
                    let mut piece = String::new();
                    match cur {
                        's' => {
                            let s = vals.get(local_idx).map(|s| s.as_str()).unwrap_or("");
                            local_idx += 1;
                            used += 1;
                            let s = match prec {
                                Some(p) => &s[..s.len().min(p)],
                                None => s,
                            };
                            if width > s.len() {
                                let pad = " ".repeat(width - s.len());
                                if left {
                                    piece.push_str(s);
                                    piece.push_str(&pad);
                                } else {
                                    piece.push_str(&pad);
                                    piece.push_str(s);
                                }
                            } else {
                                piece.push_str(s);
                            }
                        }
                        'd' | 'i' => {
                            let n: i64 = vals
                                .get(local_idx)
                                .and_then(|s| s.parse().ok())
                                .unwrap_or(0);
                            local_idx += 1;
                            used += 1;
                            let mut body = n.to_string();
                            if let Some(p) = prec {
                                if body.len() < p && n >= 0 {
                                    body = format!("{}{}", "0".repeat(p - body.len()), body);
                                }
                            }
                            if width > body.len() {
                                let pad_char = if flags.contains('0') && !left {
                                    '0'
                                } else {
                                    ' '
                                };
                                let pad: String = std::iter::repeat(pad_char)
                                    .take(width - body.len())
                                    .collect();
                                if left {
                                    piece.push_str(&body);
                                    piece.push_str(&pad);
                                } else {
                                    piece.push_str(&pad);
                                    piece.push_str(&body);
                                }
                            } else {
                                piece.push_str(&body);
                            }
                        }
                        'x' | 'X' => {
                            let n: i64 = vals
                                .get(local_idx)
                                .and_then(|s| s.parse().ok())
                                .unwrap_or(0);
                            local_idx += 1;
                            used += 1;
                            let mut body = if cur == 'X' {
                                format!("{:X}", n)
                            } else {
                                format!("{:x}", n)
                            };
                            if let Some(p) = prec {
                                if body.len() < p {
                                    body = format!("{}{}", "0".repeat(p - body.len()), body);
                                }
                            }
                            if width > body.len() {
                                let pad: String =
                                    std::iter::repeat(' ').take(width - body.len()).collect();
                                if left {
                                    piece.push_str(&body);
                                    piece.push_str(&pad);
                                } else {
                                    piece.push_str(&pad);
                                    piece.push_str(&body);
                                }
                            } else {
                                piece.push_str(&body);
                            }
                        }
                        'o' => {
                            let n: i64 = vals
                                .get(local_idx)
                                .and_then(|s| s.parse().ok())
                                .unwrap_or(0);
                            local_idx += 1;
                            used += 1;
                            let mut body = format!("{:o}", n);
                            if let Some(p) = prec {
                                if body.len() < p {
                                    body = format!("{}{}", "0".repeat(p - body.len()), body);
                                }
                            }
                            if width > body.len() {
                                let pad: String =
                                    std::iter::repeat(' ').take(width - body.len()).collect();
                                if left {
                                    piece.push_str(&body);
                                    piece.push_str(&pad);
                                } else {
                                    piece.push_str(&pad);
                                    piece.push_str(&body);
                                }
                            } else {
                                piece.push_str(&body);
                            }
                        }
                        'b' => {
                            let s = vals.get(local_idx).map(|s| s.as_str()).unwrap_or("");
                            local_idx += 1;
                            used += 1;
                            let expanded = expand_backslash_escapes(s);
                            if width > expanded.len() {
                                let pad = " ".repeat(width - expanded.len());
                                if left {
                                    piece.push_str(&expanded);
                                    piece.push_str(&pad);
                                } else {
                                    piece.push_str(&pad);
                                    piece.push_str(&expanded);
                                }
                            } else {
                                piece.push_str(&expanded);
                            }
                        }
                        'f' => {
                            let f: f64 = vals
                                .get(local_idx)
                                .and_then(|s| s.parse().ok())
                                .unwrap_or(0.0);
                            local_idx += 1;
                            used += 1;
                            piece.push_str(&format!("{:.*}", prec.unwrap_or(6), f));
                        }
                        other => {
                            out.push('%');
                            if !flags.is_empty() {
                                out.push_str(&flags);
                            }
                            if !width_str.is_empty() {
                                out.push_str(&width_str);
                            }
                            if let Some(p) = prec {
                                out.push('.');
                                out.push_str(&p.to_string());
                            }
                            out.push(other);
                            continue;
                        }
                    }
                    out.push_str(&piece);
                }
                Some('s') => {
                    out.push_str(vals.get(local_idx).map(|s| s.as_str()).unwrap_or(""));
                    local_idx += 1;
                    used += 1;
                }
                Some('d') | Some('i') => {
                    let n: i64 = vals
                        .get(local_idx)
                        .and_then(|s| s.parse().ok())
                        .unwrap_or(0);
                    out.push_str(&n.to_string());
                    local_idx += 1;
                    used += 1;
                }
                Some('X') => {
                    let n: i64 = vals
                        .get(local_idx)
                        .and_then(|s| s.parse().ok())
                        .unwrap_or(0);
                    out.push_str(&format!("{:X}", n));
                    local_idx += 1;
                    used += 1;
                }
                Some('x') => {
                    let n: i64 = vals
                        .get(local_idx)
                        .and_then(|s| s.parse().ok())
                        .unwrap_or(0);
                    out.push_str(&format!("{:x}", n));
                    local_idx += 1;
                    used += 1;
                }
                Some('o') => {
                    let n: i64 = vals
                        .get(local_idx)
                        .and_then(|s| s.parse().ok())
                        .unwrap_or(0);
                    out.push_str(&format!("{:o}", n));
                    local_idx += 1;
                    used += 1;
                }
                Some('b') => {
                    let s = vals.get(local_idx).map(|s| s.as_str()).unwrap_or("");
                    out.push_str(&expand_backslash_escapes(s));
                    local_idx += 1;
                    used += 1;
                }
                Some('f') => {
                    let f: f64 = vals
                        .get(local_idx)
                        .and_then(|s| s.parse().ok())
                        .unwrap_or(0.0);
                    out.push_str(&format!("{:.6}", f));
                    local_idx += 1;
                    used += 1;
                }
                Some('c') => {
                    if let Some(s) = vals.get(local_idx) {
                        if let Some(ch) = s.chars().next() {
                            out.push(ch);
                        }
                    }
                    local_idx += 1;
                    used += 1;
                }
                Some(c2) => {
                    out.push('%');
                    out.push(c2);
                }
                None => out.push('%'),
            },
            c => out.push(c),
        }
    }
    used
}

fn expand_backslash_escapes(s: &str) -> String {
    let mut out = String::new();
    let bytes = s.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'\\' {
            i += 1;
            if i >= bytes.len() {
                out.push('\\');
                break;
            }
            match bytes[i] {
                b'n' => out.push('\n'),
                b't' => out.push('\t'),
                b'r' => out.push('\r'),
                b'\\' => out.push('\\'),
                b'a' => out.push('\u{07}'),
                b'b' => out.push('\u{08}'),
                b'f' => out.push('\u{0C}'),
                b'v' => out.push('\u{0B}'),
                b'0' => {
                    let mut octal = String::new();
                    let mut j = i + 1;
                    for _ in 0..3 {
                        if j < bytes.len() && bytes[j].is_ascii_digit() && bytes[j] < b'8' {
                            octal.push(bytes[j] as char);
                            j += 1;
                        } else {
                            break;
                        }
                    }
                    i = j - 1;
                    if octal.is_empty() {
                        out.push('\0');
                    } else {
                        let val = u32::from_str_radix(&octal, 8).unwrap_or(0).min(255);
                        out.push(val as u8 as char);
                    }
                }
                other => {
                    out.push('\\');
                    out.push(other as char);
                }
            }
        } else {
            out.push(bytes[i] as char);
        }
        i += 1;
    }
    out
}
