#!/usr/bin/env python3
"""download the android images waydroid runs, the way `waydroid init` does:
the newest lineageos system and vendor image on its update channel, each
checked against the sha-256 the channel lists. the channel is trusted over
https alone, as waydroid itself trusts it.

    tools/fetch-waydroid.py [--arch ARCH] [--out DIR] [--update]

leaves system.img and vendor.img in DIR. put there as
/usr/share/waydroid-extra/images on a device, `waydroid init` uses them
instead of downloading. images already in DIR are kept, without asking the
channel, unless --update is given. defaults to arm64, for the rpi4 and
phones; the vm is x86_64."""

import argparse
import hashlib
import json
import pathlib
import shutil
import tempfile
import urllib.request
import zipfile

CHANNEL = "https://ota.waydro.id"
# the android flavour without google apps, and the vendor image for mainline kernels
CHANNELS = {"system": "system/lineage/waydroid_{arch}/VANILLA.json", "vendor": "vendor/waydroid_{arch}/MAINLINE.json"}
DEFAULT_ARCH = "arm64"
DEFAULT_CACHE = pathlib.Path.home() / ".cache/behead/waydroid"
CHUNK = 1 << 20


def newest_build(kind, arch):
    with urllib.request.urlopen(f"{CHANNEL}/{CHANNELS[kind].format(arch=arch)}") as response:
        return max(json.load(response)["response"], key=lambda build: build["datetime"])


def download(build, archive):
    """stream a build's zip into an open file, and refuse it unless the hash matches"""
    digest = hashlib.sha256()
    with urllib.request.urlopen(build["url"]) as response:
        while chunk := response.read(CHUNK):
            digest.update(chunk)
            archive.write(chunk)
    if digest.hexdigest() != build["id"]:
        raise SystemExit(f"{build['filename']}: sha-256 mismatch")


def fetch(kind, arch, image):
    """the newest image of a kind, moved into place only once it is whole"""
    build, partial = newest_build(kind, arch), image.with_suffix(".part")
    with tempfile.TemporaryFile(dir=image.parent) as archive:
        download(build, archive)
        with zipfile.ZipFile(archive) as unpacked, unpacked.open(image.name) as source, open(partial, "wb") as out:
            shutil.copyfileobj(source, out, CHUNK)
    partial.rename(image)
    return build["filename"]


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--arch", default=DEFAULT_ARCH)
    parser.add_argument("--out", type=pathlib.Path)
    parser.add_argument("--update", action="store_true")
    args = parser.parse_args()
    out = args.out or DEFAULT_CACHE / args.arch
    out.mkdir(parents=True, exist_ok=True)
    for kind in CHANNELS:
        image = out / f"{kind}.img"
        if image.exists() and not args.update:
            print(f"{image}: kept")
        else:
            print(f"{image}: verified, from {fetch(kind, args.arch, image)}", flush=True)


if __name__ == "__main__":
    main()
