use crate::{
    erased, ChildProcessOptions, InputSpecification, OutputSpecification, StdioSpecification,
};
use std::{
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
    sync::Arc,
};

/// Child process builder
#[derive(Default, Debug)]
pub struct Command {
    sandbox: Option<Arc<dyn erased::Sandbox>>,
    exe: Option<PathBuf>,
    argv: Vec<OsString>,
    env: Vec<OsString>,
    stdin: Option<InputSpecification>,
    stdout: Option<OutputSpecification>,
    stderr: Option<OutputSpecification>,
    current_dir: Option<PathBuf>,
}

impl Command {
    pub fn build(self) -> Option<ChildProcessOptions> {
        let create_default_in_channel = || InputSpecification::empty();
        let create_default_out_channel = || OutputSpecification::ignore();
        let opts = ChildProcessOptions {
            path: self.exe.clone()?,
            arguments: self.argv.clone(),
            environment: self.env.clone(),
            extra_inherit: Vec::new(),
            stdio: StdioSpecification {
                stdin: self.stdin.unwrap_or_else(create_default_in_channel),
                stdout: self.stdout.unwrap_or_else(create_default_out_channel),
                stderr: self.stderr.unwrap_or_else(create_default_out_channel),
            },
            pwd: self.current_dir.clone().unwrap_or_else(|| "/".into()),
        };
        Some(opts)
    }

    pub fn new() -> Command {
        Default::default()
    }

    pub fn spawn(
        self,
        backend: &dyn erased::Backend,
    ) -> anyhow::Result<Box<dyn erased::ChildProcess>> {
        let options = self
            .build()
            .expect("spawn() was requested, but required fields were not set");
        let sandbox = self
            .sandbox
            .clone()
            .expect("spawn() was requested, but no sandbox was set");
        backend.spawn(options, sandbox)
    }

    pub fn sandbox(&mut self, sandbox: Arc<dyn erased::Sandbox>) -> &mut Self {
        self.sandbox.replace(sandbox);
        self
    }

    pub fn path<S: AsRef<Path>>(&mut self, path: S) -> &mut Self {
        self.exe.replace(path.as_ref().to_path_buf());
        self
    }

    pub fn arg<S: AsRef<OsStr>>(&mut self, a: S) -> &mut Self {
        self.argv.push(a.as_ref().to_os_string());
        self
    }

    pub fn args(&mut self, args: impl IntoIterator<Item = impl AsRef<OsStr>>) -> &mut Self {
        self.argv
            .extend(args.into_iter().map(|s| s.as_ref().to_os_string()));
        self
    }

    pub fn env(&mut self, var: impl AsRef<OsStr>) -> &mut Self {
        self.env.push(var.as_ref().to_os_string());
        self
    }

    pub fn envs(&mut self, items: impl IntoIterator<Item = impl AsRef<OsStr>>) -> &mut Self {
        self.env
            .extend(items.into_iter().map(|var| var.as_ref().to_os_string()));
        self
    }

    pub fn current_dir<S: AsRef<Path>>(&mut self, a: S) -> &mut Self {
        self.current_dir.replace(a.as_ref().to_path_buf());
        self
    }

    pub fn stdin(&mut self, stdin: InputSpecification) -> &mut Self {
        self.stdin.replace(stdin);
        self
    }

    pub fn stdout(&mut self, stdout: OutputSpecification) -> &mut Self {
        self.stdout.replace(stdout);
        self
    }

    pub fn stderr(&mut self, stderr: OutputSpecification) -> &mut Self {
        self.stderr.replace(stderr);
        self
    }
}
