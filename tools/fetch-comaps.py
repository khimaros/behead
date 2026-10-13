#!/usr/bin/env python3
"""download regions of the map for comaps, which shows, searches and routes
on them with no network. the regions are those of the comaps a device will
run: its own list names each with a size and a sha-1, which every download
is checked against, and the version of the data they must be of.

    tools/fetch-comaps.py --flatpak INSTALLATION --out DIR REGION...

INSTALLATION is the flatpak installation with comaps in it, as
tools/fetch-flatpak.sh leaves one. the regions go to DIR/VERSION, named as
comaps names them, such as `US_Tennessee_West`; --list prints the names.
sessions/phosh.sh copies them to where comaps keeps its own. downloads are
kept in a cache, so a region is fetched once for each version."""

import argparse
import base64
import hashlib
import json
import pathlib
import shutil
import sys
import urllib.error
import urllib.parse
import urllib.request

APPLICATION = "app.comaps.comaps"
# where a flatpak application's files are, under its installation, and the
# list of regions among them
COUNTRIES = f"app/{APPLICATION}/*/*/*/files/share/comaps/data/countries.txt"
# servers that have the regions under maps/VERSION, tried in turn. comaps asks
# a server of its own for this list; on 2026-10-12 only the first still had
# the version the flatpak was built with
SERVERS = ("https://mapgen-fi-1.comaps.app", "https://cdn-fi-1.comaps.app", "https://cdn-us-1.comaps.app")
DEFAULT_CACHE = pathlib.Path.home() / ".cache/behead/comaps"
EXTENSION = ".mwm"
CHUNK = 1 << 20


def regions(countries):
    """every region of comaps' list by name, as (bytes, sha-1 in base64)"""
    def leaves(node):
        for child in node.get("g", ()):
            yield from leaves(child)
        if "g" not in node:
            yield node["id"], (node["s"], node["sha1_base64"])
    return dict(leaves(countries))


def checked(path, size, sha1):
    """whether a file is the region the list describes"""
    if not path.exists() or path.stat().st_size != size:
        return False
    digest = hashlib.sha1()
    with path.open("rb") as file:
        while chunk := file.read(CHUNK):
            digest.update(chunk)
    return base64.b64encode(digest.digest()).decode() == sha1


def download(name, version, target):
    """a region from the first server that has it, into `target`"""
    for server in SERVERS:
        url = f"{server}/maps/{version}/{urllib.parse.quote(name)}{EXTENSION}"
        try:
            with urllib.request.urlopen(url) as response, target.open("wb") as file:
                shutil.copyfileobj(response, file, CHUNK)
            return url
        except urllib.error.URLError:
            continue
    sys.exit(f"{name}: no server has it for version {version}")


def fetch(name, described, version, cache):
    """the region's file in the cache, downloaded unless it is there and whole"""
    path = cache / str(version) / f"{name}{EXTENSION}"
    if checked(path, *described):
        return path
    path.parent.mkdir(parents=True, exist_ok=True)
    partial = path.with_suffix(".part")
    url = download(name, version, partial)
    if not checked(partial, *described):
        partial.unlink()
        sys.exit(f"{name}: what {url} sent is not the region comaps lists")
    partial.replace(path)
    return path


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--flatpak", required=True, type=pathlib.Path, help="the flatpak installation with comaps in it")
    parser.add_argument("--out", type=pathlib.Path, help="where the regions go, in a directory named for their version")
    parser.add_argument("--cache", type=pathlib.Path, default=DEFAULT_CACHE)
    parser.add_argument("--list", action="store_true", help="print the names of the regions and leave")
    parser.add_argument("region", nargs="*")
    args = parser.parse_args()
    found = sorted(args.flatpak.glob(COUNTRIES))
    if not found:
        sys.exit(f"no {APPLICATION} in {args.flatpak}")
    countries = json.loads(found[-1].read_text())
    version, known = countries["v"], regions(countries)
    if args.list:
        print("\n".join(sorted(known)))
        return
    unknown = [name for name in args.region if name not in known]
    if unknown or not args.out:
        sys.exit(f"not regions of comaps {version}: {unknown}" if unknown else "--out DIR is needed")
    target = args.out / str(version)
    target.mkdir(parents=True, exist_ok=True)
    for name in args.region:
        shutil.copy(fetch(name, known[name], version, args.cache), target)
        print(f"{name}: {known[name][0] >> 20} MB, verified, in {target}")


if __name__ == "__main__":
    main()
