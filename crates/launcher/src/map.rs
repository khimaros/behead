//! the launcher's map: the roads, the water and the street names of an
//! area, drawn around a place from a file tools/fetch-map.py made of
//! openstreetmap's, whose layout is described there. the file holds the area
//! in squares of several sizes, and a picture reads the squares it shows of
//! the size that suits how close it is, so a card's worth of a country
//! costs a picture no more than a town does.
//!
//! places are in units of which the world is 2^32 around, laid out as a web
//! map's: x from the west, y from the far north. a view is so many of them
//! to a pixel, a power of two, which keeps the arithmetic to whole numbers.

use behead_demo::{plain, Surface, ADVANCE, GLYPH_HEIGHT};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::Mutex;

const EXTENSION: &str = "bmap";
const MAGIC: &[u8] = b"bmap";
const VERSION: u8 = 1;
const HEAD: usize = 8;
const LEVEL_HEAD: usize = 16;
const ENTRY: usize = 8;
/// a square's own coordinates go from 0 to 2^16
const LOCAL_BITS: u32 = 16;
const MOTORWAY: u8 = 1;
const MAIN: u8 = 2;
const MIDDLE: u8 = 3;
const MINOR: u8 = 4;
const WATER: u8 = 5;
const RIVER: u8 = 6;
const NAME: u8 = 7;
/// land, in a map of the world, where everything else is sea
const LAND_KIND: u8 = 8;
/// what is drawn over what: land and water under everything, the widest
/// roads last of the lines, and names over them all
const ORDER: [u8; 8] = [LAND_KIND, WATER, RIVER, MINOR, MIDDLE, MAIN, MOTORWAY, NAME];
pub const LAND: [u8; 3] = [26, 30, 38];
const WATER_COLOUR: [u8; 3] = [30, 52, 78];
const SEA: [u8; 3] = WATER_COLOUR;
/// a map whose coarsest squares are at least this wide, as a power of two
/// of units, is one of the world: a 32nd of it across
const WORLD_SHIFT: u32 = 27;
const NAME_COLOUR: [u8; 3] = [190, 194, 200];
/// a square of the level drawn is at least this many pixels wide: smaller,
/// and there are more streets in a picture than anyone could tell apart
const SQUARE_PIXELS: u32 = 256;
/// the closest and the widest view, as the power of two of units to a
/// pixel: about a metre at the latitudes people drive, and the whole world
/// on a screen 512 pixels wide
pub const ZOOM: (u8, u8) = (7, 23);
/// the view a map opens on, about four metres to a pixel
pub const ZOOM_START: u8 = 9;

/// a line's colour, and its width in pixels on a screen of 480 lines
fn stroke(kind: u8) -> ([u8; 3], u32) {
    match kind {
        MOTORWAY => ([232, 140, 64], 4),
        MAIN => ([214, 176, 84], 3),
        MIDDLE => ([128, 132, 140], 2),
        RIVER => (WATER_COLOUR, 2),
        _ => ([78, 84, 96], 1),
    }
}

/// where a map's bytes are: a file, read a square at a time, or memory
pub enum Source {
    File(Mutex<File>),
    Bytes(&'static [u8]),
}

impl Source {
    fn read(&self, offset: u32, length: usize) -> Option<Vec<u8>> {
        match self {
            Self::Bytes(bytes) => bytes.get(offset as usize..offset as usize + length).map(<[u8]>::to_vec),
            Self::File(file) => {
                let (mut file, mut bytes) = (file.lock().ok()?, vec![0; length]);
                file.seek(SeekFrom::Start(offset.into())).ok()?;
                file.read_exact(&mut bytes).ok()?;
                Some(bytes)
            }
        }
    }
}

fn number(bytes: &[u8], at: usize, length: usize) -> u32 {
    bytes[at..at + length].iter().rev().fold(0, |sum, &byte| sum << 8 | byte as u32)
}

/// the squares of one size: how wide, which of the world's they start at,
/// how many across and down, and where the file lists them
struct Level {
    shift: u32,
    column: u32,
    row: u32,
    columns: u32,
    rows: u32,
    table: u32,
}

/// a picture's view of the world: the place in its middle, the units to a
/// pixel as a power of two, and the picture's size
#[derive(Clone, Copy)]
pub struct View {
    pub centre: (u32, u32),
    pub zoom: u8,
    pub size: (u32, u32),
    /// pixels in one of the launcher's units, for the widths and the letters
    pub scale: u32,
}

impl View {
    /// where on the picture a place is. places off it come out off it
    pub fn pixel(&self, (x, y): (u32, u32)) -> (i64, i64) {
        let across =
            |place: u32, centre: u32, size: u32| ((place as i64 - centre as i64) >> self.zoom) + (size / 2) as i64;
        (across(x, self.centre.0, self.size.0), across(y, self.centre.1, self.size.1))
    }

    /// the place under a pixel
    fn place(&self, x: i64, y: i64) -> (i64, i64) {
        let back = |pixel: i64, centre: u32, size: u32| centre as i64 + ((pixel - (size / 2) as i64) << self.zoom);
        (back(x, self.centre.0, self.size.0), back(y, self.centre.1, self.size.1))
    }
}

/// a place in the map's units, from degrees
pub fn world(latitude: f64, longitude: f64) -> (u32, u32) {
    let whole = (1u64 << 32) as f64;
    let y = (1.0 - latitude.to_radians().tan().asinh() / std::f64::consts::PI) / 2.0;
    (((longitude + 180.0) / 360.0 * whole) as u32, (y * whole) as u32)
}

pub struct Map {
    source: Source,
    levels: Vec<Level>,
}

impl Map {
    /// None for anything that is not a map this can read
    pub fn open(source: Source) -> Option<Self> {
        let head = source.read(0, HEAD)?;
        if &head[..MAGIC.len()] != MAGIC || head[4] != VERSION {
            return None;
        }
        let heads = source.read(HEAD as u32, head[5] as usize * LEVEL_HEAD)?;
        let mut table = (HEAD + heads.len()) as u32;
        let levels = heads.chunks_exact(LEVEL_HEAD).map(|head| {
            let (columns, rows) = (number(head, 12, 2), number(head, 14, 2));
            let level = Level {
                shift: head[0].into(),
                column: number(head, 4, 4),
                row: number(head, 8, 4),
                columns,
                rows,
                table,
            };
            table += columns * rows * ENTRY as u32;
            level
        });
        let levels: Vec<Level> = levels.collect();
        (!levels.is_empty()).then_some(Self { source, levels })
    }

    fn finest(&self) -> &Level {
        self.levels.last().unwrap()
    }

    /// whether this is a map of the world, to go under the others
    pub fn is_world(&self) -> bool {
        self.levels[0].shift >= WORLD_SHIFT
    }

    /// whether a place is on this map
    pub fn covers(&self, (x, y): (u32, u32)) -> bool {
        let level = self.finest();
        let (column, row) = (x >> level.shift, y >> level.shift);
        (level.column..level.column + level.columns).contains(&column)
            && (level.row..level.row + level.rows).contains(&row)
    }

    /// the place in the middle of the map
    pub fn middle(&self) -> (u32, u32) {
        let level = self.finest();
        let half = |first: u32, count: u32| ((first as u64 * 2 + count as u64) << level.shift >> 1) as u32;
        (half(level.column, level.columns), half(level.row, level.rows))
    }

    /// the level for a view: the finest whose squares are wide enough on it
    fn level(&self, zoom: u8) -> &Level {
        let wide =
            |level: &&Level| level.shift >= zoom as u32 && (1u32 << (level.shift - zoom as u32)) >= SQUARE_PIXELS;
        self.levels.iter().rev().find(wide).unwrap_or(&self.levels[0])
    }

    /// the squares of a level a view shows, each with the place of its
    /// corner and its features
    fn squares(&self, level: &Level, view: &View) -> Vec<((u64, u64), Vec<u8>)> {
        let (left, top) = view.place(0, 0);
        let (right, bottom) = view.place(view.size.0.into(), view.size.1.into());
        let span = |from: i64, to: i64, first: u32, count: u32| {
            let within = |place: i64| (place >> level.shift).clamp(first as i64, (first + count) as i64 - 1) as u32;
            (from < ((first + count) as i64) << level.shift && to >= (first as i64) << level.shift)
                .then(|| within(from)..=within(to))
        };
        let (Some(columns), Some(rows)) =
            (span(left, right, level.column, level.columns), span(top, bottom, level.row, level.rows))
        else {
            return vec![];
        };
        let mut squares = vec![];
        for row in rows {
            for column in columns.clone() {
                let entry = level.table + ((row - level.row) * level.columns + column - level.column) * ENTRY as u32;
                let Some(entry) = self.source.read(entry, ENTRY) else { continue };
                let features = self.source.read(number(&entry, 0, 4), number(&entry, 4, 4) as usize);
                squares.push((
                    ((column as u64) << level.shift, (row as u64) << level.shift),
                    features.unwrap_or_default(),
                ));
            }
        }
        squares
    }

    /// draw the view: its water, its roads and their names
    pub fn draw(&self, surface: &mut impl Surface, view: &View) {
        let level = self.level(view.zoom);
        let squares = self.squares(level, view);
        let mut names: Vec<(i64, i64, i64, i64)> = vec![];
        for kind in ORDER {
            for (corner, features) in &squares {
                // the last of a square's coordinates is its far edge, where
                // the next square starts: what is cut at the edge meets
                // its other part there, with no gap between them
                let pixel = |x: u32, y: u32| {
                    let place = |local: u32, corner: u64| {
                        let across = (local + (local >> (LOCAL_BITS - 1))) as u64;
                        (corner + (across << level.shift >> LOCAL_BITS)).min(u32::MAX.into()) as u32
                    };
                    view.pixel((place(x, corner.0), place(y, corner.1)))
                };
                each(features, kind, |body| match kind {
                    NAME => name(surface, view, &mut names, pixel(number(body, 0, 2), number(body, 2, 2)), &body[5..]),
                    _ => {
                        let points: Vec<(i64, i64)> =
                            body.chunks_exact(4).map(|point| pixel(number(point, 0, 2), number(point, 2, 2))).collect();
                        match kind {
                            LAND_KIND => fill(surface, &points, LAND),
                            WATER => fill(surface, &points, WATER_COLOUR),
                            _ => {
                                let (colour, width) = stroke(kind);
                                let width = (width * view.scale / 3).max(1);
                                points.windows(2).for_each(|ends| line(surface, ends[0], ends[1], width, colour));
                            }
                        }
                    }
                });
            }
        }
    }
}

/// a view of several maps: the world's land and sea first, where one of
/// them is of the world, then every other map's part of the view over it.
/// without the world all is land. returns whether any has the view's middle
pub fn draw(maps: &[Map], surface: &mut impl Surface, view: &View) -> bool {
    surface.fill(if maps.iter().any(Map::is_world) { SEA } else { LAND });
    for world in [true, false] {
        maps.iter().filter(|map| map.is_world() == world).for_each(|map| map.draw(surface, view));
    }
    maps.iter().any(|map| map.covers(view.centre))
}

/// the maps in a directory, by name: the files in it that are one
pub fn maps_in(directory: &Path) -> Vec<Map> {
    let files = std::fs::read_dir(directory).into_iter().flatten().flatten().map(|entry| entry.path());
    let mut files: Vec<_> = files.filter(|path| path.extension().is_some_and(|ending| ending == EXTENSION)).collect();
    files.sort();
    files.iter().filter_map(|path| Map::open(Source::File(Mutex::new(File::open(path).ok()?)))).collect()
}

/// call with the body of every feature of one kind among a square's: a
/// shape's points, or a name's point, length and letters
fn each(mut features: &[u8], wanted: u8, mut found: impl FnMut(&[u8])) {
    while features.len() >= 3 {
        let kind = features[0];
        let (start, length) = match kind {
            NAME if features.len() >= 6 => (1, 5 + features[5] as usize),
            NAME => return,
            _ => (3, number(features, 1, 2) as usize * 4),
        };
        let Some(body) = features.get(start..start + length) else { return };
        if kind == wanted {
            found(body);
        }
        features = &features[start + length..];
    }
}

/// a street's name where it is, unless another name is there already
fn name(
    surface: &mut impl Surface,
    view: &View,
    taken: &mut Vec<(i64, i64, i64, i64)>,
    at: (i64, i64),
    letters: &[u8],
) {
    let text = plain(&String::from_utf8_lossy(letters));
    let scale = (view.scale / 3).max(1);
    let (width, height) = ((text.chars().count() as u32 * ADVANCE * scale) as i64, (GLYPH_HEIGHT * scale) as i64);
    let (left, top) = (at.0 - width / 2, at.1 - height / 2);
    let (right, bottom) = (left + width, top + height);
    let on_screen = left >= 0 && top >= 0 && right <= view.size.0 as i64 && bottom <= view.size.1 as i64;
    let free = taken.iter().all(|&(l, t, r, b)| right < l || left > r || bottom < t || top > b);
    if on_screen && free {
        taken.push((left - height, top - height, right + height, bottom + height));
        surface.text(left, top, scale, NAME_COLOUR, &text);
    }
}

/// a line so many pixels wide, a square of them at every step along it
fn line(surface: &mut impl Surface, from: (i64, i64), to: (i64, i64), width: u32, colour: [u8; 3]) {
    let (wide, high) = surface.size();
    let off = |a: i64, b: i64, size: u32| (a < 0 && b < 0) || (a > size as i64 && b > size as i64);
    if off(from.0, to.0, wide) || off(from.1, to.1, high) {
        return;
    }
    let (across, down) = ((to.0 - from.0).abs(), (to.1 - from.1).abs());
    let (step_x, step_y) = ((to.0 - from.0).signum(), (to.1 - from.1).signum());
    let (mut x, mut y, mut error, half) = (from.0, from.1, across - down, (width / 2) as i64);
    loop {
        surface.rect(x - half, y - half, width, width, colour);
        if (x, y) == to {
            return;
        }
        let twice = 2 * error;
        if twice > -down {
            error -= down;
            x += step_x;
        }
        if twice < across {
            error += across;
            y += step_y;
        }
    }
}

/// an area filled, a row of the picture at a time: between every other
/// pair of the places its outline crosses the row
fn fill(surface: &mut impl Surface, outline: &[(i64, i64)], colour: [u8; 3]) {
    let high = surface.size().1 as i64;
    let rows = outline.iter().map(|point| point.1);
    let (Some(top), Some(bottom)) = (rows.clone().min(), rows.max()) else { return };
    for y in top.max(0)..=bottom.min(high - 1) {
        let sides = outline.iter().zip(outline.iter().cycle().skip(1));
        let mut crossings: Vec<i64> = sides
            .filter(|(a, b)| (a.1 <= y) != (b.1 <= y))
            .map(|(a, b)| a.0 + (y - a.1) * (b.0 - a.0) / (b.1 - a.1))
            .collect();
        crossings.sort_unstable();
        crossings
            .chunks_exact(2)
            .for_each(|pair| surface.rect(pair[0], y, (pair[1] - pair[0]).max(1) as u32, 1, colour));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use behead_demo::Canvas;

    /// a map of one level, of one square that wide at that column and row,
    /// with shapes of a kind each, their points across the square
    fn square(shift: u8, (column, row): (u32, u32), shapes: &[(u8, &[u16])]) -> Map {
        let shape = |&(kind, points): &(u8, &[u16])| {
            let points: Vec<u8> = points.iter().flat_map(|n| n.to_le_bytes()).collect();
            [vec![kind, (points.len() / 4) as u8, 0], points].concat()
        };
        let features: Vec<u8> = shapes.iter().flat_map(shape).collect();
        let start = (HEAD + LEVEL_HEAD + ENTRY) as u32;
        let level = [vec![shift, 0, 0, 0], column.to_le_bytes().to_vec(), row.to_le_bytes().to_vec(), vec![1, 0, 1, 0]];
        let level = level.concat();
        let entry = [start.to_le_bytes(), (features.len() as u32).to_le_bytes()].concat();
        let file = [MAGIC.to_vec(), vec![VERSION, 1, 0, 0], level, entry, features].concat();
        Map::open(Source::Bytes(file.leak())).unwrap()
    }

    /// a square at the world's corner, with a main road across its middle
    /// and a pond above it
    fn one_square() -> Map {
        let pond = [0x2000, 0x2000, 0x4000, 0x2000, 0x4000, 0x4000, 0x2000, 0x4000];
        square(20, (0, 0), &[(MAIN, &[0, 0x8000, 0xffff, 0x8000]), (WATER, &pond)])
    }

    #[test]
    fn what_fills_two_squares_side_by_side_has_no_gap_where_they_meet() {
        // water from edge to edge of each, as a lake cut at the squares' edge is
        let full = [0, 0, 0xffff, 0, 0xffff, 0xffff, 0, 0xffff];
        let maps = [(0, 0), (1, 0), (0, 1)].map(|place| square(20, place, &[(WATER, &full)]));
        // the corner the three share, in the middle of the picture
        let view = View { centre: (1 << 20, 1 << 20), zoom: 12, size: (256, 256), scale: 3 };
        let mut canvas = Canvas::new(256, 256);
        draw(&maps, &mut canvas, &view);
        for x in 120..136 {
            assert_eq!(pixel(&canvas, x, 60), WATER_COLOUR, "from the left square into the right, at {x}");
        }
        for y in 120..136 {
            assert_eq!(pixel(&canvas, 60, y), WATER_COLOUR, "from the upper square into the lower, at {y}");
        }
    }

    #[test]
    fn the_world_is_under_every_map_and_the_sea_around_its_land() {
        // a sixteenth of the world, up and left of its middle, the quarter
        // of it nearest the middle being land
        let land = [0x8000, 0x8000, 0xffff, 0x8000, 0xffff, 0xffff, 0x8000, 0xffff];
        let world = square(30, (1, 1), &[(LAND_KIND, &land)]);
        assert!(world.is_world() && !one_square().is_world());
        let middle = (1u32 << 31) - (1 << 26);
        let view = View { centre: (middle, middle), zoom: 20, size: (256, 256), scale: 3 };
        let mut canvas = Canvas::new(256, 256);
        assert!(draw(&[one_square(), world], &mut canvas, &view), "the world has every place");
        assert_eq!(pixel(&canvas, 10, 128), LAND, "land, left of where it ends");
        assert_eq!(pixel(&canvas, 250, 128), SEA, "and the sea beyond");
        assert!(!draw(&[one_square()], &mut canvas, &view), "a town's map has not");
        assert_eq!(pixel(&canvas, 250, 128), LAND, "and without the world it is all land for want of better");
    }

    fn pixel(canvas: &Canvas, x: u32, y: u32) -> [u8; 3] {
        let at = ((y * canvas.width + x) * 3) as usize;
        canvas.pixels[at..at + 3].try_into().unwrap()
    }

    #[test]
    fn draws_the_road_and_the_water_of_the_square_in_view() {
        let (map, mut canvas) = (one_square(), Canvas::new(256, 256));
        // the whole square, 2^20 units wide, on 256 pixels
        let view = View { centre: map.middle(), zoom: 12, size: (256, 256), scale: 3 };
        canvas.fill(LAND);
        map.draw(&mut canvas, &view);
        assert_eq!(pixel(&canvas, 10, 128), stroke(MAIN).0, "the road, at the left of its row");
        assert_eq!(pixel(&canvas, 245, 128), stroke(MAIN).0, "and at the right");
        assert_eq!(pixel(&canvas, 48, 48), WATER_COLOUR, "inside the pond");
        assert_eq!(pixel(&canvas, 128, 200), LAND, "and nothing below the road");
        assert!(map.covers(map.middle()) && !map.covers((1 << 21, 0)));
    }

    #[test]
    fn a_closer_view_draws_everything_further_apart() {
        let (map, mut canvas) = (one_square(), Canvas::new(256, 256));
        let view = View { centre: map.middle(), zoom: 11, size: (256, 256), scale: 3 };
        canvas.fill(LAND);
        map.draw(&mut canvas, &view);
        assert_eq!(pixel(&canvas, 128, 128), stroke(MAIN).0, "the road still through the middle");
        assert_eq!(pixel(&canvas, 48, 48), LAND, "and the pond out of the picture");
    }

    #[test]
    fn places_are_where_a_web_map_has_them() {
        assert_eq!(world(0.0, 0.0), (1 << 31, 1 << 31), "the equator at greenwich is the middle");
        let (x, y) = world(36.0, -86.0);
        assert_eq!((x >> 18, y >> 18), (4278, 6433), "the tile of zoom 14 a place east of nashville is in");
        assert!(Map::open(Source::Bytes(b"not a map")).is_none());
    }
}
