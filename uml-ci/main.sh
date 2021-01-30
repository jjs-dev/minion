#!/bin/bash

set -e

wget -q https://download.fedoraproject.org/pub/fedora/linux/releases/33/Cloud/x86_64/images/Fedora-Cloud-Base-33-1.2.x86_64.raw.xz -O img.xz
unxz img.xz
wget -q https://cdn.kernel.org/pub/linux/kernel/v5.x/linux-5.4.93.tar.xz
tar -xJf linux-5.4.93.tar.xz
(
cd linux-5.4.93
export ARCH=um
make defconfig
make -j3
)
linux-5.4.93/linux mem=4096M ubda=img rootfstype=hostfs init="$PWD"/uml-setup.sh
linux-5.4.93/linux mem=4096M ubda=img root=/dev/ubda1 rootfstype=ext4 hostfs=.. eth0=slirp,,./uml-slirp.sh &
./uml-ssh.sh
