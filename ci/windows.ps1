$ErrorActionPreference = "Stop"

Write-Output "::group::Info"
Write-Output "Target: $env:CI_TARGET"
Write-Output "::group::Running tests"

$env:RUST_BACKTRACE = "full"
"./tests/$env:CI_TARGET/minion-tests.exe" --trace
if ($LASTEXITCODE -ne 0) {
    throw "tests failed"
}