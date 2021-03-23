use seccomp::{Action, Context, Rule};
use std::{fmt::Write, os::unix::io::AsRawFd};
const DANGEROUS_SYSCALLS: &[i64] = &[
    libc::SYS_ptrace,
    libc::SYS_process_vm_readv,
    libc::SYS_process_vm_writev,
    libc::SYS_kill,
];

const SAFE_SYSCALLS: &[i64] = &[
    libc::SYS_exit,
    libc::SYS_fork,
    libc::SYS_clone,
    libc::SYS_read,
    libc::SYS_write,
    libc::SYS_wait4,
    libc::SYS_waitid,
    libc::SYS_execve,
];

fn new_deny_dangerous() -> anyhow::Result<Vec<u8>> {
    let mut cx = Context::default(Action::Allow)?;

    for &call in DANGEROUS_SYSCALLS {
        cx.add_rule(Rule::new(call as usize, None, Action::Errno(libc::EPERM)))?;
    }
    from_cx(cx)
}
fn new_unrestricted() -> anyhow::Result<Vec<u8>> {
    let cx = Context::default(Action::Allow)?;

    from_cx(cx)
}

fn new_pure() -> anyhow::Result<Vec<u8>> {
    let mut cx = Context::default(Action::Errno(libc::EPERM))?;

    for &call in SAFE_SYSCALLS {
        cx.add_rule(Rule::new(call as usize, None, Action::Allow))?;
    }
    from_cx(cx)
}

fn from_cx(cx: Context) -> anyhow::Result<Vec<u8>> {
    let f = tempfile::NamedTempFile::new()?;
    let fd = f.as_file().as_raw_fd();

    cx.export(fd)?;
    let data = std::fs::read(f.path())?;

    Ok(data)
}

fn export_policy(name: &str, data: &[u8]) -> String {
    format!("pub (in super) const {}: &[u8] = &{:?};", name, data)
}

pub fn gen() -> anyhow::Result<()> {
    let mut contents = String::new();
    writeln!(contents, "// this is @generated file").unwrap();
    writeln!(
        contents,
        "// codegen: minion-codegen/src/seccomp_policies.rs"
    )
    .unwrap();
    writeln!(
        contents,
        "{}",
        export_policy("UNRESTRICTED", &new_unrestricted()?)
    )
    .unwrap();
    writeln!(
        contents,
        "{}",
        export_policy("DENY_DANGEROUS", &new_deny_dangerous()?)
    )
    .unwrap();
    writeln!(contents, "{}", export_policy("PURE", &new_pure()?)).unwrap();

    crate::put_file("src/linux/seccomp/gen.rs", &contents)?;
    Ok(())
}
