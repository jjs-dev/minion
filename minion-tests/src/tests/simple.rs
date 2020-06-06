//! Tests that simple program that does nothing completes successfully.
use minion::ChildProcess;
use minion::Dominion;

pub(crate) struct TOk;
impl crate::TestCase for TOk {
    fn name(&self) -> &'static str {
        "test_ok"
    }
    fn description(&self) -> &'static str {
        "tests that exit(0) program works"
    }
    fn test(&self) -> ! {
        std::process::exit(0)
    }
    fn check(&self, cp: &mut dyn ChildProcess, _: minion::DominionRef) {
        assert!(matches!(cp.get_exit_code(), Ok(Some(0))));
        super::assert_empty(&mut cp.stdout().unwrap());
        super::assert_empty(&mut cp.stderr().unwrap());
    }
    fn real_time_limit(&self) -> std::time::Duration {
        std::time::Duration::from_secs(5)
    }
}

pub(crate) struct TTl;
impl crate::TestCase for TTl {
    fn name(&self) -> &'static str {
        "test_cpu_time_limit_exceeded"
    }
    fn description(&self) -> &'static str {
        "contains program that always does work \
        and checks that is it terminated"
    }
    fn test(&self) -> ! {
        exceed_time_limit()
    }
    fn check(&self, cp: &mut dyn ChildProcess, _: minion::DominionRef) {
        super::assert_killed(cp);
    }
}

pub(crate) struct TTlFork;
impl crate::TestCase for TTlFork {
    fn name(&self) -> &'static str {
        "test_cpu_time_limit_with_fork"
    }
    fn description(&self) -> &'static str {
        "launches two threads that consume time \
        and checks that they are killed because of CPU time limit \
        (TODO: verify this is not wall-clock time limit)"
    }
    fn test(&self) -> ! {
        nix::unistd::fork().unwrap();
        exceed_time_limit()
    }
    fn check(&self, cp: &mut dyn ChildProcess, _: minion::DominionRef) {
        super::assert_killed(cp);
    }
    fn process_count_limit(&self) -> u32 {
        2
    }
}

pub(crate) struct TIdle;
impl crate::TestCase for TIdle {
    fn name(&self) -> &'static str {
        "test_idleness_limit_exceeded"
    }
    fn description(&self) -> &'static str {
        "launches program that sleeps for very long time \
        checks that it still will be killed"
    }
    fn test(&self) -> ! {
        nix::unistd::sleep(1_000_000_000);
        std::process::exit(0)
    }
    fn check(&self, cp: &mut dyn ChildProcess, _: minion::DominionRef) {
        super::assert_killed(cp);
    }
}

pub(crate) struct TRet1;
impl crate::TestCase for TRet1 {
    fn name(&self) -> &'static str {
        "test_exit_nonzero"
    }
    fn description(&self) -> &'static str {
        "launches simple program that returns 1"
    }
    fn test(&self) -> ! {
        std::process::exit(1);
    }
    fn check(&self, cp: &mut dyn ChildProcess, _: minion::DominionRef) {
        super::assert_exit_code(cp, 1);
    }
}

pub(crate) struct TOom;
impl crate::TestCase for TOom {
    fn name(&self) -> &'static str {
        "test_out_of_memory"
    }
    fn description(&self) -> &'static str {
        "launches program that consumes more memory than allowed \
        and checks that program was killed"
    }
    fn test(&self) -> ! {
        unsafe {
            const ALLOC_SIZE: usize = 1 << 26;
            let layout = std::alloc::Layout::array::<u8>(ALLOC_SIZE).unwrap();
            let mem = std::alloc::alloc_zeroed(layout);
            const PAGE_SIZE: usize = 1 << 12;
            *mem = 45;
            for i in 0..ALLOC_SIZE / PAGE_SIZE {
                let i = i * PAGE_SIZE;
                *mem.add(i) = *mem.add(i / 2) + 3;
            }
            let mut hash = 0usize;
            // this should prevent optimizer from removing loop
            for j in 0..ALLOC_SIZE {
                hash = hash.wrapping_add(*mem.add(j) as usize).wrapping_mul(3);
            }
            // should be unreachable
            std::process::exit((hash % 256) as i32);
        }
    }

    fn check(&self, cp: &mut dyn ChildProcess, d: minion::DominionRef) {
        // TODO this test is broken
        // super::assert_exit_code(cp, -9);
        // assert!(!d.check_cpu_tle().unwrap());
        // assert!(!d.check_real_tle().unwrap());
    }
    fn real_time_limit(&self) -> std::time::Duration {
        std::time::Duration::from_secs(20)
    }
    fn time_limit(&self) -> std::time::Duration {
        std::time::Duration::from_secs(10)
    }
}

fn exceed_time_limit() -> ! {
    loop {
        unsafe {
            asm!("nop");
        }
    }
}