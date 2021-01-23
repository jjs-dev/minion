set -euo pipefail
echo "::group::Info"
echo "Target: $CI_TARGET"
echo "Cgroup version: $CI_CGROUPS"

echo "this is hack, ignore this file" >> ./stracez-dummy 

echo "::group::Preparing"

if [[ $CI_CGROUPS == "cgroup-v2" ]] && [ -z "${CI_VM+set}" ]; then
  echo "::group::Preparing virtual machine"
  sudo apt-get update
  sudo apt-get install -y libopus0 libqt5core5a libqt5gui5 libqt5opengl5 libqt5printsupport5 \
      libqt5widgets5 libqt5x11extras5 libsdl1.2debian
  wget https://releases.hashicorp.com/vagrant/2.2.14/vagrant_2.2.14_x86_64.deb -q -O vagrant.deb
  wget https://download.virtualbox.org/virtualbox/5.2.44/virtualbox-5.2_5.2.44-139111~Ubuntu~bionic_amd64.deb -q -O vb.deb
  wget http://security.ubuntu.com/ubuntu/pool/main/libv/libvpx/libvpx5_1.7.0-3ubuntu0.18.04.1_amd64.deb -q -O libvpx.deb
  sudo dpkg -i libvpx.deb
  sudo dpkg -i vb.deb
  sudo dpkg -i vagrant.deb
  vagrant --version
cat > Vagrantfile <<EOF
Vagrant.configure("2") do |config|
  config.vm.box = "fedora/32-cloud-base"

  config.vm.provider "virtualbox" do |vb|
    vb.memory = "900"
    vb.customize ["modifyvm", :id, "--hwvirtex", "off"]
  end
end
EOF
  cat Vagrantfile
  sudo vagrant up
  echo "::group::Installing packages"
  sudo vagrant ssh --command "sudo dnf install -y gcc gcc-c++ strace zip"
  echo "::group::Installing rust"
  sudo vagrant ssh --command "curl https://sh.rustup.rs -sSf | sh -s -- -y --profile minimal --default-toolchain stable"
  echo "::group::Entering VM"
  sudo vagrant ssh --command "bash -c 'cd /vagrant && CI_CGROUPS=$CI_CGROUPS CI_TARGET=$CI_TARGET CI_VM=1 bash ci/linux.sh'" 
  echo "Host: pulling logs from VM"
  sudo vagrant ssh --command "cat /vagrant/logs.zip | base64" | base64 --decode > logs.zip
  rm stracez-dummy
  unzip logs.zip
  sleep 10
  echo "Current directory after VM finish"
  ls .
  exit 0
fi

rustup target add $CI_TARGET  

if [[ $CI_CGROUPS == "cgroup-v2" ]]; then
  echo "::group::Some cgroup hacks"
  sudo mkdir /sys/fs/cgroup/minion
  echo "+cpu +memory +pids" | sudo tee /sys/fs/cgroup/cgroup.subtree_control
  echo "+cpu +memory +pids" | sudo tee /sys/fs/cgroup/minion/cgroup.subtree_control
fi

echo "::group::Compiling tests"
echo '[build]
rustflags=["--cfg", "minion_ci"]' > .cargo/config

export RUSTC_BOOTSTRAP=1
cargo build -p minion-tests -Zunstable-options --out-dir=./out --target=$CI_TARGET

echo "::group::Running tests"
sudo --preserve-env ./out/minion-tests --trace
echo "::group::Finalize"
echo "Current directory after running tests"
ls .
echo "Collecting logs to archive"
zip logs.zip strace*
