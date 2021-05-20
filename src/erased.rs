//! Contains type-erased minion API.
//! Useful for trait objects.
//!
//! Please note that this API is not type-safe. For example, if you pass
//! `Sandbox` instance to another backend, it will panic.
use std::{any::Any, sync::Arc};

use futures_util::{FutureExt, TryFutureExt};

use crate::ChildProcessOptions;

/// Type-erased `Sandbox`
pub trait Sandbox: std::fmt::Debug + Send + Sync + 'static {
    fn id(&self) -> String;
    fn check_cpu_tle(&self) -> anyhow::Result<bool>;
    fn check_real_tle(&self) -> anyhow::Result<bool>;
    fn kill(&self) -> anyhow::Result<()>;
    fn resource_usage(&self) -> anyhow::Result<crate::ResourceUsageData>;
    fn debug_info(&self) -> anyhow::Result<serde_json::Value>;
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
    fn debug_info(&self) -> anyhow::Result<serde_json::Value> {
        self.debug_info().map_err(Into::into)
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
        self.stdin()
            .map(|x| Box::new(x) as Box<dyn std::io::Write + Send + Sync + 'static>)
    }
    fn stdout(&mut self) -> Option<Box<dyn std::io::Read + Send + Sync + 'static>> {
        self.stdout()
            .map(|x| Box::new(x) as Box<dyn std::io::Read + Send + Sync + 'static>)
    }
    fn stderr(&mut self) -> Option<Box<dyn std::io::Read + Send + Sync + 'static>> {
        self.stderr()
            .map(|x| Box::new(x) as Box<dyn std::io::Read + Send + Sync + 'static>)
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
    fn spawn(
        &self,
        options: ChildProcessOptions,
        sandbox: Arc<dyn Sandbox>,
    ) -> anyhow::Result<Box<dyn ChildProcess>>;
}

impl<B: crate::Backend> Backend for B {
    fn new_sandbox(&self, options: crate::SandboxOptions) -> anyhow::Result<Arc<dyn Sandbox>> {
        let sb = <Self as crate::Backend>::new_sandbox(&self, options)?;
        Ok(Arc::new(sb))
    }

    fn spawn(
        &self,
        options: ChildProcessOptions,
        sandbox: Arc<dyn Sandbox>,
    ) -> anyhow::Result<Box<dyn ChildProcess>> {
        let any_sandbox = sandbox.into_arc_any();
        let down_sandbox = any_sandbox.downcast().expect("sandbox type mismatch");
        let down_options = crate::ChildProcessOptions {
            path: options.path,
            arguments: options.arguments,
            environment: options.environment,
            stdio: options.stdio,
            extra_inherit: Vec::new(),
            pwd: options.pwd,
        };
        let cp = <Self as crate::Backend>::spawn(&self, down_options, down_sandbox)?;
        Ok(Box::new(cp))
    }
}

/// Returns backend instance
pub fn setup() -> anyhow::Result<Box<dyn Backend>> {
    Ok(Box::new(crate::linux::LinuxBackend::new(
        crate::linux::Settings::new(),
    )?))
}
