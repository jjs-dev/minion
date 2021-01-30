set -euo pipefail
echo "::group::Info"
echo "Target: $CI_TARGET"
echo "Cgroup version: $CI_CGROUPS"
echo "Operating system: $CI_OS"

echo "this is hack, ignore this file" >> ./stracez-dummy 

echo "::group::Preparing"

if [[ $CI_CGROUPS == "cgroup-v2" ]] && [ -z "${CI_VM+set}" ]; then
  echo "::group::Preparing virtual machine"
  sudo apt install sshpass slirp
  ( cd uml-ci; ./main.sh; )
  sudo sshpass -p user ssh -p 2224 user@127.0.0.1 "cat /vagrant/logs.zip | base64" | base64 --decode > logs.zip
  sudo sshpass -p user ssh -p 2224 user@127.0.0.1 sudo poweroff
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
