use std::collections::{HashMap, HashSet};

/// `std::env::set_var` panics on values containing NUL. Bash truncates at
/// the first NUL when exporting — do the same instead of aborting.
fn safe_set_var(name: &str, value: &str) {
    let v = match value.find('\0') {
        Some(i) => &value[..i],
        None => value,
    };
    std::env::set_var(name, v);
}

/// Shell variable environment with proper lexical scope isolation.
///
/// The environment is a stack of frames:
///   - `frames[0]`   — global scope (process-inherited variables, script globals)
///   - `frames.last()` — innermost function call scope
///
/// Variable lookup searches from the top frame downward (dynamic scoping,
/// matching bash behaviour).  `set()` updates the nearest frame that already
/// owns the name, or falls back to the global frame.  `set_local()` always
/// writes into the topmost frame, implementing `local`.
#[derive(Debug, Clone)]
pub struct Env {
    /// Frame stack: index 0 = global, last = innermost function.
    frames: Vec<HashMap<String, String>>,
    /// Array storage, frame-parallel with `frames` (None = unset index,
    /// emulating bash sparse arrays after `unset arr[i]`).
    arrays: Vec<HashMap<String, Vec<Option<String>>>>,
    /// Names exported to child processes (kept in sync with process env).
    exported: HashSet<String>,
}

impl Default for Env {
    fn default() -> Self {
        Self {
            frames: vec![HashMap::new()],
            arrays: vec![HashMap::new()],
            exported: HashSet::new(),
        }
    }
}

impl Env {
    /// Create a new environment initialised from the current process environment.
    pub fn new() -> Self {
        let mut global: HashMap<String, String> = std::env::vars().collect();
        // bash default IFS: space, tab, newline (unless inherited set).
        global
            .entry("IFS".to_string())
            .or_insert_with(|| " \t\n".to_string());
        let exported: HashSet<String> = global.keys().cloned().collect();
        Self {
            frames: vec![global],
            arrays: vec![HashMap::new()],
            exported,
        }
    }

    // ── Lookup ────────────────────────────────────────────────────────────────

    /// Return the value of `name` from the nearest frame that has it.
    pub fn get(&self, name: &str) -> Option<&str> {
        // Positional params are bound per function frame — never fall through
        // to an outer frame's params (bash: `fn` with no args sees empty `$1`).
        let is_positional = !name.is_empty() && name.chars().all(|c| c.is_ascii_digit());
        for frame in self.frames.iter().rev() {
            if let Some(v) = frame.get(name) {
                return Some(v.as_str());
            }
            if is_positional && frame.contains_key("#") {
                // This frame has its own positional set; stop here.
                return None;
            }
        }
        None
    }

    /// Scalar lookup with array fallback: `$arr` where `arr` is an array
    /// expands to element 0 (bash semantics).
    pub fn get_scalar_or_array0(&self, name: &str) -> Option<&str> {
        if let Some(v) = self.get(name) {
            return Some(v);
        }
        if let Some(arr) = self.find_array(name) {
            if let Some(Some(first)) = arr.first() {
                return Some(first.as_str());
            }
        }
        None
    }

    // ── Assignment ────────────────────────────────────────────────────────────

    /// Assign `name = value` with bash-compatible scoping:
    /// - If any frame already owns `name`, update it there (innermost wins).
    /// - Otherwise insert in the global frame.
    pub fn set(&mut self, name: impl Into<String>, value: impl Into<String>) {
        let name = name.into();
        let value = value.into();
        // bash: scalar assignment to an array name writes element 0 and
        // keeps the remaining elements.
        if self.is_array(&name) {
            self.array_set_index(&name, 0, value);
            return;
        }
        safe_set_var(&name, &value);
        for frame in self.frames.iter_mut().rev() {
            if frame.contains_key(&name) {
                frame.insert(name, value);
                return;
            }
        }
        self.frames[0].insert(name, value);
    }

    /// Declare `name` as local to the current function frame and assign it.
    /// This is what `local name=value` does inside a function.
    pub fn set_local(&mut self, name: impl Into<String>, value: impl Into<String>) {
        let name = name.into();
        let value = value.into();
        if self.exported.contains(&name) {
            safe_set_var(&name, &value);
        }
        if let Some(frame) = self.frames.last_mut() {
            frame.insert(name, value);
        }
    }

    /// Declare `name` as local without assigning a value.
    /// The variable is inserted into the current frame with the value from the
    /// nearest enclosing scope (or "" if not set anywhere), shadowing it.
    pub fn declare_local(&mut self, name: &str) {
        if let Some(frame) = self.frames.last_mut() {
            frame.entry(name.to_string()).or_insert(String::new());
        }
    }

    /// Remove `name` from the innermost frame that owns it.
    /// If that frame was a `local`, the outer scope's value becomes visible again.
    pub fn unset(&mut self, name: &str) {
        if self.exported.remove(name) {
            std::env::remove_var(name);
        }
        for frame in self.frames.iter_mut().rev() {
            if frame.remove(name).is_some() {
                self.array_unset(name);
                return; // only remove from innermost frame
            }
        }
        // bash: `unset arr` removes the array even without a scalar entry.
        self.array_unset(name);
    }

    /// Mark `name` for export (optionally setting its value at the same time).
    pub fn export(&mut self, name: &str, value: Option<&str>) {
        if let Some(v) = value {
            self.set(name, v);
        }
        let val = self.get(name).unwrap_or("").to_string();
        safe_set_var(name, &val);
        self.exported.insert(name.to_string());
    }

    // ── Expansion ─────────────────────────────────────────────────────────────

    /// Expand `$name`, `$1`, `$#`, `$@`, `$?`, etc. to a string value.
    pub fn expand(&self, name: &str) -> String {
        match name {
            "*" => {
                // `$*` joins on the first char of IFS (default space).
                let count: usize = self.get("#").and_then(|s| s.parse().ok()).unwrap_or(0);
                let items: Vec<String> = (1..=count)
                    .map(|i| self.get(&i.to_string()).unwrap_or("").to_string())
                    .collect();
                let sep = match self.get("IFS") {
                    Some(v) if !v.is_empty() => v.get(0..1).unwrap_or(" "),
                    _ => " ",
                };
                items.join(sep)
            }
            "@" => {
                let count: usize = self.get("#").and_then(|s| s.parse().ok()).unwrap_or(0);
                (1..=count)
                    .map(|i| self.get(&i.to_string()).unwrap_or("").to_string())
                    .collect::<Vec<_>>()
                    .join(" ")
            }
            "#" => self.get("#").unwrap_or("0").to_string(),
            "?" => self.get("?").unwrap_or("0").to_string(),
            "$" => std::process::id().to_string(),
            "0" => self.get("0").unwrap_or("shellsc").to_string(),
            n if n.chars().all(|c| c.is_ascii_digit()) => self.get(n).unwrap_or("").to_string(),
            _ => self.get_scalar_or_array0(name).unwrap_or("").to_string(),
        }
    }

    // ── Function call frames ──────────────────────────────────────────────────

    /// Push a new scope for a function call, binding positional parameters.
    pub fn push_frame(&mut self, args: &[String]) {
        let mut frame = HashMap::new();
        frame.insert("#".to_string(), args.len().to_string());
        for (i, a) in args.iter().enumerate() {
            frame.insert((i + 1).to_string(), a.clone());
        }
        self.frames.push(frame);
        self.arrays.push(HashMap::new());
    }

    /// Pop the innermost function scope, restoring the outer environment.
    /// Syncs the process env for any exported variables that were shadowed.
    pub fn pop_frame(&mut self) {
        if self.frames.len() <= 1 {
            return;
        }
        self.arrays.pop();
        if let Some(frame) = self.frames.pop() {
            for name in frame.keys() {
                if self.exported.contains(name.as_str()) {
                    let outer = self.get(name).unwrap_or("");
                    safe_set_var(name, outer);
                }
            }
        }
    }

    // ── Positional parameter helpers (compatibility with vm.rs / set builtin) ─

    /// Return the current positional parameter list ($1, $2, …).
    pub fn get_positional(&self) -> Vec<String> {
        let count: usize = self.get("#").and_then(|s| s.parse().ok()).unwrap_or(0);
        (1..=count)
            .map(|i| self.get(&i.to_string()).unwrap_or("").to_string())
            .collect()
    }

    /// Set positional parameters in the *current* frame (used by `set --`).
    pub fn set_positional(&mut self, args: &[String]) {
        self.set_local("#", args.len().to_string());
        for (i, a) in args.iter().enumerate() {
            self.set_local(&(i + 1).to_string(), a.as_str());
        }
        // Clear stale higher-numbered params in the current frame.
        let new_count = args.len();
        let mut i = new_count + 1;
        while let Some(frame) = self.frames.last_mut() {
            if frame.remove(&i.to_string()).is_none() {
                break;
            }
            i += 1;
        }
    }

    /// No-op kept for call-site compatibility; pop_frame() handles restoration.
    pub fn restore_positional(&mut self, _saved: &[String]) {}

    // ── Arrays ────────────────────────────────────────────────────────────────

    /// Replace the whole array (`arr=(x y)` resets all elements).
    /// Follows scalar scoping: update the innermost owning frame, else global.
    pub fn set_array(&mut self, name: &str, items: Vec<String>) {
        let vec: Vec<Option<String>> = items.into_iter().map(Some).collect();
        for i in (0..self.arrays.len()).rev() {
            if self.arrays[i].contains_key(name) {
                self.arrays[i].insert(name.to_string(), vec);
                return;
            }
        }
        self.arrays[0].insert(name.to_string(), vec);
    }

    /// Append elements (`arr+=(x y)`).
    pub fn array_append(&mut self, name: &str, items: Vec<String>) {
        for i in (0..self.arrays.len()).rev() {
            if let Some(v) = self.arrays[i].get_mut(name) {
                v.extend(items.into_iter().map(Some));
                return;
            }
        }
        let vec: Vec<Option<String>> = items.into_iter().map(Some).collect();
        self.arrays[0].insert(name.to_string(), vec);
    }

    /// `local arr=(x y)` — always write the innermost frame, shadowing any
    /// outer array of the same name for the rest of the function.
    pub fn set_array_local(&mut self, name: &str, items: Vec<String>) {
        let vec: Vec<Option<String>> = items.into_iter().map(Some).collect();
        if let Some(frame) = self.arrays.last_mut() {
            frame.insert(name.to_string(), vec);
        }
    }

    /// `local arr+=(x y)` — append inside the innermost frame.
    pub fn array_append_local(&mut self, name: &str, items: Vec<String>) {
        if let Some(frame) = self.arrays.last_mut() {
            let entry = frame.entry(name.to_string()).or_insert_with(Vec::new);
            entry.extend(items.into_iter().map(Some));
        }
    }

    /// Read one element; unset indices yield "" (bash prints empty).
    pub fn array_get(&self, name: &str, idx: usize) -> String {
        self.find_array(name)
            .and_then(|v| v.get(idx))
            .and_then(|s| s.as_deref())
            .unwrap_or("")
            .to_string()
    }

    /// All set elements, in index order.
    pub fn array_get_all(&self, name: &str) -> Vec<String> {
        self.find_array(name)
            .map(|v| v.iter().filter_map(|s| s.clone()).collect())
            .unwrap_or_default()
    }

    /// Number of set elements (`${#arr[@]}`).
    pub fn array_len(&self, name: &str) -> usize {
        self.find_array(name)
            .map(|v| v.iter().filter(|s| s.is_some()).count())
            .unwrap_or(0)
    }

    /// Assign one element `arr[idx]=v`, growing with empty fills as needed.
    pub fn array_set_index(&mut self, name: &str, idx: usize, value: String) {
        for i in (0..self.arrays.len()).rev() {
            if let Some(v) = self.arrays[i].get_mut(name) {
                if v.len() <= idx {
                    v.resize_with(idx + 1, || None);
                }
                v[idx] = Some(value);
                return;
            }
        }
        let mut vec: Vec<Option<String>> = Vec::new();
        vec.resize_with(idx + 1, || None);
        vec[idx] = Some(value);
        self.arrays[0].insert(name.to_string(), vec);
    }

    /// Unset the whole array.
    pub fn array_unset(&mut self, name: &str) {
        for i in (0..self.arrays.len()).rev() {
            if self.arrays[i].remove(name).is_some() {
                return;
            }
        }
    }

    /// Unset one element, leaving a sparse hole (`unset arr[i]`).
    /// bash keeps the array length — no truncation.
    pub fn array_unset_index(&mut self, name: &str, idx: usize) {
        for i in (0..self.arrays.len()).rev() {
            if let Some(v) = self.arrays[i].get_mut(name) {
                if idx < v.len() {
                    v[idx] = None;
                }
                return;
            }
        }
    }

    /// Does this name exist as an array anywhere?
    pub fn is_array(&self, name: &str) -> bool {
        self.find_array(name).is_some()
    }

    fn find_array(&self, name: &str) -> Option<&Vec<Option<String>>> {
        for i in (0..self.arrays.len()).rev() {
            if let Some(v) = self.arrays[i].get(name) {
                return Some(v);
            }
        }
        None
    }

    // ── Subshell snapshot / restore ───────────────────────────────────────────

    pub fn snapshot(&self) -> Self {
        self.clone()
    }

    pub fn restore(&mut self, saved: Self) {
        // Remove process-env entries added by the subshell.
        for k in &self.exported {
            if !saved.exported.contains(k.as_str()) {
                std::env::remove_var(k);
            }
        }
        // Reset changed exported values.
        for k in &saved.exported {
            let saved_val = saved.get(k).unwrap_or("");
            let curr_val = self.get(k).unwrap_or("");
            if saved_val != curr_val {
                safe_set_var(k, saved_val);
            }
        }
        *self = saved;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env() -> Env {
        Env::default()
    }

    #[test]
    fn set_get_roundtrip() {
        let mut e = env();
        e.set_array("a", vec!["x".into(), "y".into(), "z".into()]);
        assert_eq!(e.array_get("a", 0), "x");
        assert_eq!(e.array_get("a", 2), "z");
        assert_eq!(e.array_get("a", 9), "");
        assert_eq!(e.array_len("a"), 3);
    }

    #[test]
    fn replace_resets() {
        let mut e = env();
        e.set_array("a", vec!["x".into(), "y".into()]);
        e.set_array("a", vec!["z".into()]);
        assert_eq!(e.array_len("a"), 1);
        assert_eq!(e.array_get("a", 0), "z");
        assert_eq!(e.array_get("a", 1), "");
    }

    #[test]
    fn append_grows() {
        let mut e = env();
        e.set_array("a", vec!["x".into()]);
        e.array_append("a", vec!["y".into(), "z".into()]);
        assert_eq!(e.array_len("a"), 3);
        assert_eq!(e.array_get("a", 2), "z");
    }

    #[test]
    fn index_set_grows_sparse() {
        let mut e = env();
        e.array_set_index("a", 3, "v".into());
        assert_eq!(e.array_len("a"), 1);
        assert_eq!(e.array_get("a", 1), "");
        assert_eq!(e.array_get("a", 3), "v");
    }

    #[test]
    fn unset_index_leaves_hole() {
        let mut e = env();
        e.set_array("a", vec!["x".into(), "y".into(), "z".into()]);
        e.array_unset_index("a", 1);
        assert_eq!(e.array_get("a", 1), "");
        assert_eq!(e.array_get("a", 2), "z");
        assert_eq!(e.array_get_all("a"), vec!["x".to_string(), "z".to_string()]);
    }

    #[test]
    fn unset_name_removes_array() {
        let mut e = env();
        e.set_array("a", vec!["x".into()]);
        e.unset("a");
        assert!(!e.is_array("a"));
        assert_eq!(e.array_len("a"), 0);
    }

    #[test]
    fn function_array_assign_hits_global() {
        // bash: `arr=(x)` inside a function (no `local`) updates the global.
        let mut e = env();
        e.set_array("a", vec!["g".into()]);
        e.push_frame(&[]);
        e.set_array("a", vec!["l".into(), "loc".into()]);
        e.pop_frame();
        assert_eq!(e.array_get("a", 0), "l");
        assert_eq!(e.array_len("a"), 2);
    }

    #[test]
    fn get_all_set_order() {
        let mut e = env();
        e.set_array("a", vec!["1".into(), "2".into(), "3".into()]);
        assert_eq!(e.array_get_all("a"), vec!["1", "2", "3"]);
        e.array_unset_index("a", 0);
        assert_eq!(e.array_get_all("a"), vec!["2", "3"]);
    }
}
