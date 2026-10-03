#!/bin/bash
# provisions the test vm. the cloud kernel in the stock image omits the usb
# gadget modules, so the full kernel is installed; it takes effect on reboot.
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive

apt-get update
apt-get install -y --no-install-recommends \
    linux-image-amd64 kmod python3 python3-usb ffmpeg openssl podman \
    sway wf-recorder kodi kodi-visualization-spectrum libgl1-mesa-dri alsa-utils \
    phosh phoc gnome-settings-daemon-common libglib2.0-bin waydroid lxc \
    geoclue-2.0 geoclue-2-demo pulseaudio

# the car's location for linux applications, asked for by test_phosh.py
install -D -m 644 /incant/sessions/geoclue.conf /etc/geoclue/conf.d/90-behead.conf
# with glib 2.90 geoclue locks up while it starts for an application: one
# thread sets up libproxy as it connects to the socket, the main thread opens
# a file, and each holds the gio lock the other waits for. keep it off libproxy
mkdir -p /etc/systemd/system/geoclue.service.d
printf '[Service]\nEnvironment=GIO_USE_PROXY_RESOLVER=dummy\n' > /etc/systemd/system/geoclue.service.d/behead.conf
systemctl daemon-reload
systemctl restart geoclue

# android for test_waydroid.py: about 1 GB of images, then rendering on the
# cpu, since the vm has no gpu, and osmand~ and the f-droid client from
# f-droid, checked end to end
if [ ! -e /var/lib/waydroid/images/system.img ]; then
    waydroid init -s VANILLA
fi
python3 - <<'EOF'
import configparser
path = "/var/lib/waydroid/waydroid.cfg"
config = configparser.ConfigParser()
config.read(path)
config["properties"].update({"ro.hardware.gralloc": "default", "ro.hardware.egl": "swiftshader"})
with open(path, "w") as out:
    config.write(out)
EOF
waydroid upgrade --offline
python3 /incant/tools/fetch-fdroid.py net.osmand.plus --abi x86_64
python3 /incant/tools/fetch-fdroid.py org.fdroid.fdroid --abi x86_64
# both go into android's system image, as in the pi image, so android has
# them from its first start. nothing may use the image meanwhile
systemctl stop waydroid-container
python3 /incant/tools/preinstall-apks.py "$(sed -n 's/^images_path = //p' /var/lib/waydroid/waydroid.cfg)/system.img" \
    --abi x86_64 /root/.cache/behead/apk/fdroid/*.apk

# openauto, the independent headunit used by test_openauto.py. host
# networking, because podman's default bridge needs nftables, which the
# image lacks, and the test runs openauto on the host network anyway.
podman build --network=host -t behead-openauto /incant/tests/openauto
