#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("winapi call failed: {errno}")]
    Syscall { errno: u32 },
    #[error("hresult call failed: {hresult}")]
    Hresult { hresult: i32 },
    #[error("background thread failed")]
    BackgroundThreadFailure,
}

impl From<u32> for Error {
    fn from(errno: u32) -> Self {
        Error::Syscall { errno }
    }
}

impl Error {
    pub(crate) fn last() -> Self {
        let errno = unsafe { winapi::um::errhandlingapi::GetLastError() };
        if cfg!(debug_assertions) {
            tracing::error!(errno = errno, backtrace = ?backtrace::Backtrace::new(), "win32 error");
        } else {
            tracing::error!(errno = errno, "win32 error");
        };
        Error::Syscall { errno }
    }
}

/// Helper for checking return values
pub(crate) struct Cvt {
    _priv: (),
}

impl Cvt {
    /// checks that operation returned non-zero
    pub fn nonzero(ret: i32) -> Result<i32, Error> {
        if ret != 0 {
            Ok(ret)
        } else {
            Err(Error::last())
        }
    }

    /// Checks HRESULT is successful
    pub fn hresult(hr: winapi::shared::winerror::HRESULT) -> Result<(), Error> {
        if winapi::shared::winerror::SUCCEEDED(hr) {
            Ok(())
        } else {
            tracing::error!(result = hr, "Unsuccessful HRESULT");
            Err(Error::Hresult { hresult: hr })
        }
    }
}
