use shell_ast::ShellError;

/// Runtime value on the VM stack.
#[derive(Debug, Clone)]
pub enum Value {
    Str(String),
}

impl Value {
    pub fn into_string(self) -> String {
        match self {
            Value::Str(s) => s,
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            Value::Str(s) => s.as_str(),
        }
    }
}

/// Operand stack for the VM.
#[derive(Debug, Default)]
pub struct Stack {
    inner: Vec<Value>,
}

impl Stack {
    pub fn new() -> Self {
        Self { inner: vec![] }
    }

    pub fn join(&self, separator: &str) -> String {
        self.inner
            .iter()
            .map(|v| v.as_str())
            .collect::<Vec<&str>>()
            .join(separator)
    }

    pub fn push(&mut self, v: Value) {
        self.inner.push(v);
    }

    pub fn push_str(&mut self, s: impl Into<String>) {
        self.inner.push(Value::Str(s.into()));
    }

    pub fn pop(&mut self) -> Result<Value, ShellError> {
        self.inner
            .pop()
            .ok_or_else(|| ShellError::VmError("stack underflow".into()))
    }

    /// Pop N values; returns them in push order (oldest first).
    pub fn pop_n(&mut self, n: usize) -> Result<Vec<Value>, ShellError> {
        if self.inner.len() < n {
            return Err(ShellError::VmError(format!(
                "stack underflow: need {}, have {}",
                n,
                self.inner.len()
            )));
        }
        let at = self.inner.len() - n;
        let vals = self.inner.drain(at..).collect();
        Ok(vals)
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}
