use clap::Clap;
use minion::{self};
use std::time::Duration;

#[derive(Debug)]
struct EnvItem {
    name: String,
    value: String,
}

fn parse_env_item(src: &str) -> Result<EnvItem, &'static str> {
    let p = src.find('=').ok_or("Env item doesn't look like KEY=VAL")?;
    Ok(EnvItem {
        name: String::from(&src[0..p]),
        value: String::from(&src[p + 1..]),
    })
}

fn parse_path_exposition_item(src: &str) -> Result<minion::SharedItem, String> {
    let parts = src.splitn(3, ':').collect::<Vec<_>>();
    if parts.len() != 3 && parts.len() != 4 {
        return Err(format!(
            "--expose item must look like `src:mode:dest` (3 parts) or `src:mode:dest:flags` (4 parts), but {} parts was provided",
            parts.len()
        ));
    }
    let amask = parts[1];
    if amask.len() != 3 {
        return Err(format!(
            "access mask must contain 3 chars (R, W, X flags), but {} provided",
            amask.len()
        ));
    }
    let kind = match amask {
        "rwx" => minion::SharedItemKind::Full,
        "r-x" => minion::SharedItemKind::Readonly,
        _ => {
            return Err(format!(
                "unknown access mask {}. rwx or r-x expected",
                amask
            ));
        }
    };
    let flags = parts
        .get(3)
        .map(|flags| flags.split(',').map(ToString::to_string).collect())
        .unwrap_or_default();
    Ok(minion::SharedItem {
        id: None,
        src: parts[0].to_string().into(),
        dest: parts[2].to_string().into(),
        kind,
        flags,
    })
}

#[derive(Clap, Debug)]
struct ExecOpt {
    /// Full name of executable file (e.g. /bin/ls)
    #[clap(name = "bin")]
    executable: String,

    /// Arguments for isolated process
    #[clap(short = 'a', long = "arg")]
    argv: Vec<String>,

    /// Environment variables (KEY=VAL) which will be passed to isolated process
    #[clap(short = 'e', long, parse(try_from_str = parse_env_item))]
    env: Vec<EnvItem>,

    /// Max peak process count (including main)
    #[clap(short = 'n', long = "max-process-count", default_value = "16")]
    num_processes: usize,

    /// Max memory available to isolated process
    #[clap(short = 'm', long, default_value = "256000000")]
    memory_limit: usize,

    /// Total CPU time in milliseconds
    #[clap(short = 't', long, default_value = "1000")]
    time_limit: u32,

    /// Print parsed argv
    #[clap(long)]
    dump_argv: bool,

    /// Print libminion parameters
    #[clap(long = "dump-generated-security-settings")]
    dump_minion_params: bool,

    /// Skip system check
    #[clap(long)]
    skip_system_check: bool,

    /// Isolation root
    #[clap(short = 'r', long = "root")]
    isolation_root: String,

    /// Exposed paths (/source/path:MASK:/dest/path), MASK is r-x for readonly access and rwx for full access
    #[clap(
        short = 'x',
        long = "expose",
        parse(try_from_str = parse_path_exposition_item)
    )]
    exposed_paths: Vec<minion::SharedItem>,

    /// Process working dir, relative to `isolation_root`
    #[clap(short = 'p', long = "pwd", default_value = "/")]
    pwd: String,
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let options: ExecOpt = Clap::parse();
    if options.dump_argv {
        println!("{:#?}", options);
    }
    if !options.skip_system_check {
        let mut res = minion::CheckResult::new();
        minion::check(&mut res);
        if res.has_errors() {
            eprintln!("{}", res);
            // TODO: option to abort
        }
    }
    let backend = match minion::erased::setup() {
        Ok(b) => b,
        Err(err) => {
            eprintln!("Backend initialization failed: {}", err);
            std::process::exit(1);
        }
    };

    let sandbox = backend
        .new_sandbox(minion::SandboxOptions {
            max_alive_process_count: options.num_processes.min(u32::max_value() as usize) as u32,
            memory_limit: options.memory_limit as u64,
            isolation_root: options.isolation_root.into(),
            shared_items: options.exposed_paths,
            cpu_time_limit: Duration::from_millis(u64::from(options.time_limit)),
            real_time_limit: Duration::from_millis(u64::from(options.time_limit * 3)),
        })
        .unwrap();

    let (stdin_fd, stdout_fd, stderr_fd);
    unsafe {
        stdin_fd = libc::dup(0) as u64;
        stdout_fd = libc::dup(1) as u64;
        stderr_fd = libc::dup(2) as u64;
    }
    let args = minion::ChildProcessOptions {
        path: options.executable.into(),
        arguments: options.argv.iter().map(|x| x.into()).collect(),
        environment: options
            .env
            .iter()
            .map(|v| format!("{}={}", &v.name, &v.value).into())
            .collect(),
        stdio: minion::StdioSpecification {
            stdin: minion::InputSpecification::handle(minion::Handle::new(stdin_fd)),
            stdout: minion::OutputSpecification::handle(minion::Handle::new(stdout_fd)),
            stderr: minion::OutputSpecification::handle(minion::Handle::new(stderr_fd)),
        },
        extra_inherit: Vec::new(),
        pwd: options.pwd.into(),
    };
    if options.dump_minion_params {
        println!("{:#?}", args);
    }
    let mut cp = backend.spawn(args, sandbox.clone()).unwrap();
    let exit_code = cp.wait_for_exit().unwrap().await.unwrap();
    println!("---> Child process exited with code {:?} <---", exit_code);
    if sandbox.check_cpu_tle().unwrap() {
        println!("Note: CPU time limit was exceeded");
    }
    if sandbox.check_real_tle().unwrap() {
        println!("Note: wall-clock time limit was exceeded");
    }
}
