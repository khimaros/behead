#!/usr/bin/env python3
"""put apps into an android system image, so android has them from its
first start with nothing to install: each apk goes to /system/app, where
android looks for the apps it ships with.

    tools/preinstall-apks.py SYSTEM_IMAGE --abi ABI APK...

the image is the ext4 one waydroid starts android from, changed in place,
without root. apks are named as f-droid names them, PACKAGE_VERSIONCODE.apk.
android does not unpack the native libraries of an app it ships with, so
the ones for ABI are stored beside the apk. an app already in the image is
replaced. needs e2fsprogs: e2fsck, resize2fs and debugfs."""

import argparse
import os
import pathlib
import re
import subprocess
import tempfile
import zipfile

# e2fsprogs is outside a user's PATH on debian
os.environ["PATH"] += os.pathsep + "/usr/sbin" + os.pathsep + "/sbin"
APPS = "/system/app"
# the directory android expects an app's libraries in, by abi
INSTRUCTION_SETS = {"arm64-v8a": "arm64", "armeabi-v7a": "arm", "x86_64": "x86_64", "x86": "x86"}
BLOCK = 4096
# room for directories and file metadata, on top of the files themselves
SPARE_BLOCKS = 4096
FILE_MODE, DIRECTORY_MODE = "0100644", "040755"
LABEL = "u:object_r:system_file:s0"
# e2fsck exits 1 when it corrected something, which is fine
E2FSCK_OK = (0, 1)


def run(*command, ok=(0,), **options):
    done = subprocess.run(command, capture_output=True, text=True, **options)
    if done.returncode not in ok:
        raise SystemExit(f"{command[0]} failed: {done.stdout}{done.stderr}")
    return done.stdout + done.stderr


def owned_by_android(path, mode):
    """debugfs commands giving a new file or directory the owner, mode and
    label android's own have"""
    return [f"sif {path} mode {mode}", f"sif {path} uid 0", f"sif {path} gid 0",
            f'ea_set {path} security.selinux "{LABEL}"']


def unpack_libraries(apk, abi, directory):
    """the apk's native libraries for one abi, as files under directory"""
    libraries = []
    with zipfile.ZipFile(apk) as archive:
        for name in archive.namelist():
            if re.fullmatch(rf"lib/{re.escape(abi)}/[^/]+\.so", name):
                target = directory / f"{apk.stem}-{pathlib.PurePosixPath(name).name}"
                target.write_bytes(archive.read(name))
                libraries.append((target, pathlib.PurePosixPath(name).name))
    return libraries


def plan(apk, abi, directory):
    """the debugfs commands that put one app in the image, and the local
    files they copy"""
    package = re.fullmatch(r"(.+?)(_\d+)?", apk.stem).group(1)
    app = f"{APPS}/{package}"
    libraries = unpack_libraries(apk, abi, directory)
    library_path = f"{app}/lib/{INSTRUCTION_SETS[abi]}"
    directories = [app] + ([f"{app}/lib", library_path] if libraries else [])
    files = [(apk, f"{app}/{package}.apk")] + [(local, f"{library_path}/{name}") for local, name in libraries]
    # an earlier copy goes first. rm and rmdir on what is not there do nothing
    commands = [f"rm {path}" for _, path in files] + [f"rmdir {path}" for path in reversed(directories)]
    for path in directories:
        commands += [f"mkdir {path}", *owned_by_android(path, DIRECTORY_MODE)]
    for local, path in files:
        commands += [f"write {local} {path}", *owned_by_android(path, FILE_MODE)]
    return commands, files


def grow(image, size):
    """make room in the image for this many more bytes"""
    blocks = int(re.search(r"Block count:\s+(\d+)", run("dumpe2fs", "-h", image)).group(1))
    run("e2fsck", "-fy", image, ok=E2FSCK_OK)
    run("resize2fs", image, str(blocks + size // BLOCK + SPARE_BLOCKS))


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("image", type=pathlib.Path)
    parser.add_argument("apks", type=pathlib.Path, nargs="+", metavar="apk")
    parser.add_argument("--abi", required=True, choices=INSTRUCTION_SETS)
    arguments = parser.parse_args()
    with tempfile.TemporaryDirectory() as tmp:
        plans = [plan(apk, arguments.abi, pathlib.Path(tmp)) for apk in arguments.apks]
        commands = [command for commands, _ in plans for command in commands]
        files = [file for _, files in plans for file in files]
        grow(arguments.image, sum(local.stat().st_size for local, _ in files))
        run("debugfs", "-w", "-f", "-", arguments.image, input="\n".join(commands) + "\n")
        # debugfs reports a failed command only in its output, so check the result
        for local, path in files:
            size = re.search(r"Size: (\d+)", run("debugfs", "-R", f"stat {path}", arguments.image))
            if not size or int(size.group(1)) != local.stat().st_size:
                raise SystemExit(f"{path} did not reach {arguments.image}")
            print(f"{arguments.image}: {path}")


if __name__ == "__main__":
    main()
