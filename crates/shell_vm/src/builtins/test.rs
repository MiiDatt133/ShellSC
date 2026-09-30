use super::BuiltinResult;
use crate::status::ExitStatus;

pub fn run(args: &[String]) -> BuiltinResult {
    let args: Vec<&str> = args
        .iter()
        .map(|s| s.as_str())
        .filter(|&s| s != "]")
        .collect();
    let ok = eval_test(&args);
    BuiltinResult {
        out: vec![],
        status: if ok { ExitStatus::OK } else { ExitStatus::FAIL },
    }
}

fn eval_test(args: &[&str]) -> bool {
    match args {
        [] => false,
        [flag, val] => match *flag {
            "-f" => std::path::Path::new(val).is_file(),
            "-d" => std::path::Path::new(val).is_dir(),
            "-e" => std::path::Path::new(val).exists(),
            "-s" => std::fs::metadata(val).map(|m| m.len() > 0).unwrap_or(false),
            "-z" => val.is_empty(),
            "-n" => !val.is_empty(),
            "-r" | "-w" | "-x" => std::path::Path::new(val).exists(),
            _ => false,
        },
        [left, op, right] => match *op {
            "=" | "==" => left == right,
            "!=" => left != right,
            "<" => left < right,
            ">" => left > right,
            "-eq" => parse_i64(left) == parse_i64(right),
            "-ne" => parse_i64(left) != parse_i64(right),
            "-lt" => parse_i64(left) < parse_i64(right),
            "-le" => parse_i64(left) <= parse_i64(right),
            "-gt" => parse_i64(left) > parse_i64(right),
            "-ge" => parse_i64(left) >= parse_i64(right),
            _ => false,
        },
        ["!", rest @ ..] => !eval_test(rest),
        _ => false,
    }
}

fn parse_i64(s: &str) -> i64 {
    s.parse().unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::eval_test;

    #[test]
    fn s_requires_existing_nonempty_file() {
        let dir = std::env::temp_dir();
        let empty = dir.join("shellsc_test_s_empty");
        let filled = dir.join("shellsc_test_s_filled");
        std::fs::write(&empty, b"").unwrap();
        std::fs::write(&filled, b"x").unwrap();
        let e = empty.to_str().unwrap();
        let f = filled.to_str().unwrap();

        assert!(eval_test(&["-s", f]));
        assert!(!eval_test(&["-s", e]));
        assert!(!eval_test(&["-s", "/no/such/file_xyz"]));

        let _ = std::fs::remove_file(&empty);
        let _ = std::fs::remove_file(&filled);
    }
}
