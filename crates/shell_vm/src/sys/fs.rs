// === FILE: crates/shell_vm/src/sys/fs.rs ===
use shell_ast::ShellError;

/// Check if a path exists.
pub fn path_exists(path: &str) -> bool {
    std::path::Path::new(path).exists()
}

/// Check if a path is a regular file.
pub fn is_file(path: &str) -> bool {
    std::path::Path::new(path).is_file()
}

/// Check if a path is a directory.
pub fn is_dir(path: &str) -> bool {
    std::path::Path::new(path).is_dir()
}

/// Read file to string.
pub fn read_to_string(path: &str) -> Result<String, ShellError> {
    std::fs::read_to_string(path)
        .map_err(|e| ShellError::IoError(format!("read '{}': {}", path, e)))
}
// === END FILE ===
