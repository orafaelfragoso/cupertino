#!/usr/bin/env bash
# shellcheck disable=SC2034

iso_name="cupertino"
iso_label="CUPERTINO_$(date --date="@${SOURCE_DATE_EPOCH:-$(date +%s)}" +%Y%m)"
iso_publisher="Cupertino <https://github.com/orafaelfragoso/cupertino>"
iso_application="Cupertino live and install medium"
iso_version="$(date --date="@${SOURCE_DATE_EPOCH:-$(date +%s)}" +%Y.%m.%d)"
install_dir="arch"
bootmodes=('uefi.systemd-boot')
pacman_conf="pacman.conf"
airootfs_image_type="squashfs"
airootfs_image_tool_options=('-comp' 'zstd' '-Xcompression-level' '15' '-b' '1M')
file_permissions=(
  ["/etc/shadow"]="0:0:400"
  ["/etc/sudoers.d/wheel"]="0:0:440"
  ["/home/live"]="1000:1000:750"
)
