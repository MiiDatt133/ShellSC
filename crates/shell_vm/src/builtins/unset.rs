use super::BuiltinResult;
use crate::env::Env;

pub fn run(args: &[String], env: &mut Env) -> BuiltinResult {
    for arg in args {
        let key = match arg.find('=') {
            Some(eq) => &arg[..eq],
            None => arg.as_str(),
        };
        if key.is_empty() {
            continue;
        }
        // `unset arr[i]` — strip one element. Index arrives expanded, so
        // any $var index is already a literal digit string here.
        if let Some(open) = key.find('[') {
            if let Some(rel) = key[open + 1..].rfind(']') {
                let close = open + 1 + rel;
                let name = &key[..open];
                let idx = &key[open + 1..close];
                if let Ok(i) = idx.parse::<usize>() {
                    env.array_unset_index(name, i);
                    continue;
                }
            }
        }
        env.unset(key);
    }
    BuiltinResult::ok()
}
