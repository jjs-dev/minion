use crate::linux::{
    jail_common::{JobQuery, Query},
    util::{Handle, IpcSocketExt, Pid, StraceLogger},
    zygote::{setup, spawn_job, JobOptions, SetupData, Stdio, ZygoteOptions},
};
use std::{io::Write, time::Duration};

unsafe fn process_spawn_query(
    arg: &mut ZygoteOptions,
    options: &JobQuery,
    setup_data: &SetupData,
) -> crate::Result<()> {
    let mut logger = StraceLogger::new();
    writeln!(logger, "got Spawn request").ok();
    // Now we do some preprocessing.
    let env: Vec<_> = options.environment.clone();

    let mut child_fds = arg
        .sock
        .recv_struct::<u64, [Handle; 3]>()
        .unwrap()
        .1
        .unwrap();
    for f in child_fds.iter_mut() {
        *f = nix::unistd::dup(*f).unwrap();
    }
    let child_stdio = Stdio::from_fd_array(child_fds);

    let job_options = JobOptions {
        exe: options.image_path.clone(),
        argv: options.argv.clone(),
        env,
        stdio: child_stdio,
        pwd: options.pwd.clone().into_os_string(),
    };

    writeln!(logger, "JobOptions are fetched").ok();
    let startup_info = spawn_job(job_options, setup_data, arg.jail_options.jail_id.clone())?;
    writeln!(logger, "job started. Sending startup_info back").ok();
    arg.sock.send(&startup_info)?;
    Ok(())
}

unsafe fn process_poll_query(
    arg: &mut ZygoteOptions,
    pid: Pid,
    timeout: Option<Duration>,
) -> crate::Result<()> {
    let res = super::timed_wait(pid, timeout)?;
    arg.sock.send(&res)?;
    Ok(())
}

pub(crate) unsafe fn zygote_entry(mut arg: ZygoteOptions) -> crate::Result<i32> {
    let setup_data = setup::setup(&arg.jail_options, &mut arg.sock)?;

    let mut logger = StraceLogger::new();
    loop {
        let query: Query = match arg.sock.recv() {
            Ok(q) => {
                writeln!(logger, "zygote: new request").ok();
                q
            }
            Err(err) => {
                writeln!(logger, "zygote: got unprocessable query: {}", err).ok();
                return Ok(23);
            }
        };
        match query {
            Query::Spawn(ref o) => process_spawn_query(&mut arg, o, &setup_data)?,
            Query::Exit => break,
            Query::Poll(p) => process_poll_query(&mut arg, p.pid, p.timeout)?,
        };
    }
    Ok(0)
}
