//! Implements windows WaitFuture
use crate::windows::{util::OwnedHandle, Cvt, Error};
use futures_util::task::AtomicWaker;
use std::{
    pin::Pin,
    sync::{
        atomic::{
            AtomicBool,
            Ordering::{Acquire, Release},
        },
        Arc,
    },
    task::{Context, Poll},
};
use winapi::{
    shared::winerror::WAIT_TIMEOUT,
    um::{
        minwinbase::STILL_ACTIVE,
        processthreadsapi::{GetExitCodeProcess, GetProcessId},
        synchapi::WaitForSingleObject,
        winbase::WAIT_OBJECT_0,
    },
};

/// Resolves when child has finished
pub struct WaitFuture {
    /// Child handle
    child: OwnedHandle,
    /// None if background thread has not been started yet
    shared: Option<Arc<Shared>>,
}

impl WaitFuture {
    fn get_exit_code(&self) -> Result<Option<crate::ExitCode>, Error> {
        let mut exit_code = 0;
        unsafe {
            Cvt::nonzero(GetExitCodeProcess(self.child.as_raw(), &mut exit_code))?;
        }
        if exit_code == STILL_ACTIVE {
            return Ok(None);
        }
        Ok(Some(crate::ExitCode(exit_code.into())))
    }
}

impl std::future::Future for WaitFuture {
    type Output = Result<crate::ExitCode, Error>;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = Pin::into_inner(self);

        if this.shared.is_none() {
            // we should start background thread

            let shared = Shared {
                waker: AtomicWaker::new(),
                error: AtomicBool::new(false),
            };

            // immediately register before starting background thread
            shared.waker.register(cx.waker());

            let shared = Arc::new(shared);
            this.shared.replace(shared.clone());

            let thread_name = unsafe {
                format!(
                    "minion-background-wait-{}",
                    Cvt::nonzero(GetProcessId(this.child.as_raw()) as i32).unwrap_or(-1)
                )
            };

            let child_handle = match this.child.try_clone() {
                Ok(cl) => cl,
                Err(err) => {
                    return Poll::Ready(Err(err));
                }
            };

            std::thread::Builder::new()
                .name(thread_name)
                .spawn(move || background_waiter(shared, child_handle))
                .expect("Failed to create a thread");
        }

        let shared = this.shared.as_mut().expect("initialized upper");

        // now register in the waker
        shared.waker.register(cx.waker());

        // win path
        if shared.error.load(Acquire) {
            return Poll::Ready(Err(Error::BackgroundThreadFailure));
        }

        if let Some(ec) = this.get_exit_code().transpose() {
            return Poll::Ready(ec);
        }

        Poll::Pending
    }
}

struct Shared {
    /// Task waiting for finish
    waker: AtomicWaker,
    /// Set when wait has errored
    error: AtomicBool,
}

fn background_waiter(mut shared: Arc<Shared>, handle: OwnedHandle) {
    loop {
        // check if someone is still interested in our work
        if Arc::get_mut(&mut shared).is_some() {
            // only our Shared part is alive.
            // it means WaitFuture is no longer alive.
            return;
        }

        // wait for one second
        let res = unsafe { WaitForSingleObject(handle.as_raw(), 1000) };
        if res == WAIT_OBJECT_0 {
            shared.waker.wake();
            return;
        }
        if res == WAIT_TIMEOUT {
            continue;
        }
        tracing::error!(
            return_value = res,
            "Unexpected return from WaitForSingleObject",
        );
        shared.error.store(true, Release);
        shared.waker.wake();
        return;
    }
}
