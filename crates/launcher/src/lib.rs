//! a launcher for the car's screen: a home screen of tiles, a music player
//! for the files on a card, a map of where the car is from files beside
//! them, and the demo as an input test. everything works
//! by touch, and by a knob with a back button where the car has no
//! touchscreen. it draws on a `Surface` and takes the server's input lines,
//! so the behead-launcher program and the esp32 firmware show the same.

mod decode;
mod library;
mod map;
mod opus;
mod player;

use behead_demo::{parse_point, plain, Surface, ADVANCE, GLYPH_HEIGHT, ROTARY};
use library::Entry;
pub use map::{maps_in, Map, Source as MapSource};
pub use player::{Now, Player, Source};
use std::hash::{DefaultHasher, Hash, Hasher};

const BACKGROUND: [u8; 3] = [24, 28, 36];
const PANEL: [u8; 3] = [40, 46, 58];
const BUTTON: [u8; 3] = [62, 70, 88];
const FOCUS: [u8; 3] = [70, 130, 220];
const TEXT: [u8; 3] = [230, 230, 230];
const PLAYING: [u8; 3] = [120, 220, 140];
/// a screen is this many units tall, whatever its pixels: a unit is 3 of
/// them at 480 lines and 4 at 720. the sizes below are in units
const UNITS: u32 = 160;
/// the smallest screen everything fits on, in pixels
pub const MIN_SIZE: (u32, u32) = (320, UNITS);
/// height of the header, and of a button
const BAR: u32 = 16;
/// height of a line of the list, tall enough for a finger
const ROW: u32 = 14;
const PAD: u32 = 4;
const MARGIN: u32 = 8;
/// width of a button with a word on it
const BUTTON_WIDTH: u32 = 44;
/// width of the buttons beside the list, which move it a page
const PAGER_WIDTH: u32 = 32;
/// how many times a tile's name is the size of other text
const TILE_TEXT: u32 = 2;
/// a name too long for its place slides a letter this often, slowly enough
/// to read and to cost the esp32-p4 few pictures drawn, and rests for this
/// many steps at each end
const SLIDE_STEPS_PER_SECOND: u64 = 2;
const SLIDE_REST: u64 = 4;
const TITLE: &str = "BEHEAD";
const MUSIC: &str = "MUSIC";
const TILES: [(&str, Screen); 3] = [(MUSIC, Screen::Music), ("MAP", Screen::Map), ("INPUTS", Screen::Demo)];
/// the home screen has room for this many tiles across and down
const TILE_GRID: (u32, u32) = (3, 2);
/// the car on the map: a dot this many units across, with a rim
const CAR: u32 = 4;
const CAR_RIM: [u8; 3] = [240, 240, 240];
const NO_MAP: &str = "NO MAP OF HERE";
// android's key codes, as the car sends them
const KEY_HOME: u32 = 3;
const KEY_BACK: u32 = 4;
/// a car's four arrows, up, down, left and right, and which way each goes:
/// to the right, and down
const ARROWS: [(u32, (i64, i64)); 4] = [(19, (0, -1)), (20, (0, 1)), (21, (-1, 0)), (22, (1, 0))];
/// an arrow moves the map this part of the screen
const PAN_PART: u32 = 4;
const KEYS_ENTER: [u32; 2] = [23, 66];
const KEY_PLAY_PAUSE: u32 = 85;
const KEY_NEXT: u32 = 87;
const KEY_PREVIOUS: u32 = 88;
const KEY_PLAY: u32 = 126;
const KEY_PAUSE: u32 = 127;

#[derive(Clone, Copy, Debug, Hash, PartialEq)]
enum Screen {
    Home,
    Music,
    Map,
    Demo,
}

/// a part of the picture, in pixels
#[derive(Clone, Copy)]
struct Area {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

impl Area {
    fn contains(&self, x: u32, y: u32) -> bool {
        (self.x..self.x + self.width).contains(&x) && (self.y..self.y + self.height).contains(&y)
    }
}

/// what a touch or a key asks for
#[derive(Clone, Copy, Debug, PartialEq)]
enum Action {
    Open(Screen),
    /// out of a folder, or of a screen
    Leave,
    /// the list a page on, or with -1 a page back
    Page(i64),
    /// the line of the list this far below its first
    Row(usize),
    /// the track after the one playing, or with -1 the one before
    Step(i64),
    Toggle,
    /// the map a step closer, or with -1 a step wider
    Zoom(i64),
    /// the map back around the car, to stay with it
    Follow,
}

struct Button {
    area: Area,
    label: &'static str,
    action: Action,
}

/// the launcher: which screen shows, and where in it the car's driver is
pub struct App {
    player: Player,
    size: (u32, u32),
    screen: Screen,
    /// the tile of the home screen the knob is on
    tile: usize,
    demo: behead_demo::State,
    /// the folder listed, as a path from the card's root
    folder: String,
    /// the line of its listing the knob is on, and the first one shown
    cursor: usize,
    top: usize,
    maps: Vec<Map>,
    /// where the car last said it is, and the place in the middle of the
    /// map, which is the car's for as long as the map follows it
    car: Option<(u32, u32)>,
    centre: (u32, u32),
    following: bool,
    zoom: u8,
    /// where a finger moving the map was last
    finger: Option<(u32, u32)>,
}

/// as much of a text as fits a width, in letters the font has
fn fit(text: &str, width: u32, scale: u32) -> String {
    plain(text).chars().take((width / (ADVANCE * scale)) as usize).collect()
}

/// text at the left of an area, or in its middle, halfway down it
fn write(surface: &mut impl Surface, area: Area, scale: u32, centred: bool, colour: [u8; 3], text: &str) {
    let text = fit(text, area.width, scale);
    let spare = area.width - text.chars().count() as u32 * ADVANCE * scale;
    let x = area.x + if centred { spare / 2 } else { 0 };
    let y = area.y + area.height.saturating_sub(GLYPH_HEIGHT * scale) / 2;
    surface.text(x as i64, y as i64, scale, colour, &text);
}

/// a line of a folder's listing as text, and the colour it is in: the
/// track that plays is marked, and in a colour of its own
fn label(tracks: &[String], entry: Entry, now: Option<Now>) -> (String, [u8; 3]) {
    match entry {
        Entry::Folder(name) => (format!("{name}{}", library::SEPARATOR), TEXT),
        Entry::Track(track) if now.is_some_and(|now| now.track == track) => {
            (format!("> {}", library::title(&tracks[track])), PLAYING)
        }
        Entry::Track(track) => (library::title(&tracks[track]).to_string(), TEXT),
    }
}

/// a text from this many of its letters on, as the font shows them
fn skip(text: &str, letters: usize) -> String {
    plain(text).chars().skip(letters).collect()
}

/// how many letters of a text have no room in a width
fn over(text: &str, width: u32, scale: u32) -> usize {
    plain(text).chars().count().saturating_sub((width / (ADVANCE * scale)) as usize)
}

/// which step of sliding a picture is at
fn slide_step(fps: u32, frame: u64) -> u64 {
    frame * SLIDE_STEPS_PER_SECOND / fps.max(1) as u64
}

/// how far a text with this many letters too many has slid at a step: a
/// letter a step to its end, a rest there, back the same way, and a rest
/// at its start
fn slide(over: usize, step: u64) -> usize {
    let (over, turn) = (over as u64, over as u64 + SLIDE_REST);
    let along = step % (2 * turn);
    along.min(2 * turn - along).saturating_sub(SLIDE_REST / 2).min(over) as usize
}

fn panel(surface: &mut impl Surface, area: Area, colour: [u8; 3]) {
    surface.rect(area.x as i64, area.y as i64, area.width, area.height, colour);
}

impl App {
    pub fn new(player: Player, width: u32, height: u32) -> Self {
        let (size, demo) = ((width, height), behead_demo::State::default());
        Self {
            player,
            size,
            screen: Screen::Home,
            tile: 0,
            demo,
            folder: String::new(),
            cursor: 0,
            top: 0,
            maps: vec![],
            car: None,
            centre: (0, 0),
            following: true,
            zoom: map::ZOOM_START,
            finger: None,
        }
    }

    /// another map to draw from. until the car says where it is, the map
    /// screen shows the middle of the first
    pub fn add_map(&mut self, map: Map) {
        if self.maps.is_empty() && self.car.is_none() {
            self.centre = map.middle();
        }
        self.maps.push(map);
    }

    /// the car's screen has this many pixels from now on
    pub fn resize(&mut self, width: u32, height: u32) {
        self.size = (width, height);
    }

    /// a number that is another whenever the picture would be: a host that
    /// keeps its pictures need not draw one it has. drawing costs the
    /// esp32-p4 more than its time, for a picture pushes what the music
    /// decoder needs out of the chip's cache
    pub fn shows(&self, fps: u32, frame: u64) -> u64 {
        let now = self.player.now().map(|now| (now.track, now.paused, now.elapsed.as_secs()));
        let listed = (&self.folder, self.cursor, self.top, self.player.tracks().len(), self.player.note());
        // the demo's bar moves in every picture, and a long name a few
        // times a second
        let moving = ((self.screen == Screen::Demo).then_some(frame), self.slides(slide_step(fps, frame)));
        let map = (self.centre, self.zoom, self.car, self.maps.len(), self.finger.is_some());
        let mapped = (self.screen == Screen::Map).then_some(map);
        let mut hasher = DefaultHasher::new();
        (self.size, self.screen, self.tile, listed, now, moving, mapped).hash(&mut hasher);
        hasher.finish()
    }

    /// pixels in a unit
    fn scale(&self) -> u32 {
        (self.size.1 / UNITS).max(1)
    }

    /// the footer of the music screen: what plays, above a line of buttons
    fn footer(&self) -> Area {
        let height = ((ROW + BAR) * self.scale()).min(self.size.1);
        Area { x: 0, y: self.size.1 - height, width: self.size.0, height }
    }

    /// where the music screen lists a folder
    fn list(&self) -> Area {
        let (scale, footer) = (self.scale(), self.footer());
        let width = self.size.0.saturating_sub(PAGER_WIDTH * scale);
        Area { x: 0, y: BAR * scale, width, height: footer.y.saturating_sub(BAR * scale) }
    }

    /// lines of the list that fit on the screen
    fn rows(&self) -> usize {
        (self.list().height / (ROW * self.scale())).max(1) as usize
    }

    /// the home screen's tiles, in rows from the top, each row from the left
    fn tiles(&self) -> Vec<Button> {
        let (scale, (columns, rows)) = (self.scale(), TILE_GRID);
        let (margin, top) = (MARGIN * scale, BAR * scale);
        let width = self.size.0.saturating_sub(margin * (columns + 1)) / columns;
        let height = self.size.1.saturating_sub(top + margin * (rows + 1)) / rows;
        let tile = |(index, &(label, screen)): (usize, &(&'static str, Screen))| {
            let (column, row) = (index as u32 % columns, index as u32 / columns);
            let (x, y) = (margin + column * (width + margin), top + margin + row * (height + margin));
            let area = Area { x, y, width, height };
            Button { area, label, action: Action::Open(screen) }
        };
        TILES.iter().enumerate().map(tile).collect()
    }

    /// the buttons of the music screen: back, a page up and down beside
    /// the list, and the player's under it
    fn music_buttons(&self) -> Vec<Button> {
        let (scale, list) = (self.scale(), self.list());
        let (wide, bar, half) = (BUTTON_WIDTH * scale, BAR * scale, list.height / 2);
        let pager = |y, label, by| {
            let area = Area { x: list.width, y, width: self.size.0 - list.width, height: half };
            Button { area, label, action: Action::Page(by) }
        };
        let toggle = if self.player.now().is_some_and(|now| !now.paused) { "PAUSE" } else { "PLAY" };
        let player = [("PREV", Action::Step(-1)), (toggle, Action::Toggle), ("NEXT", Action::Step(1))];
        let left = self.size.0.saturating_sub(wide * player.len() as u32);
        let player = player.into_iter().enumerate().map(|(index, (label, action))| {
            let area = Area { x: left + index as u32 * wide, y: self.size.1 - bar, width: wide, height: bar };
            Button { area, label, action }
        });
        let back = Button { area: Area { x: 0, y: 0, width: wide, height: bar }, label: "BACK", action: Action::Leave };
        [back, pager(list.y, "UP", -1), pager(list.y + half, "DOWN", 1)].into_iter().chain(player).collect()
    }

    fn buttons(&self) -> Vec<Button> {
        let (wide, bar) = (BUTTON_WIDTH * self.scale(), BAR * self.scale());
        match self.screen {
            Screen::Home => self.tiles(),
            Screen::Music => self.music_buttons(),
            // down the right of the map, where a driver's hand is nearest
            Screen::Map => {
                let column = [
                    ("BACK", Action::Leave),
                    ("+", Action::Zoom(1)),
                    ("-", Action::Zoom(-1)),
                    ("HERE", Action::Follow),
                ];
                let high = self.size.1 / column.len() as u32;
                let place = |(index, (label, action))| {
                    let area =
                        Area { x: self.size.0.saturating_sub(wide), y: index as u32 * high, width: wide, height: high };
                    Button { area, label, action }
                };
                column.into_iter().enumerate().map(place).collect()
            }
            // in the corner the demo writes nothing in
            Screen::Demo => {
                let area = Area { x: self.size.0.saturating_sub(wide), y: 0, width: wide, height: bar };
                vec![Button { area, label: "BACK", action: Action::Leave }]
            }
        }
    }

    /// keep the line the knob is on among the ones shown
    fn reveal(&mut self) {
        let rows = self.rows();
        self.top = self.top.clamp((self.cursor + 1).saturating_sub(rows), self.cursor);
    }

    /// list another folder, with the knob on the line named, if it is there
    fn list_folder(&mut self, folder: String, on: Option<&str>) {
        let tracks = self.player.tracks();
        let entries = library::entries(&tracks, &folder);
        self.cursor = entries.iter().position(|entry| Some(entry) == on.map(Entry::Folder).as_ref()).unwrap_or(0);
        (self.folder, self.top) = (folder, 0);
        self.reveal();
    }

    /// a line of the list was chosen: go into a folder, play a track, or
    /// pause the one that plays
    fn choose(&mut self, index: usize) {
        let tracks = self.player.tracks();
        match library::entries(&tracks, &self.folder).get(index) {
            Some(Entry::Folder(name)) => {
                let folder = [self.folder.as_str(), name].join(&library::SEPARATOR.to_string());
                self.list_folder(folder.trim_start_matches(library::SEPARATOR).to_string(), None);
            }
            Some(&Entry::Track(track)) => {
                self.cursor = index;
                match self.player.now() {
                    Some(now) if now.track == track => self.player.pause(|paused| !paused),
                    _ => self.player.play(track),
                }
            }
            None => {}
        }
    }

    fn act(&mut self, action: Action) {
        match action {
            Action::Open(screen) => {
                // the demo starts over, with no finger left on its screen
                self.demo = behead_demo::State::default();
                self.screen = screen;
            }
            Action::Leave if self.screen == Screen::Music && !self.folder.is_empty() => {
                let folder = self.folder.clone();
                let (parent, name) = library::split(&folder);
                self.list_folder(parent.to_string(), Some(name));
            }
            Action::Leave => self.act(Action::Open(Screen::Home)),
            Action::Page(by) => {
                let (rows, count) = (self.rows(), self.entries());
                self.top = self.top.saturating_add_signed(by as isize * rows as isize).min(count.saturating_sub(rows));
                self.cursor = self.cursor.clamp(self.top, (self.top + rows - 1).min(count.saturating_sub(1)));
            }
            Action::Row(row) => self.choose(self.top + row),
            Action::Step(by) => self.player.step(by),
            Action::Toggle => self.player.pause(|paused| !paused),
            // a step closer is half as many of the world's units to a pixel
            Action::Zoom(by) => self.zoom = (self.zoom as i64 - by).clamp(map::ZOOM.0.into(), map::ZOOM.1.into()) as u8,
            Action::Follow => {
                self.following = true;
                self.centre = self.car.unwrap_or(self.centre);
            }
        }
    }

    /// the car said where it is, in degrees
    fn located(&mut self, latitude: f64, longitude: f64) {
        self.car = Some(map::world(latitude, longitude));
        if self.following {
            self.centre = self.car.unwrap_or(self.centre);
        }
    }

    /// the map's middle so many pixels to the right and down of where it
    /// was, which is the map no longer with the car
    fn pan(&mut self, right: i64, down: i64) {
        let moved = |centre: u32, by: i64| (centre as i64 + (by << self.zoom)).clamp(0, u32::MAX.into()) as u32;
        self.centre = (moved(self.centre.0, right), moved(self.centre.1, down));
        self.following = false;
    }

    /// a finger on the map moved: the map goes with it
    fn dragged(&mut self, x: u32, y: u32) {
        if let Some(from) = self.finger.replace((x, y)) {
            self.pan(from.0 as i64 - x as i64, from.1 as i64 - y as i64);
        }
    }

    /// one of a car's four arrows: to the tile that way, along the list, or
    /// the map a part of the screen that way. the knob has the map's size
    fn arrow(&mut self, (right, down): (i64, i64)) {
        match self.screen {
            Screen::Home => {
                let to = self.tile as i64 + right + down * TILE_GRID.0 as i64;
                self.tile = if (0..TILES.len() as i64).contains(&to) { to as usize } else { self.tile };
            }
            Screen::Map => self.pan(right * (self.size.0 / PAN_PART) as i64, down * (self.size.1 / PAN_PART) as i64),
            _ => self.turn(right + down),
        }
    }

    /// lines in the folder listed
    fn entries(&self) -> usize {
        library::entries(&self.player.tracks(), &self.folder).len()
    }

    /// the knob turned, or an arrow went along a list
    fn turn(&mut self, by: i64) {
        match self.screen {
            Screen::Home => self.tile = self.tile.saturating_add_signed(by as isize).min(TILES.len() - 1),
            Screen::Music => {
                self.cursor = self.cursor.saturating_add_signed(by as isize).min(self.entries().saturating_sub(1));
                self.reveal();
            }
            // a wheel rolled up, as a map on a desk has it, is closer
            Screen::Map => self.act(Action::Zoom(-by)),
            Screen::Demo => {}
        }
    }

    fn key(&mut self, code: u32) {
        match code {
            KEY_HOME => self.act(Action::Open(Screen::Home)),
            KEY_BACK => self.act(Action::Leave),
            KEY_PLAY_PAUSE => self.act(Action::Toggle),
            KEY_PLAY => self.player.pause(|_| false),
            KEY_PAUSE => self.player.pause(|_| true),
            KEY_NEXT => self.act(Action::Step(1)),
            KEY_PREVIOUS => self.act(Action::Step(-1)),
            code if ARROWS.iter().any(|(arrow, _)| *arrow == code) => {
                ARROWS.iter().filter(|(arrow, _)| *arrow == code).for_each(|(_, way)| self.arrow(*way));
            }
            code if KEYS_ENTER.contains(&code) => match self.screen {
                Screen::Home => self.act(Action::Open(TILES[self.tile].1)),
                Screen::Music => self.choose(self.cursor),
                Screen::Map => self.act(Action::Follow),
                Screen::Demo => {}
            },
            _ => {}
        }
    }

    fn touch(&mut self, x: u32, y: u32) {
        let (list, row) = (self.list(), ROW * self.scale());
        if let Some(button) = self.buttons().iter().find(|button| button.area.contains(x, y)) {
            self.act(button.action);
        } else if self.screen == Screen::Music && list.contains(x, y) {
            self.act(Action::Row(((y - list.y) / row) as usize));
        } else if self.screen == Screen::Map {
            self.finger = Some((x, y));
        }
    }

    /// act on one line of server input. unknown lines are ignored
    pub fn apply(&mut self, line: &str) {
        if self.screen == Screen::Demo {
            behead_demo::apply(&mut self.demo, line);
        }
        let words: Vec<&str> = line.split_whitespace().collect();
        match words.as_slice() {
            ["touch", "down", index, points @ ..] => {
                let point = index.parse::<usize>().ok().and_then(|index| parse_point(points.get(index)?));
                point.into_iter().for_each(|(_, (x, y))| self.touch(x, y));
            }
            ["touch", "move", _, point, ..] if self.screen == Screen::Map => {
                parse_point(point).into_iter().for_each(|(_, (x, y))| self.dragged(x, y));
            }
            ["touch", "up", ..] => self.finger = None,
            ["location", latitude, longitude, ..] => {
                if let (Ok(latitude), Ok(longitude)) = (latitude.parse(), longitude.parse()) {
                    self.located(latitude, longitude);
                }
            }
            ["button", code, "down"] => code.parse().into_iter().for_each(|code| self.key(code)),
            ["relative", code, by] if code.parse() == Ok(ROTARY) => by.parse().into_iter().for_each(|by| self.turn(by)),
            _ => {}
        }
    }

    /// where a line of the list has its text, and the footer the title of
    /// what plays: all the width but a little at each side
    fn text_in(&self, area: Area) -> Area {
        let pad = PAD * self.scale();
        Area { x: area.x + pad, width: area.width.saturating_sub(2 * pad), ..area }
    }

    /// how far along the two texts that slide are at a step, when they are
    /// too long for their places: the line the knob is on, and the title of
    /// what plays, for as long as it does
    fn slides(&self, step: u64) -> (usize, usize) {
        let (scale, tracks, now) = (self.scale(), self.player.tracks(), self.player.now());
        if self.screen != Screen::Music {
            return (0, 0);
        }
        let slid = |text: &str, area: Area| slide(over(text, self.text_in(area).width, scale), step);
        let line = library::entries(&tracks, &self.folder).get(self.cursor).map(|entry| label(&tracks, *entry, now).0);
        let title =
            now.filter(|now| !now.paused).and_then(|now| tracks.get(now.track)).map(|track| library::title(track));
        (line.map_or(0, |line| slid(&line, self.list())), title.map_or(0, |title| slid(title, self.footer())))
    }

    /// the lines of the folder listed, the one the knob is on marked and
    /// slid this far, and the track that plays in a colour of its own
    fn render_list(&self, surface: &mut impl Surface, tracks: &[String], entries: &[Entry], slid: usize) {
        let (scale, list, now) = (self.scale(), self.list(), self.player.now());
        if entries.is_empty() {
            write(surface, list, scale, true, TEXT, self.player.note());
        }
        let shown = entries.iter().enumerate().skip(self.top).take(self.rows());
        for (row, (index, entry)) in shown.enumerate() {
            let line = Area { y: list.y + row as u32 * ROW * scale, height: ROW * scale, ..list };
            let (text, colour) = label(tracks, *entry, now);
            if index == self.cursor {
                panel(surface, line, FOCUS);
            }
            let from = if index == self.cursor { slid } else { 0 };
            write(surface, self.text_in(line), scale, false, colour, &skip(&text, from));
        }
    }

    /// what plays, its title slid this far, and for how long, beside the
    /// player's buttons
    fn render_footer(&self, surface: &mut impl Surface, tracks: &[String], slid: usize) {
        let (scale, footer) = (self.scale(), self.footer());
        panel(surface, footer, PANEL);
        let Some(now) = self.player.now() else { return };
        let inside = self.text_in(footer);
        let title = skip(tracks.get(now.track).map_or("", |track| library::title(track)), slid);
        write(surface, Area { height: ROW * scale, ..inside }, scale, false, PLAYING, &title);
        let seconds = now.elapsed.as_secs();
        let time = Area { y: footer.y + ROW * scale, height: BAR * scale, ..inside };
        write(surface, time, scale, false, TEXT, &format!("{}:{:02}", seconds / 60, seconds % 60));
    }

    fn render_music(&self, surface: &mut impl Surface, step: u64) {
        let (scale, tracks, (line, title)) = (self.scale(), self.player.tracks(), self.slides(step));
        let entries = library::entries(&tracks, &self.folder);
        surface.fill(BACKGROUND);
        let header = Area { x: 0, y: 0, width: self.size.0, height: BAR * scale };
        panel(surface, header, PANEL);
        let place = if entries.is_empty() { String::new() } else { format!("{}/{}", self.cursor + 1, entries.len()) };
        let taken = (BUTTON_WIDTH + PAD) * scale;
        let name = Area { x: taken, width: header.width.saturating_sub(2 * taken), ..header };
        write(surface, name, scale, false, TEXT, if self.folder.is_empty() { MUSIC } else { &self.folder });
        let counter = Area { x: header.width.saturating_sub(taken), width: BUTTON_WIDTH * scale, ..header };
        write(surface, counter, scale, true, TEXT, &place);
        self.render_list(surface, &tracks, &entries, line);
        self.render_footer(surface, &tracks, title);
    }

    /// the map around its middle, from every map that has some of it, and
    /// the car on it where it is in the picture
    fn render_map(&self, surface: &mut impl Surface) {
        let scale = self.scale();
        let moving = self.finger.is_some();
        let view = map::View { centre: self.centre, zoom: self.zoom, size: self.size, scale, moving };
        if !map::draw(&self.maps, surface, &view) {
            write(surface, Area { x: 0, y: 0, width: self.size.0, height: self.size.1 }, scale, true, TEXT, NO_MAP);
        }
        if let Some(car) = self.car {
            let (x, y) = view.pixel(car);
            let (rim, dot) = ((CAR * scale) as i64, ((CAR - 1) * scale) as i64);
            surface.rect(x - rim / 2, y - rim / 2, rim as u32, rim as u32, CAR_RIM);
            surface.rect(x - dot / 2, y - dot / 2, dot as u32, dot as u32, FOCUS);
        }
    }

    /// draw one frame of the screen that shows
    pub fn render(&self, surface: &mut impl Surface, fps: u32, frame: u64) {
        let scale = self.scale();
        match self.screen {
            Screen::Home => {
                surface.fill(BACKGROUND);
                let header = Area { x: 0, y: 0, width: self.size.0, height: BAR * scale };
                write(surface, header, scale, true, TEXT, TITLE);
            }
            Screen::Music => self.render_music(surface, slide_step(fps, frame)),
            Screen::Map => self.render_map(surface),
            Screen::Demo => behead_demo::render(surface, &self.demo, fps, frame),
        }
        let home = self.screen == Screen::Home;
        for (index, button) in self.buttons().iter().enumerate() {
            let area = button.area;
            let face = Area {
                x: area.x + scale,
                y: area.y + scale,
                width: area.width - 2 * scale,
                height: area.height - 2 * scale,
            };
            panel(surface, face, if home && index == self.tile { FOCUS } else { BUTTON });
            write(surface, face, if home { TILE_TEXT * scale } else { scale }, true, TEXT, button.label);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use behead_demo::Canvas;

    const SIZE: (u32, u32) = (800, 480);
    /// the middle of the music tile, of the list's first line, and of the
    /// buttons, on a screen of that size
    const MUSIC_TILE: &str = "200,240";
    const FIRST_LINE: &str = "200,69";
    const BACK: &str = "60,24";
    const PAGE_DOWN: &str = "750,300";
    const PAUSE: &str = "600,456";

    /// a launcher on a card with two folders and ten tracks beside them
    fn launcher(name: &str) -> (App, Player, std::path::PathBuf) {
        let tracks = (0..10).map(|index| format!("{index}.wav"));
        launcher_with(name, ["Album/one.wav", "Album/two.wav", "Zoo/x.wav"].map(String::from).into_iter().chain(tracks))
    }

    /// a launcher on a card of these files
    fn launcher_with(name: &str, files: impl Iterator<Item = String>) -> (App, Player, std::path::PathBuf) {
        let root = std::env::temp_dir().join(format!("behead-launcher-{name}-{}", std::process::id()));
        for file in files {
            std::fs::create_dir_all(root.join(&file).parent().unwrap()).unwrap();
            std::fs::write(root.join(file), b"").unwrap();
        }
        let player = Player::default();
        player.scan(&root);
        std::fs::remove_dir_all(&root).unwrap();
        (App::new(player.clone(), SIZE.0, SIZE.1), player, root)
    }

    fn feed(app: &mut App, lines: &[&str]) {
        lines.iter().for_each(|line| app.apply(line));
    }

    fn tap(app: &mut App, at: &str) {
        feed(app, &[&format!("touch down 0 0:{at}"), &format!("touch up 0 0:{at}")]);
    }

    fn pixel(app: &App, x: u32, y: u32) -> [u8; 3] {
        let mut canvas = Canvas::new(SIZE.0, SIZE.1);
        app.render(&mut canvas, 30, 0);
        let offset = ((y * SIZE.0 + x) * 3) as usize;
        canvas.pixels[offset..offset + 3].try_into().unwrap()
    }

    #[test]
    fn a_touch_opens_a_tile_a_folder_and_a_track() {
        let (mut app, player, _) = launcher("touch");
        assert_eq!(pixel(&app, 40, 240), FOCUS, "the knob starts on the music tile");
        assert_eq!(pixel(&app, 40, 400), BACKGROUND, "the tiles are in two rows of three, the second still empty");
        tap(&mut app, MUSIC_TILE);
        assert_eq!(app.screen, Screen::Music);
        tap(&mut app, FIRST_LINE);
        assert_eq!(app.folder, "Album");
        tap(&mut app, FIRST_LINE);
        assert_eq!(player.now().map(|now| (now.track, now.paused)), Some((10, false)));
        tap(&mut app, FIRST_LINE);
        assert_eq!(player.now().map(|now| now.paused), Some(true), "a touch on the track that plays pauses it");
        tap(&mut app, BACK);
        assert_eq!((app.folder.as_str(), app.cursor), ("", 0), "back on the folder it left");
        tap(&mut app, BACK);
        assert_eq!(app.screen, Screen::Home);
    }

    #[test]
    fn folders_inside_folders_open_and_close_in_turn() {
        let files = ["Artist/Album/Disc 2/one.wav", "Artist/Album/two.wav", "Artist/three.wav", "four.wav"];
        let (mut app, player, _) = launcher_with("nested", files.map(String::from).into_iter());
        tap(&mut app, MUSIC_TILE);
        for folder in ["Artist", "Artist/Album", "Artist/Album/Disc 2"] {
            tap(&mut app, FIRST_LINE);
            assert_eq!(app.folder, folder);
        }
        tap(&mut app, FIRST_LINE);
        assert_eq!(player.now().map(|now| now.track), Some(0));
        for folder in ["Artist/Album", "Artist", ""] {
            tap(&mut app, BACK);
            assert_eq!((app.screen, app.folder.as_str(), app.cursor), (Screen::Music, folder, 0));
        }
    }

    #[test]
    fn the_knob_and_keys_do_the_same() {
        let (mut app, player, _) = launcher("knob");
        feed(&mut app, &["relative 65536 1", "button 23 down", "button 23 up"]);
        assert_eq!(app.screen, Screen::Map);
        feed(&mut app, &["location 36.0 -86.0 - - - -", "relative 65536 -1", "relative 65536 3"]);
        let wider = (Some(map::world(36.0, -86.0)), map::ZOOM_START + 2);
        assert_eq!((app.car, app.zoom), wider, "clockwise, which is a wheel rolled down, is wider");
        feed(&mut app, &["touch down 0 0:100,100", "touch move 0 0:90,100", "touch up 0 0:90,100"]);
        assert_eq!(app.centre.0, map::world(36.0, -86.0).0 + (10 << app.zoom), "a finger takes the map with it");
        feed(&mut app, &["button 23 down"]);
        assert_eq!(Some(app.centre), app.car, "and a click has it back around the car");
        let (car, zoom) = (map::world(36.0, -86.0), app.zoom);
        feed(&mut app, &["button 22 down", "button 19 down"]);
        let moved = (car.0 + ((SIZE.0 / PAN_PART) << zoom), car.1 - ((SIZE.1 / PAN_PART) << zoom));
        assert_eq!((app.centre, app.zoom), (moved, zoom), "a car's arrows move the map and leave its size alone");
        feed(&mut app, &["button 3 down", "button 20 down"]);
        assert_eq!(app.tile, 1, "no tile is under the map's yet, so down goes nowhere");
        feed(&mut app, &["button 22 down", "button 23 down"]);
        assert_eq!(app.screen, Screen::Demo, "and right goes to the tile beside it");
        feed(&mut app, &["button 3 down", "relative 65536 -5", "button 66 down"]);
        assert_eq!(app.screen, Screen::Music);
        feed(&mut app, &["relative 65536 1", "button 23 down", "button 20 down"]);
        assert_eq!((app.folder.as_str(), app.cursor, player.now()), ("Zoo", 0, None), "no line after its one track");
        feed(&mut app, &["button 19 down", "button 23 down"]);
        assert_eq!(player.now().map(|now| now.track), Some(12));
        feed(&mut app, &["button 4 down"]);
        assert_eq!((app.folder.as_str(), app.cursor), ("", 1));
        feed(&mut app, &["button 4 down"]);
        assert_eq!(app.screen, Screen::Home);
    }

    #[test]
    fn the_cars_media_keys_work_on_any_screen() {
        let (mut app, player, _) = launcher("media");
        feed(&mut app, &["button 85 down"]);
        assert_eq!(player.now().map(|now| (now.track, now.paused)), Some((0, false)), "play starts at the first track");
        feed(&mut app, &["button 87 down", "button 87 down", "button 88 down", "button 127 down"]);
        assert_eq!(player.now().map(|now| (now.track, now.paused)), Some((1, true)));
        feed(&mut app, &["button 126 down"]);
        assert_eq!(player.now().map(|now| now.paused), Some(false));
        tap(&mut app, MUSIC_TILE);
        tap(&mut app, PAUSE);
        assert_eq!(player.now().map(|now| now.paused), Some(true));
    }

    #[test]
    fn the_list_follows_the_knob_and_moves_by_pages() {
        let (mut app, _, _) = launcher("pages");
        tap(&mut app, MUSIC_TILE);
        let rows = app.rows();
        assert_eq!(rows, 8);
        feed(&mut app, &["relative 65536 9"]);
        assert_eq!((app.cursor, app.top), (9, 2));
        assert_eq!(pixel(&app, 700, 48 + 42 * 7 + 4), FOCUS, "the line the knob is on is the last shown");
        feed(&mut app, &["relative 65536 99"]);
        assert_eq!((app.cursor, app.top), (11, 4));
        feed(&mut app, &["relative 65536 -99"]);
        tap(&mut app, PAGE_DOWN);
        assert_eq!((app.cursor, app.top), (4, 4), "the last page is a full one");
        tap(&mut app, "750,100");
        assert_eq!((app.cursor, app.top), (4, 0));
    }

    #[test]
    fn says_so_when_there_is_no_music_and_ignores_what_it_cannot_do() {
        let mut app = App::new(Player::default(), SIZE.0, SIZE.1);
        tap(&mut app, MUSIC_TILE);
        feed(&mut app, &["relative 65536 3", "button 23 down", "touch down 0 0:200,200", "touch down 5", "nonsense"]);
        tap(&mut app, PAGE_DOWN);
        assert_eq!((app.cursor, app.top), (0, 0));
        // halfway down the list, which is 342 pixels under a header of 48
        assert!((0..700).any(|x| pixel(&app, x, 219) == TEXT), "a note where the list would be");
    }

    #[test]
    fn the_input_test_takes_input_until_it_is_left() {
        let (mut app, _, _) = launcher("demo");
        tap(&mut app, "600,240");
        assert_eq!(app.screen, Screen::Demo);
        feed(&mut app, &["touch down 0 0:300,300"]);
        assert_eq!(app.demo.touches, 1);
        assert_eq!(pixel(&app, 300, 300), [255, 64, 64], "the demo's marker under the finger");
        tap(&mut app, "780,20");
        assert_eq!((app.screen, app.demo.touches), (Screen::Home, 0));
    }

    #[test]
    fn says_when_the_picture_would_be_another() {
        let (mut app, player, _) = launcher("shows");
        let home = app.shows(30, 0);
        assert_eq!(app.shows(30, 1), home, "a still screen is the same picture later");
        feed(&mut app, &["relative 65536 1"]);
        assert_ne!(app.shows(30, 1), home, "the knob moved to the other tile");
        feed(&mut app, &["relative 65536 -1", "touch move 0 0:5,5"]);
        assert_eq!(app.shows(30, 2), home);
        player.play(3);
        assert_ne!(app.shows(30, 2), home, "a track started, which the music screen marks");
        tap(&mut app, "600,240");
        assert_ne!(app.shows(30, 3), app.shows(30, 4), "the input test moves by itself");
    }

    #[test]
    fn a_text_slides_to_its_end_and_back_with_a_rest_at_each() {
        let slid: Vec<usize> = (0..15).map(|step| slide(3, step)).collect();
        assert_eq!(slid, [0, 0, 0, 1, 2, 3, 3, 3, 3, 3, 2, 1, 0, 0, 0]);
        assert!((0..40).all(|step| slide(0, step) == 0), "a text that fits stays where it is");
    }

    #[test]
    fn slides_a_long_name_on_the_knobs_line_and_the_title_of_what_plays() {
        // 60 letters, where a line of the list has room for 38 and the footer for 43
        let long = format!("{}.wav", "a long name of sixty letters ".repeat(3).split_at(60).0);
        let (mut app, player, _) = launcher_with("slide", [long, "short.wav".to_string()].into_iter());
        tap(&mut app, MUSIC_TILE);
        let (rested, moved) = (SLIDE_REST / 2, SLIDE_REST / 2 + 5);
        assert_eq!((app.slides(rested), app.slides(moved)), ((0, 0), (5, 0)), "the line the knob is on slides");
        assert_ne!(app.shows(30, 0), app.shows(30, 30 * moved / SLIDE_STEPS_PER_SECOND), "and is drawn anew for it");
        assert_eq!(app.shows(30, 0), app.shows(30, 1), "but not for every picture");
        feed(&mut app, &["button 23 down", "relative 65536 1"]);
        assert_eq!(app.slides(moved), (0, 5), "the title of what plays slides, and the short line does not");
        player.pause(|_| true);
        assert_eq!(app.slides(moved), (0, 0), "a title at rest stays where it is");
        feed(&mut app, &["button 126 down", "button 3 down"]);
        assert_eq!(app.slides(moved), (0, 0), "nothing slides on another screen");
    }

    #[test]
    fn fits_a_smaller_and_a_larger_screen() {
        for (width, height) in [MIN_SIZE, (1280, 720), (1920, 1080)] {
            let (mut app, _, _) = launcher(&format!("size-{height}"));
            app.resize(width, height);
            for screen in [Screen::Home, Screen::Music, Screen::Demo] {
                app.screen = screen;
                app.render(&mut Canvas::new(width, height), 30, 0);
                let inside = |area: &Area| area.x + area.width <= width && area.y + area.height <= height;
                assert!(app.buttons().iter().all(|button| inside(&button.area)), "{screen:?} at {width}x{height}");
            }
        }
    }
}
