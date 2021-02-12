//! Defines IPC utilities.
//! Currently we  `nix` directly but we will switch to std/tokio when
//! std APIs for fd passing are stable
// TODO: defend against DoS
use crate::linux::fd::Fd;
use nix::sys::{
    socket::{
        recvmsg, sendmsg, socketpair, AddressFamily, ControlMessage, ControlMessageOwned, MsgFlags,
        SockFlag, SockType,
    },
    uio::IoVec,
};
use serde::{de::DeserializeOwned, Serialize};

#[derive(thiserror::Error, Debug)]
pub enum IpcError {
    #[error("serialization error")]
    Serde(#[from] serde_json::Error),
    #[error("syscall failed")]
    Syscall(#[from] nix::Error),
    #[error("unexpected ancillary messages")]
    Ancillary,
}

pub struct Socket {
    fd: Fd,
}

const MAX_FD_COUNT: usize = 3;

impl Socket {
    pub fn pair() -> Result<(Self, Self), IpcError> {
        let (a, b) = socketpair(
            AddressFamily::Unix,
            SockType::SeqPacket,
            None,
            SockFlag::SOCK_CLOEXEC,
        )?;
        Ok((Socket { fd: Fd::new(a) }, Socket { fd: Fd::new(b) }))
    }

    pub fn inner(&self) -> &Fd {
        &self.fd
    }

    pub fn send<T: Serialize>(&mut self, message: &T) -> Result<(), IpcError> {
        let message = serde_json::to_vec(&message)?;
        let len = (message.len()).to_ne_bytes();

        let iov_len = IoVec::from_slice(&len);
        let iov_data = IoVec::from_slice(&message);
        sendmsg(self.fd.as_raw(), &[iov_len], &[], MsgFlags::empty(), None)?;
        sendmsg(self.fd.as_raw(), &[iov_data], &[], MsgFlags::empty(), None)?;
        Ok(())
    }

    pub fn recv<T: DeserializeOwned>(&mut self) -> Result<T, IpcError> {
        let mut len = [0; 8];
        recvmsg(
            self.fd.as_raw(),
            &[IoVec::from_mut_slice(&mut len)],
            None,
            MsgFlags::empty(),
        )?;
        let len = usize::from_ne_bytes(len);

        let mut message = vec![0; len];
        recvmsg(
            self.fd.as_raw(),
            &[IoVec::from_mut_slice(&mut message)],
            None,
            MsgFlags::empty(),
        )?;
        let message = serde_json::from_slice(&message)?;
        Ok(message)
    }

    pub fn send_fds(&mut self, fds: &[Fd]) -> Result<(), IpcError> {
        assert!(fds.len() <= MAX_FD_COUNT);
        let iov = IoVec::from_slice(b"_");
        let raw_fds = fds.iter().map(|fd| fd.as_raw()).collect::<Vec<_>>();

        sendmsg(
            self.fd.as_raw(),
            &[iov],
            &[ControlMessage::ScmRights(&raw_fds)],
            MsgFlags::empty(),
            None,
        )?;
        Ok(())
    }

    pub fn recv_fds(&mut self, fd_count: usize) -> Result<Vec<Fd>, IpcError> {
        assert!(fd_count <= MAX_FD_COUNT);
        let mut buf = [0; 1];
        let iov = IoVec::from_mut_slice(&mut buf);
        let mut cmsg_space = nix::cmsg_space!([Fd; MAX_FD_COUNT]);
        let msg = recvmsg(
            self.fd.as_raw(),
            &[iov],
            Some(&mut cmsg_space),
            MsgFlags::empty(),
        )?;
        let mut cmsgs = msg.cmsgs();
        let next = cmsgs.next().ok_or(IpcError::Ancillary)?;
        match next {
            ControlMessageOwned::ScmRights(fds) => {
                if fds.len() != fd_count {
                    return Err(IpcError::Ancillary);
                }

                Ok(fds.into_iter().map(Fd::new).collect())
            }
            _ => Err(IpcError::Ancillary),
        }
    }
}
