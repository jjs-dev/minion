use std::sync::atomic::{AtomicBool, Ordering};

mod seccomp_policies;

static DID_WRITES: AtomicBool = AtomicBool::new(false);

fn main() -> anyhow::Result<()> {
    seccomp_policies::gen()?;

    if DID_WRITES.load(Ordering::SeqCst) {
        anyhow::bail!("Files were updated, exiting with 1");
    }
    Ok(())
}
fn put_file(path: &str, contents: &str) -> anyhow::Result<()> {
    let path = std::env::current_dir()?.join(path);
    if let Ok(data) = std::fs::read_to_string(&path) {
        if data == contents {
            return Ok(());
        }
    }
    println!("Updating {}", path.display());
    DID_WRITES.store(true, Ordering::SeqCst);

    std::fs::write(path, contents)?;
    Ok(())
}
