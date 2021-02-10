//! Minion on windows

mod child;
mod constrain;
pub mod error;
mod isolate;
mod pipe;
mod sandbox;
mod spawn;
mod util;
mod wait;

pub use error::Error;
pub use pipe::{ReadPipe, WritePipe};
pub use sandbox::WindowsSandbox;

use error::Cvt;

/// Minion backend, supporting Windows.
#[derive(Debug)]
pub struct WindowsBackend {}

impl WindowsBackend {
    pub fn new() -> WindowsBackend {
        WindowsBackend {}
    }
}

impl crate::Backend for WindowsBackend {
    type Error = Error;
    type Sandbox = WindowsSandbox;
    type ChildProcess = child::WindowsChildProcess;

    fn new_sandbox(&self, options: crate::SandboxOptions) -> Result<Self::Sandbox, Self::Error> {
        let sandbox = sandbox::WindowsSandbox::create(options)?;
        Ok(sandbox)
    }

    fn spawn(
        &self,
        options: crate::ChildProcessOptions<Self::Sandbox>,
    ) -> Result<Self::ChildProcess, Self::Error> {
        child::WindowsChildProcess::create_process(options)
    }
}
