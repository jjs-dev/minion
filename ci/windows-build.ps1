$ErrorActionPreference = "Stop"


function Build-JJSForTarget {
    param($TargetName)

    New-Item -ItemType directory -Path ./tests/$TargetName
    cargo build -p minion-tests -Zunstable-options --out-dir=./tests/$TargetName --target=$TargetName

    if ($LASTEXITCODE -ne 0) {
        throw "build failure" 
    }
}

Write-Output @'
[build]
rustflags=["--cfg", "minion_ci"]
'@ | Out-File -FilePath ./.cargo/config -Encoding 'utf8'



$env:RUSTC_BOOTSTRAP = 1
Build-JJSForTarget -TargetName x86_64-pc-windows-gnu
Build-JJSForTarget -TargetName x86_64-pc-windows-msvc