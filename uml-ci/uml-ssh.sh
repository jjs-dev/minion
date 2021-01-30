#!/bin/bash

# wait for vm startup
while ! echo incompatible | nc 127.0.0.1 2224; do sleep 1; done

set -e

sleep 10
sshpass -p root ssh -o 'StrictHostKeyChecking no' -p 2224 root@127.0.0.1 'sed -i '"'"'s/.*wheel.*/#\0/'"'"' /etc/sudoers; sed -i '"'"'s/#*\(.*wheel.*NOPASSWD.*\)/\1/'"'"' /etc/sudoers; adduser -g wheel --uid '"$(id -u)"' user; echo user:user | chpasswd'
sshpass -p user ssh -o 'StrictHostKeyChecking no' -p 2224 user@127.0.0.1 'sudo mount -t hostfs hostfs ~; cd; exec uml-ci/payload.sh'
