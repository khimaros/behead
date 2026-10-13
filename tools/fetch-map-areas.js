#!/usr/bin/env -S gjs -m
// download areas of the map for gnome maps to show without a network, with
// gnome maps' own downloader, so that what is left is exactly what the
// application would have left itself: its list of areas, downloads.json, and
// its store of tiles, downloads.db, under XDG_DATA_HOME/gnome-maps.
//
//   XDG_DATA_HOME=DIR tools/fetch-map-areas.js NAME=WEST,SOUTH,EAST,NORTH...
//
// degrees, as in "San Francisco=-122.55,37.68,-122.33,37.84". an area already
// in the list is brought up to date, not added again. needs gnome maps 51 or
// newer, the first with downloads, and the network. in javascript, unlike the
// tools beside it, because it is gnome maps' modules that it runs.
import Gio from "gi://Gio";
import GLib from "gi://GLib";
import System from "system";

// where gnome maps is installed, which names its own modules: inside its
// flatpak, whose own gjs then runs this, or by the distribution
const PACKAGE = "gnome-maps";
const PREFIX = ["/app", "/usr"].find((prefix) => GLib.file_test(`${prefix}/share/${PACKAGE}`, GLib.FileTest.IS_DIR));
const VERSION = "0";

// its library is in lib itself, or under the machine's own directory of it
function libdir() {
    const lib = Gio.File.new_for_path(`${PREFIX}/lib`);
    if (lib.get_child(PACKAGE).query_exists(null)) return lib.get_path();
    for (const entry of lib.enumerate_children("standard::name", Gio.FileQueryInfoFlags.NONE, null)) {
        const candidate = lib.get_child(entry.get_name());
        if (candidate.get_child(PACKAGE).query_exists(null)) return candidate.get_path();
    }
    return fail(`no ${PACKAGE} under ${lib.get_path()}`);
}
const USAGE = "usage: fetch-map-areas.js NAME=WEST,SOUTH,EAST,NORTH...";

function fail(message) {
    printerr(`fetch-map-areas: ${message}`);
    System.exit(1);
}

// "NAME=WEST,SOUTH,EAST,NORTH" as a name and the edges gnome maps calls a box by
function parseArea(argument) {
    const [name, edges] = argument.split("=");
    const numbers = (edges ?? "").split(",").map(Number);
    if (!name || numbers.length !== 4 || numbers.some(Number.isNaN))
        fail(`${argument}: ${USAGE}`);
    const [left, bottom, right, top] = numbers;
    return { name, box: { left, bottom, right, top } };
}

const wanted = ARGV.map(parseArea);
if (wanted.length === 0) fail(USAGE);
if (!PREFIX) fail("gnome maps is not installed here");

imports.package.init({ name: PACKAGE, version: VERSION, prefix: PREFIX, libdir: libdir() });
imports.package.initGettext();
// its modules and data are in resource files beside each other, under names
// of their own that the package's does not lead to
const share = Gio.File.new_for_path(`${PREFIX}/share/${PACKAGE}`);
for (const entry of share.enumerate_children("standard::name", Gio.FileQueryInfoFlags.NONE, null)) {
    if (entry.get_name().endsWith(".gresource"))
        Gio.Resource.load(share.get_child(entry.get_name()).get_path())._register();
}
imports.package.initFormat();
// what gnome maps' own main.js does before anything else of it runs: its
// additions to the built-in types, and promises in place of callbacks for
// the calls its downloader awaits
await import("resource:///org/gnome/Maps/src/prototypes.js");
const { default: GnomeMaps } = await import("gi://GnomeMaps");
const { default: Soup } = await import("gi://Soup?version=3.0");
const awaited = [
    [Gio.InputStream.prototype, ["read_bytes_async"]],
    [Gio.OutputStream.prototype, ["splice_async"]],
    [Soup.Session.prototype, ["send_async", "send_and_read_async"]],
    [GnomeMaps.DownloadStore.prototype, Object.getOwnPropertyNames(GnomeMaps.DownloadStore.prototype)],
];
for (const [prototype, names] of awaited) {
    for (const name of names.filter((method) => method.endsWith("_async")))
        Gio._promisify(prototype, name, name.replace(/_async$/, "_finish"));
}
const { DownloadManager } = await import("resource:///org/gnome/Maps/src/downloads.js");
const { BoundingBox } = await import("resource:///org/gnome/Maps/src/boundingBox.js");

const manager = new DownloadManager();
manager.load();
// a build host's network may call itself metered, which pauses a download
// nobody asked for. this one was asked for
manager.ignorePause = true;
manager.connect("error", (_manager, message) => fail(message));

const loop = new GLib.MainLoop(null, false);
let started = false;
manager.connect("notify::progress", () => {
    started ||= manager.progress !== null;
    if (started && manager.progress === null) loop.quit();
});

const known = new Set([...manager.areas].map((area) => area.name));
for (const { name, box } of wanted) {
    if (manager.isTooBig(new BoundingBox(box))) fail(`${name}: larger than gnome maps downloads`);
    if (known.has(name)) continue;
    manager.addArea(name, new BoundingBox(box));
}
// areas already listed, and these with them, brought up to date
manager.updateArea(manager.areas.get_item(0));
// not run(): from a module that would hold up the promises the download is made of
await loop.runAsync();
manager.save();
print(`fetch-map-areas: ${[...manager.areas].map((area) => area.name).join(", ")} in ${GLib.get_user_data_dir()}/gnome-maps`);
