#!/usr/bin/env python3
"""download an app from the f-droid repository, verified end to end: the
index entry against f-droid's signing key, the full index against the
sha-256 the entry lists, and the apk against the sha-256 the index lists.

    tools/fetch-fdroid.py [PACKAGE] [--abi ABI] [--out DIR]

defaults to osmand~ for x86_64, the vm's architecture; use arm64-v8a for
the rpi4 and phones. needs only python and the openssl command."""

import argparse
import base64
import hashlib
import io
import json
import pathlib
import re
import subprocess
import tempfile
import urllib.request
import zipfile

REPO = "https://f-droid.org/repo"
ENTRY = f"{REPO}/entry.jar"
# sha-256 of f-droid's signing certificate, as f-droid.org/docs/Release_Channels_and_Signing_Keys lists it
REPO_FINGERPRINT = "43238D512C1E5EB2D6569F4A3AFBF5523418B82E0A3ED1552770ABB9A9C9CCAB"
OSMAND = "net.osmand.plus"
DEFAULT_ABI = "x86_64"
DEFAULT_OUT = pathlib.Path.home() / ".cache/behead/apk/fdroid"
CHUNK = 1 << 20


def sha256_base64(data):
    return base64.b64encode(hashlib.sha256(data).digest()).decode()


def digest_of(text, name):
    """the SHA-256-Digest a jar signature file or manifest gives for an entry"""
    section = re.search(rf"Name: {re.escape(name)}\r?\n((?:\S.*\r?\n)*)", text)
    return re.search(r"SHA-256-Digest: (\S+)", section.group(1)).group(1) if section else None


def verified_entry():
    """the index entry, after checking f-droid signed it. a v1 jar signature
    signs the .SF file, which hashes the manifest, which hashes entry.json"""
    with urllib.request.urlopen(ENTRY) as response, zipfile.ZipFile(io.BytesIO(response.read())) as jar:
        names = jar.namelist()
        signature_file = next(name for name in names if re.fullmatch(r"META-INF/[^/]+\.SF", name))
        block = next(name for name in names if re.fullmatch(r"META-INF/[^/]+\.(RSA|DSA|EC)", name))
        sf, manifest, entry = jar.read(signature_file), jar.read("META-INF/MANIFEST.MF"), jar.read("entry.json")
        with tempfile.TemporaryDirectory() as tmp:
            tmp = pathlib.Path(tmp)
            (tmp / "sf").write_bytes(sf)
            (tmp / "block").write_bytes(jar.read(block))
            check = subprocess.run(["openssl", "cms", "-verify", "-inform", "DER", "-in", tmp / "block", "-content",
                                    tmp / "sf", "-binary", "-noverify", "-certsout", tmp / "signer.pem",
                                    "-out", "/dev/null"], capture_output=True)
            if check.returncode != 0:
                raise SystemExit("entry.jar signature does not verify")
            fingerprint = subprocess.run(["openssl", "x509", "-in", tmp / "signer.pem", "-noout", "-fingerprint",
                                          "-sha256"], capture_output=True, text=True).stdout
    if fingerprint.split("=", 1)[-1].strip().replace(":", "") != REPO_FINGERPRINT:
        raise SystemExit("entry.jar is signed by a different key")
    if not re.search(rf"SHA-256-Digest-Manifest: {re.escape(sha256_base64(manifest))}", sf.decode()):
        raise SystemExit("entry.jar manifest does not match its signature")
    if digest_of(manifest.decode(), "entry.json") != sha256_base64(entry):
        raise SystemExit("entry.json does not match the signed manifest")
    return json.loads(entry)


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


def cached(url, path, expected_hash):
    if not (path.exists() and hashlib.sha256(path.read_bytes()).hexdigest() == expected_hash):
        download(url, path, expected_hash)
    return path


def newest_for(package, abi):
    """the newest version that runs on the abi, or has no native code at all"""
    versions = [version for version in package["versions"].values()
                if abi in version["manifest"].get("nativecode", [abi])]
    if not versions:
        raise SystemExit(f"no version for {abi}")
    return max(versions, key=lambda version: version["manifest"]["versionCode"])


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("package", nargs="?", default=OSMAND)
    parser.add_argument("--abi", default=DEFAULT_ABI)
    parser.add_argument("--out", type=pathlib.Path, default=DEFAULT_OUT)
    args = parser.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)
    index = verified_entry()["index"]
    index_path = cached(f"{REPO}{index['name']}", args.out / "index-v2.json", index["sha256"])
    package = json.loads(index_path.read_bytes())["packages"].get(args.package)
    if package is None:
        raise SystemExit(f"{args.package} is not in the f-droid repository")
    version = newest_for(package, args.abi)
    file = version["file"]
    apk = cached(f"{REPO}{file['name']}", args.out / file["name"].lstrip("/"), file["sha256"])
    print(f"{args.package} {version['manifest']['versionName']} ({version['manifest']['versionCode']}) "
          f"for {args.abi}: verified {apk}")


if __name__ == "__main__":
    main()
