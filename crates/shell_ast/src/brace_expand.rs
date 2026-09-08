use crate::ast::{Word, WordPart};
use crate::span::Span;

pub fn try_brace_expand(
    prefix_parts: &[WordPart],
    body: &str,
    suffix_parts: &[WordPart],
    span: Span,
) -> Option<Vec<Word>> {
    let alternatives = parse_brace_body(body)?;
    if alternatives.len() < 2 {
        return None;
    }

    let mut results = Vec::with_capacity(alternatives.len());
    for alt in &alternatives {
        let mut parts = Vec::new();
        parts.extend_from_slice(prefix_parts);
        parts.push(WordPart::Literal(alt.clone()));
        parts.extend_from_slice(suffix_parts);
        let may_glob = parts.iter().any(|p| match p {
            WordPart::Literal(s) => s.contains('*') || s.contains('?') || s.contains('['),
            _ => false,
        });
        results.push(Word {
            parts,
            span,
            may_glob,
        });
    }
    Some(results)
}

fn parse_brace_body(body: &str) -> Option<Vec<String>> {
    if let Some(range) = try_parse_range(body) {
        return Some(range);
    }
    let parts: Vec<&str> = split_top_level_commas(body);
    if parts.len() >= 2 {
        Some(parts.into_iter().map(|s| s.to_string()).collect())
    } else {
        None
    }
}

fn split_top_level_commas(s: &str) -> Vec<&str> {
    let mut result = Vec::new();
    let mut depth = 0;
    let mut start = 0;
    for (i, c) in s.char_indices() {
        match c {
            '{' => depth += 1,
            '}' => depth -= 1,
            ',' if depth == 0 => {
                result.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    result.push(&s[start..]);
    result
}

fn try_parse_range(body: &str) -> Option<Vec<String>> {
    let dotdot = body.find("..")?;
    let left = &body[..dotdot];
    let rest = &body[dotdot + 2..];

    let (right, step_str) = if let Some(dots2) = rest.find("..") {
        (&rest[..dots2], Some(&rest[dots2 + 2..]))
    } else {
        (rest, None)
    };

    if let (Ok(start), Ok(end)) = (left.parse::<i64>(), right.parse::<i64>()) {
        let step: i64 = if let Some(s) = step_str {
            let s: i64 = s.parse().ok()?;
            if s == 0 {
                return None;
            }
            s.abs()
        } else {
            1
        };

        let mut result = Vec::new();
        if start <= end {
            let mut i = start;
            while i <= end {
                result.push(i.to_string());
                i += step;
            }
        } else {
            let mut i = start;
            while i >= end {
                result.push(i.to_string());
                i -= step;
            }
        }
        return Some(result);
    }

    let start_chars: Vec<char> = left.chars().collect();
    let end_chars: Vec<char> = right.chars().collect();
    if start_chars.len() == 1 && end_chars.len() == 1 {
        let sc = start_chars[0];
        let ec = end_chars[0];
        if !sc.is_ascii() || !ec.is_ascii() {
            return None;
        }
        let start = sc as u32;
        let end = ec as u32;
        let step: u32 = if let Some(s) = step_str {
            let s: u32 = s.parse().ok()?;
            if s == 0 {
                return None;
            }
            s
        } else {
            1
        };

        let mut result = Vec::new();
        if start <= end {
            let mut c = start;
            while c <= end {
                result.push(char::from_u32(c).unwrap().to_string());
                c += step;
            }
        } else {
            let mut c = start;
            while c >= end {
                result.push(char::from_u32(c).unwrap().to_string());
                c -= step;
            }
        }
        return Some(result);
    }

    None
}
