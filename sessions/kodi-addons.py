#!/usr/bin/env python3
"""switch on the add-ons the system's packages brought, before a kodi
without an add-on database starts for the first time. kodi.sh runs it:

    kodi-addons.py USERDATA

kodi enables by itself only the add-ons its own manifest lists. one from a
package of its own, such as kodi-visualization-spectrum, starts switched
off, and kodi asks about each on its first start, which nobody can answer
well on a car's screen. kodi keeps what its database already says about an
add-on, so this makes the database first, with every packaged add-on on.
a kodi that has a database is left alone. uses only the standard library."""

import datetime
import pathlib
import sqlite3
import sys

# where packages put add-ons: the data, and the binaries per architecture
PACKAGED = ("/usr/share/kodi/addons/*", "/usr/lib/*/kodi/addons/*")
# the add-on database of kodi 20 and 21, with the tables kodi would make
# (CAddonDatabase::CreateTables in xbmc/addons/AddonDatabase.cpp). a later
# kodi brings a database of an earlier version up to its own
VERSION = 33
SCHEMA = f"""
CREATE TABLE version (idVersion integer, iCompressCount integer);
INSERT INTO version VALUES ({VERSION}, 0);
CREATE TABLE addons (id INTEGER PRIMARY KEY, metadata BLOB, addonID TEXT NOT NULL, version TEXT NOT NULL,
    name TEXT NOT NULL, summary TEXT NOT NULL, news TEXT NOT NULL, description TEXT NOT NULL);
CREATE TABLE repo (id integer primary key, addonID text, checksum text, lastcheck text, version text,
    nextcheck TEXT);
CREATE TABLE addonlinkrepo (idRepo integer, idAddon integer);
CREATE TABLE update_rules (id integer primary key, addonID TEXT, updateRule INTEGER);
CREATE TABLE package (id integer primary key, addonID text, filename text, hash text);
CREATE TABLE installed (id INTEGER PRIMARY KEY, addonID TEXT UNIQUE, enabled BOOLEAN, installDate TEXT,
    lastUpdated TEXT, lastUsed TEXT, origin TEXT NOT NULL DEFAULT '', disabledReason INTEGER NOT NULL DEFAULT 0);
CREATE INDEX idxAddons ON addons(addonID);
CREATE UNIQUE INDEX ix_addonlinkrepo_1 ON addonlinkrepo (idAddon, idRepo);
CREATE UNIQUE INDEX ix_addonlinkrepo_2 ON addonlinkrepo (idRepo, idAddon);
CREATE UNIQUE INDEX idxUpdate_rules ON update_rules(addonID, updateRule);
CREATE UNIQUE INDEX idxPackage ON package(filename);
"""


def packaged_addons():
    """the ids of the add-ons installed system wide, which are their directories' names"""
    return sorted({path.name for pattern in PACKAGED for path in pathlib.Path("/").glob(pattern[1:]) if path.is_dir()})


def main():
    databases = pathlib.Path(sys.argv[1]) / "Database"
    if list(databases.glob("Addons*.db")):
        return
    databases.mkdir(parents=True, exist_ok=True)
    now = datetime.datetime.now().strftime("%Y-%m-%d %H:%M:%S")
    with sqlite3.connect(databases / f"Addons{VERSION}.db") as database:
        database.executescript(SCHEMA)
        database.executemany("INSERT INTO installed (addonID, enabled, installDate) VALUES (?, 1, ?)",
                             [(addon, now) for addon in packaged_addons()])


if __name__ == "__main__":
    main()
