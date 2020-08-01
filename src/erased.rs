//! Contains type-erased minion API.
//! Useful for trait objects.
//!
//! Please note that this API is not type-safe. For example, if you pass
//! `Sandbox` instance to another backend, it will panic.

/// Type-erased `Sandbox`
pub trait Sandbox: std::fmt::Debug {
    fn id(&self) -> String;
    fn check_cpu_tle(&self) -> anyhow::Result<bool>;
    fn check_real_tle(&self) -> anyhow::Result<bool>;
    fn kill(&self) -> anyhow::Result<()>;
    fn resource_usage(&self) -> anyhow::Result<crate::ResourceUsageData>;
    #[doc(hidden)]
    fn clone_to_box(&self) -> Box<dyn Sandbox>;
    #[doc(hidden)]
    fn clone_into_box_any(&self) -> Box<dyn std::any::Any>;
}

impl Clone for Box<dyn Sandbox> {
    fn clone(&self) -> Self {
        self.clone_to_box()
    }
}

impl<S: crate::Sandbox> Sandbox for S {
    fn id(&self) -> String {
        self.id()
    }
    fn check_cpu_tle(&self) -> anyhow::Result<bool> {
        self.check_cpu_tle().map_err(Into::into)
    }
    fn check_real_tle(&self) -> anyhow::Result<bool> {
        self.check_real_tle().map_err(Into::into)
    }
    fn kill(&self) -> anyhow::Result<()> {
        self.kill().map_err(Into::into)
    }
    fn resource_usage(&self) -> anyhow::Result<crate::ResourceUsageData> {
        self.resource_usage().map_err(Into::into)
    }
    fn clone_to_box(&self) -> Box<dyn Sandbox> {
        Box::new(self.clone())
    }

    fn clone_into_box_any(&self) -> Box<dyn std::any::Any> {
        Box::new(self.clone())
    }
}

/// Type-erased `ChildProcess`
pub trait ChildProcess {
    fn get_exit_code(&self) -> anyhow::Result<Option<i64>>;
    fn stdin(&mut self) -> Option<Box<dyn std::io::Write + Send + Sync + 'static>>;
    fn stdout(&mut self) -> Option<Box<dyn std::io::Read + Send + Sync + 'static>>;
    fn stderr(&mut self) -> Option<Box<dyn std::io::Read + Send + Sync + 'static>>;
    fn wait_for_exit(
        &self,
        timeout: Option<std::time::Duration>,
    ) -> anyhow::Result<crate::WaitOutcome>;
    fn poll(&self) -> anyhow::Result<()>;
    fn is_finished(&self) -> anyhow::Result<bool>;
}

impl<C: crate::ChildProcess> ChildProcess for C {
    fn get_exit_code(&self) -> anyhow::Result<Option<i64>> {
        self.get_exit_code().map_err(Into::into)
    }
    fn stdin(&mut self) -> Option<Box<dyn std::io::Write + Send + Sync + 'static>> {
        match self.stdin() {
            Some(s) => Some(Box::new(s)),
            None => None,
        }
    }
    fn stdout(&mut self) -> Option<Box<dyn std::io::Read + Send + Sync + 'static>> {
        match self.stdout() {
            Some(s) => Some(Box::new(s)),
            None => None,
        }
    }
    fn stderr(&mut self) -> Option<Box<dyn std::io::Read + Send + Sync + 'static>> {
        match self.stderr() {
            Some(s) => Some(Box::new(s)),
            None => None,
        }
    }
    fn wait_for_exit(
        &self,
        timeout: Option<std::time::Duration>,
    ) -> anyhow::Result<crate::WaitOutcome> {
        self.wait_for_exit(timeout).map_err(Into::into)
    }
    fn poll(&self) -> anyhow::Result<()> {
        self.poll().map_err(Into::into)
    }
    fn is_finished(&self) -> anyhow::Result<bool> {
        self.is_finished().map_err(Into::into)
    }
}

/// Type-erased `Backend`
pub trait Backend {
    fn new_sandbox(&self, options: crate::SandboxOptions) -> anyhow::Result<Box<dyn Sandbox>>;
    fn spawn(&self, options: ChildProcessOptions) -> anyhow::Result<Box<dyn ChildProcess>>;
}

impl<B: crate::Backend> Backend for B {
    fn new_sandbox(&self, options: crate::SandboxOptions) -> anyhow::Result<Box<dyn Sandbox>> {
        let sb = <Self as crate::Backend>::new_sandbox(&self, options)?;
        Ok(Box::new(sb))
    }

    fn spawn(&self, options: ChildProcessOptions) -> anyhow::Result<Box<dyn ChildProcess>> {
        let down_sandbox = options
            .sandbox
            .clone_into_box_any()
            .downcast()
            .expect("sandbox type mismatch");
        let down_options = crate::ChildProcessOptions {
            arguments: options.arguments,
            environment: options.environment,
            path: options.path,
            pwd: options.pwd,
            stdio: options.stdio,
            sandbox: *down_sandbox,
        };
        let cp = <Self as crate::Backend>::spawn(&self, down_options)?;
        Ok(Box::new(cp))
    }
}

pub type ChildProcessOptions = crate::ChildProcessOptions<Box<dyn Sandbox>>;

/// Returns backend instance
pub fn setup() -> anyhow::Result<Box<dyn Backend>> {
    Ok(Box::new(crate::linux::LinuxBackend::new(
        crate::linux::Settings::new(),
    )?))
}
