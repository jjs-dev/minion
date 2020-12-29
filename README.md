# Minion
Minion is library implementing highly isolated sandboxes on top of low-level OS APIs.
Features:
 - Security isolation: sandboxed process runs with reduced privileges, and these privileges can be configured by user.
 - Resource restrictions: CPU time and RAM usage can be limited, ensuring that sandbox can't starve other processes.

Additionally, CLI tool is provided for simple use cases.

Build requirements:
- Latest stable Rust (minion may compile successfully on older toolchains, but this is not guarenteed).
# Using minion library
## From Rust
Add following to `Cargo.toml`:
```toml
# under [dependencies]:
minion = { git = "https://github.com/jjs-dev/minion" }
```
## From C
```
# otherwise nightly rust is required
rm -rf .cargo
cargo build --package minion-ffi --release
```
Following files should appear somewhere in `target`:
 - `minion-ffi-prepend.h` & `minion-ffi.h` - header files
 - `libminion_ffi.a` - static library
 - `libminion_ffi.so` - shared library
# Installing CLI
## Docker image
```sh
docker pull ghcr.io/jjs-dev/minion:latest
```
(You can use minion-cli directly from image, or you can unpack image).
## From source
```
# otherwise nightly rust is required
rm -rf .cargo
cargo build --package minion-cli --release
```