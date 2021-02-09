//! Implements wait future
use crate::{
    linux::{util::Pid, LinuxSandbox},
    ExitCode,
};
use std::{
    os::unix::io::RawFd,
    pin::Pin,
    task::{Context, Poll},
};
use tokio::io::unix::AsyncFd;

/// Owns file descriptor.
struct OwnedFd(RawFd);

impl std::os::unix::io::AsRawFd for OwnedFd {
    fn as_raw_fd(&self) -> RawFd {
        self.0
    }
}

impl Drop for OwnedFd {
    fn drop(&mut self) {
        nix::unistd::close(self.0).unwrap();
    }
}

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
    inner: AsyncFd<OwnedFd>,
    sandbox: LinuxSandbox,
    pid: Pid,
}

impl WaitFuture {
    pub(crate) fn new(
        fd: RawFd,
        pid: Pid,
        sandbox: LinuxSandbox,
    ) -> Result<Self, crate::linux::Error> {
        let inner = AsyncFd::new(OwnedFd(fd))?;
        Ok(WaitFuture {
            inner,
            pid,
            sandbox,
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
