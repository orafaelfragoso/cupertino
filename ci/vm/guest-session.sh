#!/bin/bash
# Installs the payload's binaries and theme data and logs the account qa in on tty1, into a Cupertino session with
# three test windows. The screenshot shortcut writes every output to /home/qa/shots.
set -euo pipefail
mountpoint -q /mnt || mount -o ro /dev/vdb1 /mnt
install -m755 /mnt/cupertino-server /mnt/cupertino-testclient /mnt/cupertino-screenshot /usr/bin/
mkdir -p /usr/share/cupertino /etc/systemd/system/getty@tty1.service.d
cp -r /mnt/theme /usr/share/cupertino/theme
useradd -m -s /bin/bash qa
printf 'location = /home/qa/shots\nshow-thumbnail = false\n' >/home/qa/screenshot.conf
cat >/home/qa/.bash_profile <<'EOF'
if [ "$(tty)" = /dev/tty1 ]; then
  export CUPERTINO_SPEC_DIR=/usr/share/cupertino/theme CUPERTINO_SCREENSHOT_CONFIG=/home/qa/screenshot.conf
  client() { echo "/usr/bin/cupertino-testclient --app-id qa-$1 --title $1 --output /home/qa/$1.input"; }
  exec dbus-run-session /usr/bin/cupertino-server --udev --spawn "$(client one)" --spawn "$(client two)" \
    --spawn "$(client three)" >>/home/qa/session.log 2>&1
fi
EOF
chown qa:qa /home/qa/.bash_profile /home/qa/screenshot.conf
cat >/etc/systemd/system/getty@tty1.service.d/autologin.conf <<'EOF'
[Service]
ExecStart=
ExecStart=-/sbin/agetty --autologin qa --noclear %I linux
EOF
systemctl daemon-reload
systemctl restart getty@tty1
