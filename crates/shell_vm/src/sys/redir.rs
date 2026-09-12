use std::{
    collections::HashMap,
    fs::{File, OpenOptions},
    io::{BufRead, BufReader, Write},
    process::{Command, Stdio},
};

use shell_ast::ShellError;
use shell_bc::bytecode::RedirKind;

#[derive(Debug, Clone)]
pub enum RedirTargetSpec {
    File(String),
    Fd(u32),
    HereDoc(String),
}

#[derive(Debug, Clone)]
pub struct RedirSpec {
    pub kind: RedirKind,
    pub fd: u32,
    pub target: RedirTargetSpec,
}

#[derive(Debug, Default)]
pub struct RedirSet {
    pub specs: Vec<RedirSpec>,
    /// File handles opened by `exec N< f` / `exec N> f` — cloned into the set
    /// so child processes can use them directly (the fds are never dup2'd
    /// into this process, so /proc/self/fd/N would not work).
    pub exec_files: HashMap<u32, std::fs::File>,
    /// fd>2 dup targets from `exec 3>&1` — a child writing fd 3 must reach
    /// whatever fd the dup points at (File handle or default stream).
    pub exec_dups: HashMap<u32, u32>,
}

#[derive(Debug, Clone)]
enum ResolvedTarget {
    File { path: String, kind: RedirKind },
    HereDoc(String),
    Stream(u32),
}

impl Clone for RedirSet {
    fn clone(&self) -> Self {
        let mut exec_files = HashMap::new();
        for (fd, f) in &self.exec_files {
            if let Ok(c) = f.try_clone() {
                exec_files.insert(*fd, c);
            }
        }
        Self {
            specs: self.specs.clone(),
            exec_files,
            exec_dups: self.exec_dups.clone(),
        }
    }
}

impl RedirSet {
    fn clone_exec_files(src: &HashMap<u32, std::fs::File>) -> HashMap<u32, std::fs::File> {
        let mut out = HashMap::new();
        for (fd, f) in src {
            if let Ok(c) = f.try_clone() {
                out.insert(*fd, c);
            }
        }
        out
    }
}

impl RedirSet {
    pub fn new(specs: Vec<RedirSpec>) -> Self {
        Self {
            specs,
            exec_files: HashMap::new(),
            exec_dups: HashMap::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.specs.is_empty()
    }

    pub fn push(&mut self, spec: RedirSpec) {
        self.specs.push(spec);
    }

    pub fn split_stdin(&self) -> Result<(Option<Vec<u8>>, RedirSet), ShellError> {
        let mut stdin_bytes: Option<Vec<u8>> = None;
        let mut other_specs: Vec<RedirSpec> = Vec::with_capacity(self.specs.len());

        for spec in &self.specs {
            if spec.fd != 0 {
                other_specs.push(spec.clone());
                continue;
            }

            match &spec.target {
                RedirTargetSpec::File(path) => {
                    let data =
                        std::fs::read(path).map_err(|e| ShellError::IoError(e.to_string()))?;
                    stdin_bytes = Some(data);
                }
                RedirTargetSpec::HereDoc(body) => {
                    stdin_bytes = Some(body.as_bytes().to_vec());
                }
                RedirTargetSpec::Fd(n) if *n == 0 => {
                    stdin_bytes = None;
                }
                RedirTargetSpec::Fd(n) if self.exec_files.contains_key(n) => {
                    // `0<&6` — child reads directly from the `exec 6< f` handle
                    // via apply_to_command; keep the spec for that.
                    other_specs.push(spec.clone());
                }
                RedirTargetSpec::Fd(n) => {
                    // fd not open (e.g. `exec 6< f` failed): bash reports a
                    // bad file descriptor on the redirecting command.
                    let _ = writeln!(std::io::stderr(), "{}: Bad file descriptor", n);
                    return Err(ShellError::IoError(format!("{}: Bad file descriptor", n)));
                }
            }
        }

        Ok((
            stdin_bytes,
            RedirSet {
                specs: other_specs,
                exec_files: RedirSet::clone_exec_files(&self.exec_files),
                exec_dups: self.exec_dups.clone(),
            },
        ))
    }

    fn resolve_map(&self) -> HashMap<u32, ResolvedTarget> {
        let mut map = HashMap::from([
            (0, ResolvedTarget::Stream(0)),
            (1, ResolvedTarget::Stream(1)),
            (2, ResolvedTarget::Stream(2)),
        ]);

        for spec in &self.specs {
            let resolved = match (&spec.kind, &spec.target) {
                (_, RedirTargetSpec::File(path)) => ResolvedTarget::File {
                    path: path.clone(),
                    kind: spec.kind.clone(),
                },
                (_, RedirTargetSpec::HereDoc(body)) => ResolvedTarget::HereDoc(body.clone()),
                (_, RedirTargetSpec::Fd(n)) => {
                    map.get(n).cloned().unwrap_or(ResolvedTarget::Stream(*n))
                }
            };
            map.insert(spec.fd, resolved);
        }

        map
    }

    fn resolve_fd(&self, fd: u32) -> ResolvedTarget {
        self.resolve_map()
            .remove(&fd)
            .unwrap_or(ResolvedTarget::Stream(fd))
    }

    pub fn open_input(&self, fd: u32) -> Result<Box<dyn BufRead>, ShellError> {
        match self.resolve_fd(fd) {
            ResolvedTarget::File { path, .. } => {
                let file = File::open(path).map_err(|e| ShellError::IoError(e.to_string()))?;
                Ok(Box::new(BufReader::new(file)))
            }
            ResolvedTarget::HereDoc(body) => Ok(Box::new(BufReader::new(std::io::Cursor::new(
                body.into_bytes(),
            )))),
            ResolvedTarget::Stream(0) => Ok(Box::new(BufReader::new(std::io::stdin()))),
            ResolvedTarget::Stream(n) => Err(ShellError::NotImplemented(format!(
                "input redirection from fd {}",
                n
            ))),
        }
    }

    pub fn write_output(&self, fd: u32, text: &str) -> Result<(), ShellError> {
        match self.resolve_fd(fd) {
            ResolvedTarget::File { path, kind } => {
                let mut file = match kind {
                    RedirKind::Out => open_file(&path, &RedirKind::Out)?,
                    RedirKind::Append => open_file(&path, &RedirKind::Append)?,
                    RedirKind::In => {
                        return Err(ShellError::NotImplemented(
                            "writing to input redirection".into(),
                        ))
                    }
                    RedirKind::OutFd | RedirKind::InFd => {
                        return Err(ShellError::NotImplemented("fd redirection".into()))
                    }
                    RedirKind::HereDoc | RedirKind::HereDocLit => {
                        return Err(ShellError::NotImplemented(
                            "writing to heredoc redirection".into(),
                        ))
                    }
                };
                file.write_all(text.as_bytes())
                    .map_err(|e| ShellError::IoError(e.to_string()))?;
                file.flush()
                    .map_err(|e| ShellError::IoError(e.to_string()))?;
                Ok(())
            }
            ResolvedTarget::Stream(n) => write_default(n, text),
            ResolvedTarget::HereDoc(_) => Err(ShellError::NotImplemented(
                "writing to heredoc redirection".into(),
            )),
        }
    }

    /// Stdio for an fd redirect targeting fd N>2: prefer the `exec N< f`
    /// handle (cloned) — those fds are never dup2'd into this process, so
    /// /proc/self/fd/N would not exist. Falls back to /proc/self/fd/N.
    fn stdio_from_fd(&self, n: u32, write: bool) -> Option<Stdio> {
        // Follow `exec 3>&1`-style dups first: fd 3 itself is never open in
        // this process, only its target is.
        let mut n = n;
        let mut hops = 0;
        while let Some(t) = self.exec_dups.get(&n) {
            if *t == n || hops > 8 {
                break;
            }
            n = *t;
            hops += 1;
        }
        if let Some(f) = self.exec_files.get(&n) {
            if let Ok(clone) = f.try_clone() {
                return Some(Stdio::from(clone));
            }
        }
        let path = format!("/proc/self/fd/{}", n);
        if write {
            OpenOptions::new()
                .write(true)
                .append(true)
                .open(path)
                .ok()
                .map(Stdio::from)
        } else {
            File::open(path).ok().map(Stdio::from)
        }
    }

    pub fn apply_to_command(&self, cmd: &mut Command) -> Result<(), ShellError> {
        // Resolve the full fd chain first (handles 2>&1, 1>&2, etc.)
        let map = self.resolve_map();

        // stdin (fd 0)
        match map.get(&0) {
            Some(ResolvedTarget::File { path, .. }) => {
                let file = open_file(path, &RedirKind::In)?;
                cmd.stdin(Stdio::from(file));
            }
            Some(ResolvedTarget::HereDoc(_)) => {
                // heredoc stdin fed externally via stdin_bytes in proc.rs
            }
            Some(ResolvedTarget::Stream(n)) if *n != 0 => {
                // e.g. 0<&6 (fd 6 opened via exec 6< file) or 0>&1
                if let Some(s) = self.stdio_from_fd(*n, false) {
                    cmd.stdin(s);
                }
            }
            _ => {} // Stream(0) — inherit
        }

        // stdout (fd 1)
        match map.get(&1) {
            Some(ResolvedTarget::File { path, kind }) => {
                let file = open_file(path, kind)?;
                cmd.stdout(Stdio::from(file));
            }
            Some(ResolvedTarget::Stream(n)) if *n != 1 => {
                // e.g. 1>&2 — dup stderr.  Open in append mode: opening
                // /proc/self/fd/N creates a NEW file description (offset 0),
                // which would overwrite earlier writes when stderr is a file.
                if let Some(s) = self.stdio_from_fd(*n, true) {
                    cmd.stdout(s);
                }
            }
            _ => {} // Stream(1) or HereDoc — inherit stdout
        }

        // stderr (fd 2)
        match map.get(&2) {
            Some(ResolvedTarget::File { path, kind }) => {
                // Includes the resolved case of `>/dev/null 2>&1`:
                // resolve_map() already followed 2->1->File("/dev/null")
                let file = open_file(path, kind)?;
                cmd.stderr(Stdio::from(file));
            }
            Some(ResolvedTarget::Stream(n)) if *n != 2 => {
                // e.g. 2>&1 where stdout was NOT redirected to a file —
                // stderr should go wherever stdout currently goes (terminal).
                // Append mode: a fresh description starts at offset 0 and
                // would clobber earlier writes when the target is a file.
                if let Some(s) = self.stdio_from_fd(*n, true) {
                    cmd.stderr(s);
                }
            }
            _ => {} // Stream(2) — inherit stderr
        }

        Ok(())
    }
}

fn open_file(path: &str, kind: &RedirKind) -> Result<File, ShellError> {
    match kind {
        RedirKind::Out => OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(path)
            .map_err(|e| ShellError::IoError(e.to_string())),
        RedirKind::Append => OpenOptions::new()
            .create(true)
            .write(true)
            .append(true)
            .open(path)
            .map_err(|e| ShellError::IoError(e.to_string())),
        RedirKind::In => File::open(path).map_err(|e| ShellError::IoError(e.to_string())),
        _ => Err(ShellError::NotImplemented("fd redirection".into())),
    }
}

fn write_default(fd: u32, text: &str) -> Result<(), ShellError> {
    match fd {
        1 => {
            let mut out = std::io::stdout().lock();
            out.write_all(text.as_bytes())
                .map_err(|e| ShellError::IoError(e.to_string()))?;
            out.flush()
                .map_err(|e| ShellError::IoError(e.to_string()))?;
            Ok(())
        }
        2 => {
            let mut out = std::io::stderr().lock();
            out.write_all(text.as_bytes())
                .map_err(|e| ShellError::IoError(e.to_string()))?;
            out.flush()
                .map_err(|e| ShellError::IoError(e.to_string()))?;
            Ok(())
        }
        _ => Ok(()),
    }
}
