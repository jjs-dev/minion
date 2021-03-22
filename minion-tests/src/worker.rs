//! Worker source code
use crate::TestCase;
use minion::{Backend as _, ChildProcess as _};
use std::sync::Arc;

// 16 mibibytes
const MEMORY_LIMIT_IN_BYTES: u64 = 4 * (1 << 20);

async fn inner_main(test_cases: &[&'static dyn TestCase]) {
    let test_case_name = std::env::var("TEST").unwrap();
    let test_case = test_cases
        .iter()
        .copied()
        .find(|&tc| tc.name() == test_case_name)
        .unwrap();

    let profile = std::env::var("PROFILE").unwrap();

    let resource_driver = {
        match profile.as_str() {
            "cgroup-v1" => minion::linux::ResourceDriverKind::CgroupV1,
            "cgroup-v2" => minion::linux::ResourceDriverKind::CgroupV2,
            "prlimit" | "prlimit-rootless" => minion::linux::ResourceDriverKind::Prlimit,
            _ => unreachable!(),
        }
    };

    let tempdir = tempfile::TempDir::new().expect("cannot create temporary dir");
    let mut settings = minion::linux::Settings::new();
    settings.resource_drivers = vec![resource_driver];
    if profile == "prlimit-rootless" {
        settings.rootless = true;
    }
    {
        let mut res = minion::CheckResult::new();
        minion::linux::check::check(&settings, &mut res);
        if res.has_errors() || res.has_warnings() {
            panic!("Settings validation failed: {}", res);
        }
    }
    let backend = minion::linux::LinuxBackend::new(settings).expect("backend creation failed");
    let opts = minion::SandboxOptions {
        cpu_time_limit: test_case.time_limit(),
        real_time_limit: test_case.real_time_limit(),
        max_alive_process_count: test_case.process_count_limit(),
        memory_limit: MEMORY_LIMIT_IN_BYTES,
        isolation_root: tempdir.path().to_path_buf(),
        shared_items: vec![minion::SharedItem {
            id: None,
            src: std::env::current_exe().unwrap(),
            dest: "/me".into(),
            kind: minion::SharedItemKind::Readonly,
            flags: vec![],
        }],
    };
    let sandbox = backend.new_sandbox(opts).expect("can not create sandbox");
    let sandbox = Arc::new(sandbox);
    let opts = minion::ChildProcessOptions {
        path: "/me".into(),
        arguments: vec![test_case.name().into()],
        environment: vec![format!("{}=1", crate::TEST_ENV_NAME).into()],
        stdio: minion::StdioSpecification {
            stdin: minion::InputSpecification::empty(),
            stdout: minion::OutputSpecification::pipe(),
            stderr: minion::OutputSpecification::pipe(),
        },
        sandbox: sandbox.clone(),
        pwd: "/".into(),
    };
    let mut cp = backend.spawn(opts).expect("failed to spawn child");
    let exit_code = cp
        .wait_for_exit()
        .expect("failed to start waiting")
        .await
        .expect("failed to wait for child");
    test_case.check(
        crate::CompletedChild {
            exit_code,
            stdout: &mut cp.stdout().unwrap(),
            stderr: &mut cp.stderr().unwrap(),
        },
        &*sandbox,
    );
}

// worker entry point
pub fn main(test_cases: &[&'static dyn TestCase]) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(inner_main(test_cases))
}
