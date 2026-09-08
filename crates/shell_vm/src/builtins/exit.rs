use super::BuiltinResult;

pub fn run(args: &[String]) -> BuiltinResult {
    let code: i32 = args.first().and_then(|s| s.parse().ok()).unwrap_or(0);
    std::process::exit(code);
}
