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
