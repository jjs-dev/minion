//! Defines IPC utilities.
//! Currently we  `nix` directly but we will switch to std/tokio when
//! std APIs for fd passing are stable
use crate::linux::fd::Fd;
use nix::sys::{
    socket::{
        recvmsg, sendmsg, socketpair, AddressFamily, ControlMessage, ControlMessageOwned, MsgFlags,
        SockFlag, SockType,
    },
    uio::IoVec,
};
use serde::{de::DeserializeOwned, Serialize};
use std::{
    any::TypeId,
    hash::{Hash, Hasher},
    io::Write,
};

#[derive(thiserror::Error, Debug)]
pub enum IpcError {
    #[error("serialization error")]
    Serde(#[from] serde_json::Error),
    #[error("syscall failed")]
    Syscall(#[from] nix::Error),
    #[error("unexpected ancillary messages")]
    Ancillary,
}

#[derive(Debug)]
pub struct Socket {
    fd: Fd,
}

const FD_CHUNK_SIZE: usize = 8;

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

    pub fn send<T: Serialize + 'static>(&mut self, message: &T) -> Result<(), IpcError> {
        let message = serde_json::to_vec(&message)?;
        let len = (message.len()).to_ne_bytes();
        let id = typeid::<T>().to_ne_bytes();

        let iov_len = IoVec::from_slice(&len);
        let iov_id = IoVec::from_slice(&id);
        let iov_data = IoVec::from_slice(&message);
        sendmsg(
            self.fd.as_raw(),
            &[iov_len, iov_id],
            &[],
            MsgFlags::empty(),
            None,
        )?;
        sendmsg(self.fd.as_raw(), &[iov_data], &[], MsgFlags::empty(), None)?;
        Ok(())
    }

    pub fn recv<T: DeserializeOwned + 'static>(&mut self) -> Result<T, IpcError> {
        let mut len = [0; 8];
        let mut id = [0; 8];
        recvmsg(
            self.fd.as_raw(),
            &[
                IoVec::from_mut_slice(&mut len),
                IoVec::from_mut_slice(&mut id),
            ],
            None,
            MsgFlags::empty(),
        )?;
        let len = usize::from_ne_bytes(len);
        let id = u64::from_ne_bytes(id);

        assert_eq!(id, typeid::<T>());

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
        let iov = IoVec::from_slice(b"_");

        let chunks = fds.chunks(FD_CHUNK_SIZE);
        for chunk in chunks {
            let mut raw_fds = [0; FD_CHUNK_SIZE];
            for (i, x) in chunk.iter().enumerate() {
                raw_fds[i] = x.as_raw();
            }

            sendmsg(
                self.fd.as_raw(),
                &[iov],
                &[ControlMessage::ScmRights(&raw_fds[..chunk.len()])],
                MsgFlags::empty(),
                None,
            )?;
        }
        Ok(())
    }

    pub fn recv_fds(&mut self, fd_count: usize) -> Result<Vec<Fd>, IpcError> {
        let mut received = Vec::<Option<Fd>>::new();
        received.resize_with(fd_count, || None);
        let mut buf = [0; 1];
        let mut cmsg_space = nix::cmsg_space!([Fd; FD_CHUNK_SIZE]);

        for chunk in received.chunks_mut(FD_CHUNK_SIZE) {
            let iov = IoVec::from_mut_slice(&mut buf);
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
                    if fds.len() != chunk.len() {
                        return Err(IpcError::Ancillary);
                    }
                    for (i, x) in fds.iter().enumerate() {
                        chunk[i] = Some(Fd::new(*x));
                    }
                }
                _ => return Err(IpcError::Ancillary),
            }
        }
        Ok(received.into_iter().map(Option::unwrap).collect())
    }
}

fn typeid<T: 'static>() -> u64 {
    let t = TypeId::of::<T>();
    let mut h = std::collections::hash_map::DefaultHasher::new();
    t.hash(&mut h);
    let id = h.finish();

    let mut logger = crate::linux::util::strace_logger();
    writeln!(logger, "{} -> {}", std::any::type_name::<T>(), id).unwrap();
    id
}
