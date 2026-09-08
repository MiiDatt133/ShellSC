use super::BuiltinResult;
use crate::env::Env;

/// Process export pairs.
///
/// Pairs are emitted by the lowerer in two forms:
///
/// 1. **Literal** (`export FOO=bar`)  
///    Lowerer pre-splits on `=`: name=`"FOO"`, value=`"bar"`.  
///    No `=` appears in `name` after splitting.
///
/// 2. **Dynamic** (`export $var` where `$var` might be `"FOO=bar"`)  
///    Lowerer pushes the evaluated word as `name` and `""` as value.  
///    The `=` appears inside `name`, so we re-split here at runtime.
///
/// 3. **Export-only** (`export FOO`)  
///    Lowerer pushes `name="FOO"`, `value=""`.  
///    We export the variable's current value (no `=` in `name`).
pub fn run_pairs(pairs: &[(String, String)], env: &mut Env) -> BuiltinResult {
    for (name, value) in pairs {
        // Dynamic case: the full "NAME=value" string landed in `name`.
        if let Some(eq) = name.find('=') {
            env.export(&name[..eq], Some(&name[eq + 1..]));
        } else if value.is_empty() {
            // Export-only: re-export current value (or mark for environment).
            env.export(name, None);
        } else {
            // Pre-split literal: name + value already separated.
            env.export(name, Some(value));
        }
    }
    BuiltinResult::ok()
}
