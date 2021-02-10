use crate::windows::{
    spawn::{spawn, ChildParams, Stdio},
    util::OwnedHandle,
    Error, ReadPipe, WindowsSandbox, WritePipe,
};

#[derive(Debug)]
pub struct WindowsChildProcess {
    /// Handle to winapi Process object
    child: OwnedHandle,

    /// Handle to main process of child
    main_thread: OwnedHandle,

    /// Stdin
    stdin: Option<WritePipe>,
    /// Stdout
    stdout: Option<ReadPipe>,
    /// Stderr
    stderr: Option<ReadPipe>,
}

impl WindowsChildProcess {
    pub(in crate::windows) fn create_process(
        options: crate::ChildProcessOptions<WindowsSandbox>,
    ) -> Result<Self, Error> {
        let (stdio, (child_stdin, child_stdout, child_stderr)) = Stdio::make(options.stdio)?;
        let info = spawn(
            &options.sandbox,
            stdio,
            ChildParams {
                exe: options.path.into(),
                argv: options.arguments,
                env: options.environment,
                cwd: options.pwd.into(),
            },
        )?;
        let child = OwnedHandle::new(info.hProcess);
        options.sandbox.job.add_process(&child)?;

        Ok(WindowsChildProcess {
            child,
            main_thread: OwnedHandle::new(info.hThread),
            stdin: child_stdin,
            stdout: child_stdout,
            stderr: child_stderr,
        })
    }
}

impl crate::ChildProcess for WindowsChildProcess {
    type Error = Error;
    type PipeIn = WritePipe;
    type PipeOut = ReadPipe;
    type WaitFuture = crate::windows::wait::WaitFuture;

    fn stdin(&mut self) -> Option<Self::PipeIn> {
        self.stdin.take()
    }
    fn stdout(&mut self) -> Option<Self::PipeOut> {
        self.stdout.take()
    }
    fn stderr(&mut self) -> Option<Self::PipeOut> {
        self.stderr.take()
    }
    fn wait_for_exit(&mut self) -> Result<Self::WaitFuture, Self::Error> {
        todo!()
    }
}
