//! a launcher for the car's screen: a home screen of tiles, a music player
//! for the files on a card, and the demo as an input test. everything works
//! by touch, and by a knob with a back button where the car has no
//! touchscreen. it draws on a `Surface` and takes the server's input lines,
//! so the behead-launcher program and the esp32 firmware show the same.

mod decode;
mod library;
mod opus;
mod player;

use behead_demo::{parse_point, Surface, ADVANCE, GLYPH_HEIGHT, ROTARY};
use library::Entry;
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
const TITLE: &str = "BEHEAD";
const MUSIC: &str = "MUSIC";
const TILES: [(&str, Screen); 2] = [(MUSIC, Screen::Music), ("INPUTS", Screen::Demo)];
// android's key codes, as the car sends them
const KEY_HOME: u32 = 3;
const KEY_BACK: u32 = 4;
const KEYS_BEFORE: [u32; 2] = [19, 21];
const KEYS_AFTER: [u32; 2] = [20, 22];
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
}

fn fit(text: &str, width: u32, scale: u32) -> String {
    text.chars().take((width / (ADVANCE * scale)) as usize).collect()
}

/// text at the left of an area, or in its middle, halfway down it
fn write(surface: &mut impl Surface, area: Area, scale: u32, centred: bool, colour: [u8; 3], text: &str) {
    let text = fit(text, area.width, scale);
    let spare = area.width - text.chars().count() as u32 * ADVANCE * scale;
    let x = area.x + if centred { spare / 2 } else { 0 };
    let y = area.y + area.height.saturating_sub(GLYPH_HEIGHT * scale) / 2;
    surface.text(x as i64, y as i64, scale, colour, &text);
}

fn panel(surface: &mut impl Surface, area: Area, colour: [u8; 3]) {
    surface.rect(area.x as i64, area.y as i64, area.width, area.height, colour);
}

impl App {
    pub fn new(player: Player, width: u32, height: u32) -> Self {
        let (size, demo) = ((width, height), behead_demo::State::default());
        Self { player, size, screen: Screen::Home, tile: 0, demo, folder: String::new(), cursor: 0, top: 0 }
    }

    /// the car's screen has this many pixels from now on
    pub fn resize(&mut self, width: u32, height: u32) {
        self.size = (width, height);
    }

    /// a number that is another whenever the picture would be: a host that
    /// keeps its pictures need not draw one it has. drawing costs the
    /// esp32-p4 more than its time, for a picture pushes what the music
    /// decoder needs out of the chip's cache
    pub fn shows(&self, frame: u64) -> u64 {
        let now = self.player.now().map(|now| (now.track, now.paused, now.elapsed.as_secs()));
        let listed = (&self.folder, self.cursor, self.top, self.player.tracks().len(), self.player.note());
        // the demo's bar moves in every picture
        let moving = (self.screen == Screen::Demo).then_some(frame);
        let mut hasher = DefaultHasher::new();
        (self.size, self.screen, self.tile, listed, now, moving).hash(&mut hasher);
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

    fn tiles(&self) -> Vec<Button> {
        let (scale, count) = (self.scale(), TILES.len() as u32);
        let (margin, top) = (MARGIN * scale, BAR * scale);
        let width = self.size.0.saturating_sub(margin * (count + 1)) / count;
        let height = self.size.1.saturating_sub(top + 2 * margin);
        let tile = |(index, &(label, screen)): (usize, &(&'static str, Screen))| {
            let area = Area { x: margin + index as u32 * (width + margin), y: top + margin, width, height };
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
        }
    }

    /// lines in the folder listed
    fn entries(&self) -> usize {
        library::entries(&self.player.tracks(), &self.folder).len()
    }

    /// the knob turned, or an arrow key stood in for it
    fn turn(&mut self, by: i64) {
        match self.screen {
            Screen::Home => self.tile = self.tile.saturating_add_signed(by as isize).min(TILES.len() - 1),
            Screen::Music => {
                self.cursor = self.cursor.saturating_add_signed(by as isize).min(self.entries().saturating_sub(1));
                self.reveal();
            }
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
            code if KEYS_BEFORE.contains(&code) => self.turn(-1),
            code if KEYS_AFTER.contains(&code) => self.turn(1),
            code if KEYS_ENTER.contains(&code) => match self.screen {
                Screen::Home => self.act(Action::Open(TILES[self.tile].1)),
                Screen::Music => self.choose(self.cursor),
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
            ["button", code, "down"] => code.parse().into_iter().for_each(|code| self.key(code)),
            ["relative", code, by] if code.parse() == Ok(ROTARY) => by.parse().into_iter().for_each(|by| self.turn(by)),
            _ => {}
        }
    }

    /// the lines of the folder listed, the one the knob is on marked, and
    /// the track that plays in a colour of its own
    fn render_list(&self, surface: &mut impl Surface, tracks: &[String], entries: &[Entry]) {
        let (scale, list, now) = (self.scale(), self.list(), self.player.now());
        if entries.is_empty() {
            write(surface, list, scale, true, TEXT, self.player.note());
        }
        let shown = entries.iter().enumerate().skip(self.top).take(self.rows());
        for (row, (index, entry)) in shown.enumerate() {
            let line = Area { y: list.y + row as u32 * ROW * scale, height: ROW * scale, ..list };
            if index == self.cursor {
                panel(surface, line, FOCUS);
            }
            let (text, colour) = match *entry {
                Entry::Folder(name) => (format!("{name}{}", library::SEPARATOR), TEXT),
                Entry::Track(track) if now.is_some_and(|now| now.track == track) => {
                    (format!("> {}", library::title(&tracks[track])), PLAYING)
                }
                Entry::Track(track) => (library::title(&tracks[track]).to_string(), TEXT),
            };
            let inside = Area { x: line.x + PAD * scale, width: line.width.saturating_sub(2 * PAD * scale), ..line };
            write(surface, inside, scale, false, colour, &text);
        }
    }

    /// what plays and for how long, beside the player's buttons
    fn render_footer(&self, surface: &mut impl Surface, tracks: &[String]) {
        let (scale, footer) = (self.scale(), self.footer());
        panel(surface, footer, PANEL);
        let Some(now) = self.player.now() else { return };
        let inside = Area { x: PAD * scale, width: footer.width.saturating_sub(2 * PAD * scale), ..footer };
        let title = tracks.get(now.track).map_or("", |track| library::title(track));
        write(surface, Area { height: ROW * scale, ..inside }, scale, false, PLAYING, title);
        let seconds = now.elapsed.as_secs();
        let time = Area { y: footer.y + ROW * scale, height: BAR * scale, ..inside };
        write(surface, time, scale, false, TEXT, &format!("{}:{:02}", seconds / 60, seconds % 60));
    }

    fn render_music(&self, surface: &mut impl Surface) {
        let (scale, tracks) = (self.scale(), self.player.tracks());
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
        self.render_list(surface, &tracks, &entries);
        self.render_footer(surface, &tracks);
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
            Screen::Music => self.render_music(surface),
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
        let root = std::env::temp_dir().join(format!("behead-launcher-{name}-{}", std::process::id()));
        let tracks = (0..10).map(|index| format!("{index}.wav"));
        for file in ["Album/one.wav", "Album/two.wav", "Zoo/x.wav"].map(String::from).into_iter().chain(tracks) {
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
    fn the_knob_and_keys_do_the_same() {
        let (mut app, player, _) = launcher("knob");
        feed(&mut app, &["relative 65536 1", "button 23 down", "button 23 up"]);
        assert_eq!(app.screen, Screen::Demo);
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
        let home = app.shows(0);
        assert_eq!(app.shows(1), home, "a still screen is the same picture later");
        feed(&mut app, &["relative 65536 1"]);
        assert_ne!(app.shows(1), home, "the knob moved to the other tile");
        feed(&mut app, &["relative 65536 -1", "touch move 0 0:5,5"]);
        assert_eq!(app.shows(2), home);
        player.play(3);
        assert_ne!(app.shows(2), home, "a track started, which the music screen marks");
        tap(&mut app, "600,240");
        assert_ne!(app.shows(3), app.shows(4), "the input test moves by itself");
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
