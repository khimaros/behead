#!/usr/bin/env python3
"""make a map for the launcher: the roads and the water of an area, from
openstreetmap, in a file the launcher draws from with no network.

    tools/fetch-map.py OUT.bmap WEST,SOUTH,EAST,NORTH
    tools/fetch-map.py OUT.bmap world

the edges are in degrees. `world` in their place makes a map of the whole
world, its land and its lakes and no more, from natural earth: the launcher
draws it under the others, and alone wherever it has no other. the file
goes on the card or the disk the
launcher's music is on, under maps/. it holds the area three times over, in
squares of three sizes: the main roads alone for a view of the whole, down
to every street and its name for a view of a few of them, so the launcher
reads the squares on the screen and no more.

the layout, every number with its low byte first:

    "bmap", version (1 byte), levels (1 byte), 2 unused
    for each level, the coarsest first:
        shift (1 byte): a square is 2^shift units wide, of the 2^32 the
            world is around; 3 unused
        the column and the row of its first square (4 bytes each)
        columns, rows (2 bytes each)
    for each level, for each square, a row at a time:
        where its features start in the file, and their length (4 bytes each)
    the features of a square, each a kind (1 byte), then
        a road, a river or water: a count (2 bytes), and so many points,
            x and y (2 bytes each) across the square from 0 to 65535
        a name: its point, its length (1 byte), and its letters
    lines are cut at a square's edges, so none leaves its square.

map data is openstreetmap's, under its open database licence."""

import json
import math
import struct
import sys
import urllib.parse
import urllib.request

MAGIC, VERSION = b"bmap", 1
# the world around, in the units places are kept in
WORLD = 1 << 32
# a square's own coordinates
LOCAL = 1 << 16
MOTORWAY, MAIN, MIDDLE, MINOR, WATER, RIVER, NAME, LAND = 1, 2, 3, 4, 5, 6, 7, 8
# the latitude a web map's square of the world ends at, north and south
FAR_NORTH = 85.0511
# what stands for the edges of an area to ask for the whole world
WHOLE = "world"
# the world's levels: a quarter and a 32nd of it across. its land and its
# lakes are natural earth's, at one to 50 million, which is in the public
# domain
WORLD_LEVELS = ((30, {LAND, WATER}), (27, {LAND, WATER}))
NATURAL_EARTH = "https://raw.githubusercontent.com/nvkelso/natural-earth-vector/master/geojson/{}.geojson"
WORLD_LAYERS = (("ne_50m_land", LAND), ("ne_50m_lakes", WATER))
ROADS = {"motorway": MOTORWAY, "motorway_link": MOTORWAY, "trunk": MAIN, "trunk_link": MAIN, "primary": MAIN,
         "primary_link": MAIN, "secondary": MIDDLE, "tertiary": MIDDLE, "unclassified": MINOR,
         "residential": MINOR, "living_street": MINOR}
# the levels, coarsest first: how wide a square is, as the power of two of
# its width in units, and the kinds it has. the widths are those of the
# tiles of a web map at zoom 9, 12 and 14
LEVELS = ((23, {MOTORWAY, MAIN, WATER, RIVER}), (20, {MOTORWAY, MAIN, MIDDLE, WATER, RIVER}),
          (18, {MOTORWAY, MAIN, MIDDLE, MINOR, WATER, RIVER, NAME}))
# a point this small a part of its square from the one before adds nothing
# anyone could see, and water smaller than this part of a square is left out
FINEST, SMALLEST = 1 / 1024, 1 / 64
NAME_LENGTH = 40
OVERPASS = "https://overpass-api.de/api/interpreter"
QUERY = """[out:json][timeout:600];
(way["highway"~"^({roads})$"]({box});way["natural"="water"]({box});way["waterway"~"^(river|canal)$"]({box}););
out geom;"""


def world(latitude, longitude):
    """a place in the map's units, as a web map lays the world out: west to
    east across, the far north at the top"""
    latitude = max(-FAR_NORTH, min(FAR_NORTH, latitude))
    x = (longitude + 180) / 360 * WORLD
    y = (1 - math.asinh(math.tan(math.radians(latitude))) / math.pi) / 2 * WORLD
    return min(x, WORLD - 1), min(y, WORLD - 1)


def classify(tags):
    """the kind a way is drawn as, or None, and whether it is an area"""
    if tags.get("highway") in ROADS:
        return ROADS[tags["highway"]], False
    if tags.get("natural") == "water":
        return WATER, True
    if tags.get("waterway") in ("river", "canal"):
        return RIVER, False
    return None, False


def cut(start, end, low, high, axis):
    """the part of a stretch between two bounds along one axis, or None"""
    (a, b), span = (start[axis], end[axis]), end[axis] - start[axis]
    if max(a, b) < low or min(a, b) > high:
        return None

    def at(bound):
        part = (bound - a) / span
        return tuple(bound if i == axis else start[i] + part * (end[i] - start[i]) for i in (0, 1))
    start = at(low) if a < low else at(high) if a > high else start
    end = at(low) if b < low else at(high) if b > high else end
    return start, end


def clip_line(points, box):
    """the runs of a line inside a box"""
    (left, top, right, bottom), runs = box, []
    for start, end in zip(points, points[1:]):
        across = cut(start, end, left, right, 0)
        inside = across and cut(*across, top, bottom, 1)
        if not inside:
            continue
        if runs and runs[-1][-1] == inside[0]:
            runs[-1].append(inside[1])
        else:
            runs.append(list(inside))
    return runs


def clip_area(points, box):
    """the part of an area inside a box, as one outline, or nothing"""
    left, top, right, bottom = box
    for axis, bound, keep in ((0, left, 1), (0, right, -1), (1, top, 1), (1, bottom, -1)):
        kept = []
        for start, end in zip(points, points[1:] + points[:1]):
            inside = [(point[axis] - bound) * keep >= 0 for point in (start, end)]
            if inside[0]:
                kept.append(start)
            if inside[0] != inside[1]:
                part = (bound - start[axis]) / (end[axis] - start[axis])
                kept.append(tuple(start[i] + part * (end[i] - start[i]) for i in (0, 1)))
        points = kept
    return [points] if len(points) >= 3 else []


def thin(points, least):
    """a line without the points too near the one before to be seen"""
    kept = points[:1]
    for point in points[1:-1]:
        if max(abs(point[0] - kept[-1][0]), abs(point[1] - kept[-1][1])) >= least:
            kept.append(point)
    return kept + points[-1:] if len(points) > 1 else kept


def squares(points, size):
    """the columns and rows of the squares a feature reaches into"""
    xs, ys = [point[0] for point in points], [point[1] for point in points]
    return (range(int(min(xs) // size), int(max(xs) // size) + 1),
            range(int(min(ys) // size), int(max(ys) // size) + 1))


def local(point, column, row, size):
    return tuple(min(LOCAL - 1, max(0, int((value - origin * size) * LOCAL / size)))
                 for value, origin in zip(point, (column, row)))


def shape(kind, points, column, row, size):
    places = [local(point, column, row, size) for point in points]
    return struct.pack("<BH", kind, len(places)) + b"".join(struct.pack("<HH", *place) for place in places)


def name(text, point, column, row, size):
    letters = text.encode("ascii", "replace")[:NAME_LENGTH]
    return struct.pack("<BHHB", NAME, *local(point, column, row, size), len(letters)) + letters


def features(elements):
    """the ways worth drawing: a kind, whether an area, a name, and points"""
    for element in elements:
        kind, area = classify(element.get("tags", {}))
        points = [world(point["lat"], point["lon"]) for point in element.get("geometry", [])]
        if kind and len(points) >= 2:
            yield kind, area and points[0] == points[-1], element["tags"].get("name"), points


def level(shift, kinds, drawn):
    """the squares of one level, by column and row, each its features' bytes"""
    size, cells, named = 1 << shift, {}, set()
    for kind, area, text, points in drawn:
        if kind not in kinds:
            continue
        points = thin(points, size * FINEST)
        xs, ys = [point[0] for point in points], [point[1] for point in points]
        if area and max(max(xs) - min(xs), max(ys) - min(ys)) < size * SMALLEST:
            continue
        columns, rows = squares(points, size)
        for column in columns:
            for row in rows:
                box = (column * size, row * size, (column + 1) * size, (row + 1) * size)
                parts = clip_area(points, box) if area else clip_line(points, box)
                cells.setdefault((column, row), []).extend(shape(kind, part, column, row, size) for part in parts)
        middle = points[len(points) // 2]
        place = (int(middle[0] // size), int(middle[1] // size))
        if NAME in kinds and text and not area and kind != RIVER and (place, text) not in named:
            named.add((place, text))
            cells.setdefault(place, []).append(name(text, middle, *place, size))
    return cells


def pack(levels, box, drawn):
    """a map file: what is drawn, in the squares of each level that a box of
    the world, its left, top, right and bottom, reaches into"""
    (left, top, right, bottom), heads, tables, blobs = box, [], [], []
    for shift, kinds in levels:
        size, cells = 1 << shift, level(shift, kinds, drawn)
        columns = range(int(left // size), int(right // size) + 1)
        rows = range(int(top // size), int(bottom // size) + 1)
        heads.append(struct.pack("<B3xIIHH", shift, columns[0], rows[0], len(columns), len(rows)))
        tables.append([b"".join(cells.get((column, row), [])) for row in rows for column in columns])
    start = 8 + sum(map(len, heads)) + 8 * sum(map(len, tables))
    index = []
    for blob in (blob for table in tables for blob in table):
        index.append(struct.pack("<II", start + sum(map(len, blobs)), len(blob)))
        blobs.append(blob)
    return b"".join([MAGIC, struct.pack("<BB2x", VERSION, len(levels)), *heads, *index, *blobs])


def build(area, elements):
    """the map file for an area, west, south, east and north, of what
    openstreetmap gave for it"""
    west, south, east, north = area
    return pack(LEVELS, (*world(north, west), *world(south, east)), list(features(elements)))


def outlines(layer, kind):
    """the outer edge of every area in a geojson layer, as something to
    draw. islands in a lake and lakes on an island are left as they fall"""
    for feature in layer["features"]:
        geometry = feature["geometry"]
        areas = geometry["coordinates"] if geometry["type"] == "MultiPolygon" else [geometry["coordinates"]]
        for area in areas:
            yield kind, True, None, [world(latitude, longitude) for longitude, latitude, *_ in area[0][:-1]]


def build_world(layers):
    """the map file of the whole world, from geojson layers and the kind
    each is drawn as: for under the maps of places, and where there is none"""
    drawn = [shape for layer, kind in layers for shape in outlines(layer, kind)]
    return pack(WORLD_LEVELS, (0, 0, WORLD - 1, WORLD - 1), drawn)


def fetch_world():
    layers = []
    for name, kind in WORLD_LAYERS:
        with urllib.request.urlopen(NATURAL_EARTH.format(name)) as answer:
            layers.append((json.load(answer), kind))
    return layers


def fetch(area):
    west, south, east, north = area
    query = QUERY.format(roads="|".join(ROADS), box=f"{south},{west},{north},{east}")
    request = urllib.request.Request(OVERPASS, urllib.parse.urlencode({"data": query}).encode(),
                                     {"User-Agent": "behead fetch-map"})
    with urllib.request.urlopen(request) as answer:
        return json.load(answer)["elements"]


def main():
    try:
        out, edges = sys.argv[1:]
        area = None if edges == WHOLE else tuple(float(edge) for edge in edges.split(","))
        assert area is None or len(area) == 4 and area[0] < area[2] and area[1] < area[3]
    except (ValueError, AssertionError):
        sys.exit(__doc__)
    data = build(area, fetch(area)) if area else build_world(fetch_world())
    with open(out, "wb") as file:
        file.write(data)
    print(f"{out}: {len(data) / 1e6:.1f} MB")


if __name__ == "__main__":
    main()
