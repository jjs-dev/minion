/// This functions checks for system configurations issues.
/// If it returns None, minion will probably work.
/// If it returns Some(s), s is human-readable string
/// describing these problems. It should be shown to administrtor,
/// so that they can fix this problem.
pub fn check(res: &mut CheckResult) {
    #[cfg(target_os = "linux")]
    {
        crate::linux::check::check(&crate::linux::Settings::default(), res);
    }
}

/// Storage for problems reported by `minion::check` and similar
/// functions.
#[derive(Debug, Default)]
pub struct CheckResult {
    errors: Vec<String>,
    warnings: Vec<String>,
}

impl CheckResult {
    /// Creates an empty CheckResult
    pub fn new() -> CheckResult {
        Default::default()
    }
    /// Records an error
    pub(crate) fn error(&mut self, message: &str) {
        self.errors.push(message.to_string())
    }
    /// Records a warning
    pub(crate) fn warning(&mut self, message: &str) {
        self.warnings.push(message.to_string())
    }
    /// Checks if any errors were reported
    pub fn has_errors(&self) -> bool {
        !self.errors.is_empty()
    }
}

impl std::fmt::Display for CheckResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.errors.is_empty() && self.warnings.is_empty() {
            return "OK".fmt(f);
        }
        if !self.errors.is_empty() {
            "Errors:\n".fmt(f)?;
            for err in &self.errors {
                format_args!("\t{}", err).fmt(f)?;
            }
        }
        if !self.warnings.is_empty() {
            "Warnings:\n".fmt(f)?;
            for warn in &self.warnings {
                format_args!("\t{}", warn).fmt(f)?;
            }
        }
        Ok(())
    }
}
