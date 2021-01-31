#!/bin/bash

mkdir /dev/mnt
mount /dev/ubda1 /dev/mnt
sed -i 's/.*PermitRootLogin.*/PermitRootLogin yes/' /dev/mnt/etc/ssh/sshd_config
echo >> /dev/mnt/usr/lib/systemd/system/rc-local.service
echo '[Install]' >> /dev/mnt/usr/lib/systemd/system/rc-local.service
echo 'WantedBy=multi-user.target' >> /dev/mnt/usr/lib/systemd/system/rc-local.service
sed -i 's/After=.*/After=sshd.service/' /dev/mnt/usr/lib/systemd/system/rc-local.service
chroot /dev/mnt systemctl enable rc-local
echo '#!/bin/bash

echo root:root | chpasswd
ifconfig eth0 10.0.2.15 netmask 255.255.255.0
route add default dev eth0
echo nameserver 10.0.2.3 > /etc/resolv.conf
ifconfig -a > /dev/console
route > /dev/console
exit 0
' > /dev/mnt/etc/rc.d/rc.local
chmod +x /dev/mnt/etc/rc.d/rc.local
umount /dev/mnt
sync
poweroff -f
