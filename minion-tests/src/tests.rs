mod simple;

use crate::TestCase;
use once_cell::sync::Lazy;
use std::io::Read;

pub static TESTS: Lazy<Vec<&'static dyn TestCase>> = Lazy::new(get_tests);

fn get_tests() -> Vec<&'static dyn TestCase> {
    vec![
        extend_lifetime(simple::TOk),
        extend_lifetime(simple::TTl),
        extend_lifetime(simple::TTlFork),
        extend_lifetime(simple::TIdle),
        extend_lifetime(simple::TRet1),
        extend_lifetime(simple::TOom),
        extend_lifetime(simple::TSecurity),
    ]
}

fn extend_lifetime<T: TestCase + 'static>(case: T) -> &'static dyn TestCase {
    Box::leak(Box::new(case))
}

// and here are some helper function

fn assert_empty(r: &mut dyn Read) {
    assert_contains(r, b"");
}

fn assert_contains(r: &mut dyn Read, expected: &[u8]) {
    let mut actual = Vec::new();
    r.read_to_end(&mut actual).expect("io error");
    if actual != expected {
        panic!(
            "file mismatch: expected `{:?}`, actual `{:?}`",
            String::from_utf8_lossy(&expected),
            String::from_utf8_lossy(&actual)
        );
    }
}

fn assert_killed(cp: crate::CompletedChild) {
    assert_exit_code(cp, minion::ExitCode::KILLED);
}

fn assert_exit_code(cp: crate::CompletedChild, exp_exit_code: minion::ExitCode) {
    let act_exit_code = cp.exit_code;
    assert_eq!(act_exit_code, exp_exit_code);
}
