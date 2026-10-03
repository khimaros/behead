#!/usr/bin/env python3
"""download an app from the grapheneos app repository, verified end to end:
the repository metadata against grapheneos's signing key, and every apk
against the sha-256 that the signed metadata lists for it.

    tools/fetch-apk.py [PACKAGE] [--out DIR]

defaults to android auto. needs only python and the openssl command."""

import argparse
import base64
import hashlib
import json
import pathlib
import re
import shutil
import subprocess
import tempfile
import urllib.request

REPO = "https://apps.grapheneos.org"
METADATA = f"{REPO}/metadata.1.0.sjson"
# signify ed25519 key, from REPO_PUBLIC_KEY in GrapheneOS/AppStore app/build.gradle.kts
REPO_KEY = "RWQtZwEu1br1lMh911L3yPOs97cQb9LOks/ALBbqGl21ul695ocWR/ir"
ANDROID_AUTO = "com.google.android.projection.gearhead"
DEFAULT_OUT = pathlib.Path.home() / ".cache/behead/apk"
# signify blobs: 2 byte algorithm, 8 byte key number, then key or signature
SIGNIFY_HEADER = 10
ED25519_SPKI_PREFIX = bytes.fromhex("302a300506032b6570032100")
CHUNK = 1 << 20


def fetch(url):
    with urllib.request.urlopen(url) as response:
        return response.read()


def verified_metadata():
    """the repository metadata, after checking grapheneos signed it"""
    data = fetch(METADATA)
    end = data.rindex(b"}") + 1
    body, signature = data[:end], base64.b64decode(data[end:].strip())
    key = base64.b64decode(REPO_KEY)
    if signature[2:SIGNIFY_HEADER] != key[2:SIGNIFY_HEADER]:
        raise SystemExit("metadata is signed by a different key")
    with tempfile.TemporaryDirectory() as tmp:
        tmp = pathlib.Path(tmp)
        spki = base64.b64encode(ED25519_SPKI_PREFIX + key[SIGNIFY_HEADER:]).decode()
        (tmp / "key.pem").write_text(f"-----BEGIN PUBLIC KEY-----\n{spki}\n-----END PUBLIC KEY-----\n")
        (tmp / "body").write_bytes(body)
        (tmp / "sig").write_bytes(signature[SIGNIFY_HEADER:])
        check = subprocess.run(["openssl", "pkeyutl", "-verify", "-pubin", "-inkey", tmp / "key.pem", "-rawin",
                                "-in", tmp / "body", "-sigfile", tmp / "sig"], capture_output=True)
    if check.returncode != 0:
        raise SystemExit("metadata signature does not verify")
    return json.loads(body)


def download(url, path, expected_hash):
    """stream to a temporary name and keep it only if the hash matches"""
    digest, partial = hashlib.sha256(), path.with_suffix(".part")
    with urllib.request.urlopen(url) as response, open(partial, "wb") as out:
        while chunk := response.read(CHUNK):
            digest.update(chunk)
            out.write(chunk)
    if digest.hexdigest() != expected_hash:
        partial.unlink()
        raise SystemExit(f"{path.name}: sha-256 mismatch")
    partial.rename(path)


def check_signer(apk, signatures):
    """a second, independent check when the android sdk's apksigner is on the
    path: the apk must be signed by a certificate the metadata names"""
    apksigner = shutil.which("apksigner")
    if not apksigner:
        return print("apksigner not found; relying on the metadata hashes alone")
    out = subprocess.run([apksigner, "verify", "--print-certs", apk], capture_output=True, text=True)
    digests = re.findall(r"Signer #\d+ certificate SHA-256 digest: (\w+)", out.stdout)
    if out.returncode != 0 or not set(digests) & set(signatures):
        raise SystemExit(f"{apk.name}: not signed by {', '.join(signatures)}")


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("package", nargs="?", default=ANDROID_AUTO)
    parser.add_argument("--out", type=pathlib.Path, default=DEFAULT_OUT)
    args = parser.parse_args()
    package = verified_metadata()["packages"][args.package]
    version, variant = max(package["variants"].items(), key=lambda item: item[1]["versionCode"])
    out = args.out / args.package / version
    out.mkdir(parents=True, exist_ok=True)
    for apk, expected in zip(variant["apks"], variant["apkHashes"]):
        path = out / apk
        if path.exists() and hashlib.sha256(path.read_bytes()).hexdigest() == expected:
            continue
        download(f"{REPO}/packages/{args.package}/{version}/{apk}", path, expected)
    check_signer(out / variant["apks"][0], package["signatures"])
    (out / "variant.json").write_text(json.dumps({**variant, "signatures": package["signatures"]}, indent=1))
    print(f"{variant['label']} {variant['versionName']} ({version}): {len(variant['apks'])} apks verified in {out}")


if __name__ == "__main__":
    main()
