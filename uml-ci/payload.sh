#!/bin/bash

sudo dnf install -y gcc gcc-c++ strace zip
curl https://sh.rustup.rs -sSf | sh -s -- -y --profile minimal --default-toolchain stable
export CI_OS=fedora
export CI_CGROUPS=cgroup-v2
export CI_TARGET=x86_64-unknown-linux-musl
export CI_VM=1
bash ci/linux.sh
