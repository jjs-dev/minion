//! Implements wait future
use crate::{
    linux::{fd::Fd, util::Pid, LinuxSandbox},
    ExitCode,
};
use std::{
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use tokio::io::unix::AsyncFd;

/// Future that resolves when child process exits.
/// This future internally can work in two mods
/// # Pidfd mode
/// On modern kernels (i.e. >= 5.3) this future makes use of pidfd (you
/// can use `minion::linux::check::pidfd_supported()` to check for this).
/// These descriptors are spawned onto background reactor. That way only one
/// thread is used for all futures.
/// # Legacy mode
/// On old kernels this future spawns background thread that polls child
/// and monitors it completion using pipe.
pub struct WaitFuture {
    /// FD of underlying event source (either pidfd or unix socket)
    // TODO: use pipe instead of socket
    inner: AsyncFd<Fd>,
    sandbox: Arc<LinuxSandbox>,
    pid: Pid,
}

impl WaitFuture {
    pub(crate) fn new(
        fd: Fd,
        pid: Pid,
        sandbox: Arc<LinuxSandbox>,
    ) -> Result<Self, crate::linux::Error> {
        let inner = AsyncFd::new(fd)?;
        Ok(WaitFuture {
            inner,
            sandbox,
            pid,
        })
    }
}

impl std::future::Future for WaitFuture {
    type Output = Result<ExitCode, crate::linux::Error>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = Pin::into_inner(self);
        this.inner
            .poll_read_ready(cx)
            .map_ok(|_| this.sandbox.get_exit_code(this.pid))
            .map_err(Into::into)
    }
}
