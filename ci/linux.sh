set -euo pipefail
echo "::group::Info"
echo "Target: $CI_TARGET"
echo "Cgroup version: $CI_CGROUPS"
echo "Operating system: $CI_OS"

echo "this is hack, ignore this file" >> ./stracez-dummy 

if [[ $CI_OS == "ubuntu-latest" ]]; then
  if [[ $CI_CGROUPS == "cgroup-v2" ]]; then
    echo "Skipping: cgroup v2 runs in macos"
    exit 0
  fi
fi 
if [[ $CI_OS == "macos-latest" ]]; then
  if [[ $CI_CGROUPS == "cgroup-v1" ]]; then
    echo "Skipping: cgroup v1 does not need virtualization"
    exit 0
  fi
  if [[ $CI_TARGET == "x86_64-unknown-linux-gnu" ]]; then
    echo "Skipping: we will not run test on gnu targets anyway"
    exit 0
  fi
fi

echo "::group::Preparing"

if [[ $CI_CGROUPS == "cgroup-v2" ]] && [ -z "${CI_VM+set}" ]; then
  echo "::group::Preparing virtual machine"
  vagrant --version
cat > Vagrantfile <<EOF
Vagrant.configure("2") do |config|
  config.vm.box = "fedora/32-cloud-base"

  config.vm.provider "virtualbox" do |vb|
    vb.memory = "900"
  end
end
EOF
  top -l 1
  cat Vagrantfile
  sudo vagrant up
  echo "::group::Installing packages"
  sudo vagrant ssh --command "sudo dnf install -y gcc gcc-c++ strace"
  echo "::group::Installing rust"
  sudo vagrant ssh --command "curl https://sh.rustup.rs -sSf | sh -s -- -y --profile minimal --default-toolchain stable"
  echo "::group::Entering VM"
  sudo vagrant ssh --command "bash -c 'cd /vagrant && CI_OS=$CI_OS CI_CGROUPS=$CI_CGROUPS CI_TARGET=$CI_TARGET CI_VM=1 bash ci/linux.sh'"
  exit 0
fi

rustup target add $CI_TARGET  

if [[ $CI_CGROUPS == "cgroup-v2" ]]; then
  echo "::group::Some cgroup hacks"
  sudo mkdir /sys/fs/cgroup/minion
  echo "+cpu +memory +pids" | sudo tee /sys/fs/cgroup/cgroup.subtree_control | sudo tee /sys/fs/cgroup/minion/cgroup.subtree_control
fi

echo "::group::Compiling tests"
echo '[build]
rustflags=["--cfg", "minion_ci"]' > .cargo/config

export RUSTC_BOOTSTRAP=1
cargo build -p minion-tests -Zunstable-options --out-dir=./out --target=$CI_TARGET

echo "::group::Skip running if needed"
if [[ $CI_TARGET == "x86_64-unknown-linux-gnu" ]]; then
  echo "skipping: static binary required on linux"
  exit 0
fi

echo "::group::Running tests"
sudo --preserve-env ./out/minion-tests --trace
