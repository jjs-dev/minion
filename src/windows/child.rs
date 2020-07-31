use crate::windows::{
    spawn::{spawn, ChildParams, Stdio},
    Error, ReadPipe, WindowsSandbox, WritePipe,
};
use winapi::um::{handleapi::CloseHandle, winnt::HANDLE};

#[derive(Debug)]
pub struct WindowsChildProcess {
    /// Handle to winapi Process object
    child: HANDLE,

    /// Handle to main process of child
    main_thread: HANDLE,

    /// Stdin
    stdin: Option<WritePipe>,
    /// Stdout
    stdout: Option<ReadPipe>,
    /// Stderr
    stderr: Option<ReadPipe>,
}

impl Drop for WindowsChildProcess {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.main_thread);
            CloseHandle(self.child);
        }
    }
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

        Ok(WindowsChildProcess {
            child: info.hProcess,
            main_thread: info.hThread,
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
