#!/usr/bin/env python3
"""put what is in the media directory into kodi's library, with nothing to
set up on its screen and no internet. kodi.sh starts it beside kodi:

    kodi-library.py MEDIA USERDATA SESSION

SESSION is the session's process, this one's parent, which it stops with.

kodi lists a film only once its folder has a content type and a scraper has
described the file. the scraper that needs no internet, metadata.local,
reads a .nfo beside each file and skips a file without one. so this writes
a .nfo, from the file's name, for every video that has none, marks the
videos folder as movies in kodi's database once kodi has made it, and asks
kodi to scan both folders. uses only the standard library."""

import json
import os
import pathlib
import re
import socket
import sqlite3
import sys
import time
from xml.sax.saxutils import escape

VIDEO_SUFFIXES = {".avi", ".m4v", ".mkv", ".mov", ".mp4", ".mpg", ".ts", ".webm", ".wmv"}
# "Title (Year)", the name kodi's wiki asks for
TITLE_AND_YEAR = re.compile(r"^(.*\S)\s*\((\d{4})\)$")
SCRAPER = "metadata.local"
# what kodi stores for "scan recursively"
SCAN_RECURSIVE = 2147483647
# kodi's json-rpc, which it offers to this host alone unless told otherwise
RPC = ("127.0.0.1", 9090)
# how long kodi may take to make its database and answer, on a slow host
WAIT = 300
POLL = 1


def description(video):
    """a .nfo for a video, with the title and year its file name gives"""
    named = TITLE_AND_YEAR.match(video.stem)
    title, year = named.groups() if named else (video.stem, None)
    year = f"    <year>{year}</year>\n" if year else ""
    return f"<movie>\n    <title>{escape(title)}</title>\n{year}</movie>\n"


def describe(videos):
    for video in sorted(videos.rglob("*")):
        nfo = video.with_suffix(".nfo")
        if video.suffix.lower() in VIDEO_SUFFIXES and not nfo.exists():
            nfo.write_text(description(video))


def video_database(userdata):
    """kodi's newest video database, once it has the table of folders"""
    numbered = {int(re.search(r"\d+", path.stem).group()): path for path in userdata.glob("Database/MyVideos*.db")}
    if not numbered:
        return None
    database = sqlite3.connect(numbered[max(numbered)], timeout=WAIT)
    return database if database.execute("select 1 from sqlite_master where name = 'path'").fetchone() else None


def mark_as_movies(database, folder):
    """what kodi's "set content" does for a folder, unless it has been done"""
    if not database.execute("select 1 from path where strPath = ?", (folder,)).fetchone():
        database.execute("insert into path (strPath, strContent, strScraper, scanRecursive, useFolderNames, "
                         "strSettings, noUpdate, exclude) values (?, 'movies', ?, ?, 0, '', 0, 0)",
                         (folder, SCRAPER, SCAN_RECURSIVE))
        database.commit()


def call(method, **params):
    """one json-rpc call. kodi sends notifications on the same connection"""
    with socket.create_connection(RPC, timeout=WAIT) as kodi:
        kodi.sendall(json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).encode())
        received = ""
        while True:
            received += kodi.recv(4096).decode()
            try:
                message, end = json.JSONDecoder().raw_decode(received)
            except ValueError:
                continue
            if message.get("id") == 1:
                return message
            received = received[end:].lstrip()


def wait_for(attempt, session):
    """the first thing an attempt returns, tried while the session runs"""
    deadline = time.monotonic() + WAIT
    while os.getppid() == session and time.monotonic() < deadline:
        try:
            if (result := attempt()) is not None:
                return result
        except (OSError, sqlite3.Error):
            pass
        time.sleep(POLL)
    sys.exit("kodi-library: the session ended, or kodi took too long")


def main(media, userdata, session):
    videos, music = media / "videos", media / "music"
    describe(videos)
    mark_as_movies(wait_for(lambda: video_database(userdata), session), f"{videos}/")
    wait_for(lambda: call("VideoLibrary.Scan"), session)
    call("AudioLibrary.Scan", directory=f"{music}/")


if __name__ == "__main__":
    main(pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2]), int(sys.argv[3]))
