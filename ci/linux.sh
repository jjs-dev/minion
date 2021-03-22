set -euxo pipefail
echo "::group::Info"
echo "Operating system: $CI_OS"

echo "this is hack, ignore this file" >> ./stracez-dummy 



echo "::group::Preparing"

if [[ $CI_OS == "macos-latest" ]] && [ -z "${CI_VM+set}" ]; then
  echo "::group::Preparing virtual machine"
  vagrant --version
cat > Vagrantfile <<EOF
Vagrant.configure("2") do |config|
  config.vm.box = "fedora/32-cloud-base"

  config.vm.provider "virtualbox" do |vb|
    vb.memory = "700"
  end
end
EOF
  top -l 1
  cat Vagrantfile
  sudo vagrant up || sudo vagrant up || sudo vagrant up
  echo "::group::Installing packages"
  sudo vagrant ssh --command "sudo dnf install -y strace zip"
  echo "::group::Entering VM"
  sudo vagrant ssh --command "bash -c 'cd /vagrant && CI_OS=$CI_OS CI_VM=1 bash ci/linux.sh'" 
  echo "Host: pulling logs from VM"
  sudo vagrant ssh --command "cat /vagrant/logs.zip | base64" | base64 --decode > logs.zip
  rm stracez-dummy
  unzip logs.zip
  sleep 10
  echo "Current directory after VM finish"
  ls .
  exit 0
fi

if [[ $CI_OS == "macos-latest" ]]; then
  echo "::group::Some cgroup hacks"
  sudo mkdir /sys/fs/cgroup/minion
  echo "+cpu +memory +pids" | sudo tee /sys/fs/cgroup/cgroup.subtree_control
  echo "+cpu +memory +pids" | sudo tee /sys/fs/cgroup/minion/cgroup.subtree_control
fi

echo "::group::Running tests"
TEST_BIN=./tests/x86_64-unknown-linux-musl/minion-tests
chmod +x $TEST_BIN
if [[ $CI_OS == "ubuntu-20.04" ]]; then
  # TODO: actually run rootless without root
  PROFILES="cgroup-v1 prlimit"
fi 
if [[ $CI_OS == "macos-latest" ]]; then
  PROFILES="cgroup-v2"
fi

sudo --preserve-env $TEST_BIN --trace --profile $PROFILES
echo "::group::Finalize"
echo "Current directory after running tests"
ls .
echo "Collecting logs to archive"
zip logs.zip strace*
