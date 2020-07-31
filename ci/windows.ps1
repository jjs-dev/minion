$ErrorActionPreference = "Stop"

Write-Output "::group::Info"
Write-Output "Target: $env:CI_TARGET"
Write-Output "::group::Preparing"
rustup target add $env:CI_TARGET
Write-Output @'
[build]
rustflags=["--cfg", "minion_ci"]
'@ | Out-File -FilePath ./.cargo/config -Encoding 'utf8'

Write-Output "::group::Compiling tests"
$env:RUSTC_BOOTSTRAP = 1
cargo build -p minion-tests -Zunstable-options --out-dir=./out --target=$env:CI_TARGET

if ($LASTEXITCODE -ne 0) {
    throw "build failure" 
}

Write-Output "::group::Running tests"
$env:RUST_BACKTRACE = 1
./out/minion-tests.exe
if ($LASTEXITCODE -ne 0) {
    throw "tests failed"
}