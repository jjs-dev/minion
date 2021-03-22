//! Minion testing framework
//! Actual tests are defined in tests module.
//! By convention, each test is struct named `T`.
//! # Details
//! Testing lifecycle is rather complicated.
//! At first, we have have **master** process, which is spawned directly
//! by user. Also, we have **worker** processes, which are forked from
//! master. Finally, test itself is executed inside another process.
//! I.e.
//! Master: parses CLI, selects tests, spawns Workers
//! Worker: sets up sandbox, executes `minion-tests` in Test mode.
//! Test: just executes test selected by name
#![feature(asm, test)]
mod master;
mod tests;
mod worker;

pub struct CompletedChild<'a> {
    pub exit_code: minion::ExitCode,
    // stdin: &'a mut dyn std::io::Write,
    pub stdout: &'a mut dyn std::io::Read,
    pub stderr: &'a mut dyn std::io::Read,
}

impl<'a> CompletedChild<'a> {
    pub fn by_ref(&mut self) -> CompletedChild<'_> {
        CompletedChild {
            exit_code: self.exit_code,
            stdout: &mut *self.stdout,
            stderr: &mut *self.stderr,
        }
    }
}

/// Each test implements this trait.
pub trait TestCase: Send + Sync {
    /// Returns test name. Executed on master.
    fn name(&self) -> &'static str;
    /// Test description.
    fn description(&self) -> &'static str;
    /// Test itself. Executed on worker inside minion sandbox.
    fn test(&self) -> !;
    /// Validates that test was successful,
    /// given completed `ChildProcess` object.
    /// If tests passed, does nothing otherwise
    /// panics.
    /// Executed on worker.
    fn check(&self, cp: CompletedChild, sb: &dyn minion::erased::Sandbox);
    /// Overrides CPU time limit
    fn time_limit(&self) -> std::time::Duration {
        std::time::Duration::from_secs(1)
    }
    /// Overrides wall-clock time limit
    fn real_time_limit(&self) -> std::time::Duration {
        std::time::Duration::from_secs(2)
    }
    /// Overrides process limit
    fn process_count_limit(&self) -> u32 {
        1
    }
    /// If this method returns false, test is skipped
    fn filter(&self, _profile: &str) -> bool {
        true
    }
}

static WORKER_ENV_NAME: &str = "__MINION_ROLE_IS_WORKER__";
static TEST_ENV_NAME: &str = "__MINION_ROLE_IS_TEST__";
fn main() {
    tracing_subscriber::fmt()
        .with_writer(std::io::stdout)
        .with_max_level(tracing::Level::DEBUG)
        .init();
    let test_cases = &*tests::TESTS;
    let role = get_role();
    match role {
        Role::Master => master::main(&test_cases),
        Role::Worker => worker::main(&test_cases),
        Role::Test => {
            let test_name = std::env::args().nth(1).unwrap();
            let test = test_cases
                .iter()
                .copied()
                .find(|&tc| tc.name() == test_name)
                .unwrap();
            test.test();
        }
    }
}

fn get_role() -> Role {
    match std::env::var(WORKER_ENV_NAME) {
        Ok(_) => Role::Worker,
        Err(_) => match std::env::var(TEST_ENV_NAME) {
            Ok(_) => Role::Test,
            Err(_) => Role::Master,
        },
    }
}

enum Role {
    Master,
    Worker,
    Test,
}
