//! Master source code
use crate::TestCase;

#[derive(Copy, Clone)]
enum Outcome {
    Success,
    Failure,
}

impl Outcome {
    fn and(self, that: Outcome) -> Outcome {
        match self {
            Outcome::Failure => Outcome::Failure,
            Outcome::Success => that,
        }
    }
}

// master entry point
pub fn main(test_cases: &[&'static dyn TestCase]) {
    let matches = clap::App::new("minion-tests")
        .arg(
            clap::Arg::with_name("test-filter")
                .long("test-filter")
                .takes_value(true),
        )
        .arg(
            clap::Arg::with_name("trace")
                .long("trace")
                .takes_value(false),
        )
        .get_matches();
    check_static();

    let filter = get_filter(&matches);
    let filtered_test_cases: Vec<_> = test_cases
        .iter()
        .copied()
        .filter(|&tc| filter(tc))
        .collect();
    let opts = ExecuteOptions {
        trace: matches.is_present("trace"),
    };
    let outcome = execute_tests(&filtered_test_cases, opts);
    println!("------ execution done ------");
    match outcome {
        Outcome::Success => println!("tests succeeded"),
        Outcome::Failure => {
            println!("some tests failed");
            std::process::exit(1);
        }
    }
}

#[derive(Copy, Clone)]
struct ExecuteOptions {
    trace: bool,
}

fn execute_tests(test_cases: &[&dyn TestCase], exec_opts: ExecuteOptions) -> Outcome {
    println!("will run:");
    for &case in test_cases {
        println!("  - {}", case.name());
    }
    println!("({} tests)", test_cases.len());
    let mut outcome = Outcome::Success;
    for &case in test_cases {
        outcome = outcome.and(execute_single_test(case, exec_opts));
    }
    outcome
}

fn execute_single_test(case: &dyn TestCase, exec_opts: ExecuteOptions) -> Outcome {
    println!("------ {} ------", case.name());
    let self_exe = std::env::current_exe().unwrap();
    let mut cmd = if exec_opts.trace {
        let mut cmd = std::process::Command::new("strace");
        cmd.arg("-f"); // follow forks
        cmd.arg("-o").arg(format!("strace-log-{}.txt", case.name()));
        cmd.arg(self_exe);
        cmd
    } else {
        std::process::Command::new(self_exe)
    };

    cmd.env_clear();
    cmd.env(crate::WORKER_ENV_NAME, "1");
    cmd.env("TEST", case.name());
    let status = cmd.status().unwrap();
    if status.success() {
        Outcome::Success
    } else {
        println!("test failed");
        Outcome::Failure
    }
}

fn get_filter(matches: &clap::ArgMatches) -> Box<dyn Fn(&'static dyn TestCase) -> bool> {
    match matches.value_of("test-filter") {
        Some(filter) => {
            let filter = filter.to_string();
            Box::new(move |test_case| test_case.name().contains(&filter))
        }
        None => Box::new(|_test_case| true),
    }
}

fn check_static() {
    let file_output = std::process::Command::new("file")
        .arg(std::env::current_exe().unwrap())
        .output()
        .expect("failed to execute fail");
    assert!(file_output.status.success());
    let file_output = String::from_utf8_lossy(&file_output.stdout);
    if file_output.contains("dynamically linked") {
        panic!(
            "minion-tests must be static executable; \
        recompile for x86_64-unknown-linux-musl"
        )
    }
}