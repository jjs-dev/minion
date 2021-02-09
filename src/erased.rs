//! Contains type-erased minion API.
//! Useful for trait objects.
//!
//! Please note that this API is not type-safe. For example, if you pass
//! `Sandbox` instance to another backend, it will panic.
use std::{any::Any, sync::Arc};

use futures_util::{FutureExt, TryFutureExt};

/// Type-erased `Sandbox`
pub trait Sandbox: std::fmt::Debug + Send + Sync + 'static {
    fn id(&self) -> String;
    fn check_cpu_tle(&self) -> anyhow::Result<bool>;
    fn check_real_tle(&self) -> anyhow::Result<bool>;
    fn kill(&self) -> anyhow::Result<()>;
    fn resource_usage(&self) -> anyhow::Result<crate::ResourceUsageData>;
    fn into_arc_any(self: Arc<Self>) -> Arc<dyn Any + Send + Sync + 'static>;
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
    fn into_arc_any(self: Arc<Self>) -> Arc<dyn Any + Send + Sync + 'static> {
        self
    }
}

/// Type-erased `ChildProcess`
pub trait ChildProcess: Send + Sync + 'static {
    fn stdin(&mut self) -> Option<Box<dyn std::io::Write + Send + Sync + 'static>>;
    fn stdout(&mut self) -> Option<Box<dyn std::io::Read + Send + Sync + 'static>>;
    fn stderr(&mut self) -> Option<Box<dyn std::io::Read + Send + Sync + 'static>>;
    fn wait_for_exit(
        &mut self,
    ) -> anyhow::Result<futures_util::future::BoxFuture<'static, anyhow::Result<crate::ExitCode>>>;
}

impl<C: crate::ChildProcess> ChildProcess for C {
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
        &mut self,
    ) -> anyhow::Result<futures_util::future::BoxFuture<'static, anyhow::Result<crate::ExitCode>>>
    {
        Ok(self.wait_for_exit()?.map_err(Into::into).boxed())
    }
}

/// Type-erased `Backend`
pub trait Backend: Send + Sync + 'static {
    fn new_sandbox(&self, options: crate::SandboxOptions) -> anyhow::Result<Arc<dyn Sandbox>>;
    fn spawn(&self, options: ChildProcessOptions) -> anyhow::Result<Box<dyn ChildProcess>>;
}

impl<B: crate::Backend> Backend for B {
    fn new_sandbox(&self, options: crate::SandboxOptions) -> anyhow::Result<Arc<dyn Sandbox>> {
        let sb = <Self as crate::Backend>::new_sandbox(&self, options)?;
        Ok(Arc::new(sb))
    }

    fn spawn(&self, options: ChildProcessOptions) -> anyhow::Result<Box<dyn ChildProcess>> {
        let any_sandbox = options.sandbox.clone().into_arc_any();
        let down_sandbox = any_sandbox.downcast().expect("sandbox type mismatch");
        let down_options = crate::ChildProcessOptions {
            arguments: options.arguments,
            environment: options.environment,
            path: options.path,
            pwd: options.pwd,
            stdio: options.stdio,
            sandbox: down_sandbox,
        };
        let cp = <Self as crate::Backend>::spawn(&self, down_options)?;
        Ok(Box::new(cp))
    }
}

pub type ChildProcessOptions = crate::ChildProcessOptions<dyn Sandbox>;

/// Returns backend instance
pub fn setup() -> anyhow::Result<Box<dyn Backend>> {
    Ok(Box::new(crate::linux::LinuxBackend::new(
        crate::linux::Settings::new(),
    )?))
}
