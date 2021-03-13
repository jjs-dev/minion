set -euo pipefail
echo "::group::Info"
echo "Target: $CI_TARGET"
echo "Cgroup version: $CI_CGROUPS"
echo "Operating system: $CI_OS"

echo "this is hack, ignore this file" >> ./stracez-dummy 

if [[ $CI_OS == "ubuntu-20.04" ]]; then
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
fi

echo "::group::Preparing"

if [[ $CI_CGROUPS == "cgroup-v2" ]] && [ -z "${CI_VM+set}" ]; then
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
  sudo vagrant ssh --command "bash -c 'cd /vagrant && CI_OS=$CI_OS CI_CGROUPS=$CI_CGROUPS CI_TARGET=$CI_TARGET CI_VM=1 bash ci/linux.sh'" 
  echo "Host: pulling logs from VM"
  sudo vagrant ssh --command "cat /vagrant/logs.zip | base64" | base64 --decode > logs.zip
  rm stracez-dummy
  unzip logs.zip
  sleep 10
  echo "Current directory after VM finish"
  ls .
  exit 0
fi

if [[ $CI_CGROUPS == "cgroup-v2" ]]; then
  echo "::group::Some cgroup hacks"
  sudo mkdir /sys/fs/cgroup/minion
  echo "+cpu +memory +pids" | sudo tee /sys/fs/cgroup/cgroup.subtree_control
  echo "+cpu +memory +pids" | sudo tee /sys/fs/cgroup/minion/cgroup.subtree_control
fi

echo "::group::Running tests"
chmod +x ./tests/$CI_TARGET/minion-tests
sudo --preserve-env ./tests/$CI_TARGET/minion-tests --trace
echo "::group::Finalize"
echo "Current directory after running tests"
ls .
echo "Collecting logs to archive"
zip logs.zip strace*
