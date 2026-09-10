use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    Closed,
    Permission,
    Communication,
    Cancelled,
    UnsettledGeometry,
    Other,
}

/// Classification is independent of presentation. AX failures retain their code
/// and operation, including when an engine operation adds context.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackendError {
    pub kind: ErrorKind,
    pub ax_code: Option<i32>,
    pub context: String,
    pub causes: Vec<BackendError>,
}
impl BackendError {
    pub fn new(kind: ErrorKind, context: impl Into<String>) -> Self {
        Self {
            kind,
            ax_code: None,
            context: context.into(),
            causes: vec![],
        }
    }
    pub fn context(mut self, context: impl fmt::Display) -> Self {
        self.context = format!("{context}: {}", self.context);
        self
    }
    pub fn multiple(errors: Vec<Self>) -> Self {
        Self {
            causes: errors,
            ..Self::new(ErrorKind::Other, "Window operations failed")
        }
    }
    pub fn cancelled() -> Self {
        Self::new(ErrorKind::Cancelled, "Operation superseded")
    }
}
impl From<String> for BackendError {
    fn from(value: String) -> Self {
        Self::new(ErrorKind::Other, value)
    }
}
impl From<&str> for BackendError {
    fn from(value: &str) -> Self {
        value.to_owned().into()
    }
}
impl fmt::Display for BackendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.context)?;
        if let Some(code) = self.ax_code {
            write!(f, " (AX {code})")?;
        }
        for cause in &self.causes {
            write!(f, "\n{cause}")?;
        }
        Ok(())
    }
}
impl std::error::Error for BackendError {}
