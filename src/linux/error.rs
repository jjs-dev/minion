use crate::linux::{
    cgroup::{CgroupDetectionError, CgroupError},
    ipc::IpcError,
};
#[derive(Eq, PartialEq)]
pub enum ErrorKind {
    /// This error typically means that isolated process tried to break its sandbox
    Sandbox,
    /// Bug in code, using minion, or in minion itself
    System,
}

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    #[error("requested operation is not supported by backend")]
    NotSupported,
    #[error("system call failed in undesired fashion (error code {})", code)]
    Syscall { code: i32 },
    #[error("io error")]
    Io {
        #[from]
        cause: std::io::Error,
    },
    #[error("sandbox interaction failed")]
    SandboxMisbehavior {
        #[source]
        cause: Option<IpcError>,
    },
    #[error("unknown error")]
    Unknown,
    #[error("Cgroup detection failure")]
    CgroupDetection {
        #[from]
        cause: CgroupDetectionError,
    },
    #[error("Cgroup manipulation failed")]
    Cgroup {
        #[from]
        cause: CgroupError,
    },
    #[error("sandbox ipc error")]
    SandboxIpc {
        #[from]
        cause: IpcError,
    },
    #[error("Invalid flag used in SharedItem: {flag}")]
    InvalidSharedItemFlag { flag: String },
}

impl Error {
    pub fn kind(&self) -> ErrorKind {
        match self {
            Error::NotSupported => ErrorKind::System,
            Error::Syscall { .. } => ErrorKind::System,
            Error::Io { .. } => ErrorKind::System,
            Error::SandboxMisbehavior { .. } => ErrorKind::Sandbox,
            Error::Unknown => ErrorKind::System,
            Error::Cgroup { .. } => ErrorKind::System,
            Error::CgroupDetection { .. } => ErrorKind::System,
            Error::SandboxIpc { .. } => ErrorKind::Sandbox,
            Error::InvalidSharedItemFlag { .. } => ErrorKind::System,
        }
    }

    pub fn is_system(&self) -> bool {
        self.kind() == ErrorKind::System
    }

    pub fn is_sandbox(&self) -> bool {
        self.kind() == ErrorKind::Sandbox
    }
}

impl From<nix::Error> for Error {
    fn from(err: nix::Error) -> Self {
        if let Some(errno) = err.as_errno() {
            Error::Syscall { code: errno as i32 }
        } else {
            Error::Unknown
        }
    }
}
