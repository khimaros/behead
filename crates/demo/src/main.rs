//! demo video source for the behead server. draws the screen the car shows
//! and reacts to touches and buttons. by default it reads the input lines
//! the server writes to its stdin and writes raw rgb24 frames to stdout, at
//! the requested rate, for an encoder to compress:
//!
//!     behead-demo --width 800 --height 480 --fps 30 | ffmpeg -f rawvideo ...
//!
//! with --wayland it is a fullscreen wayland client instead, taking its size
//! and input from the compositor, so it shows that input injected with
//! `behead --uinput` reaches an ordinary application.

mod font;
mod wayland;

use std::collections::{BTreeMap, VecDeque};
use std::io::{BufRead, Write};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const USAGE: &str = "usage: behead-demo --width N --height N --fps N
       behead-demo --wayland [--fps N]";
/// the rate the wayland mode assumes when timing its sweeping bar
const DEFAULT_FPS: u32 = 30;
const BACKGROUND: [u8; 3] = [24, 28, 36];
const TEXT: [u8; 3] = [230, 230, 230];
const TRAIL: [u8; 3] = [90, 96, 110];
const BAR: [u8; 3] = [70, 130, 220];
const LABEL: [u8; 3] = [0, 0, 0];
/// finger markers, chosen by pointer id
const POINTER_COLOURS: [[u8; 3]; 4] = [[255, 64, 64], [64, 220, 96], [255, 200, 40], [200, 90, 255]];
const MARKER: u32 = 48;
const TRAIL_DOT: u32 = 6;
const TRAIL_LENGTH: usize = 400;
const TEXT_SCALE: u32 = 3;
const LABEL_SCALE: u32 = 4;
const MARGIN: u32 = 16;
const LINE_HEIGHT: u32 = (font::HEIGHT + 3) * TEXT_SCALE;
const BAR_HEIGHT: u32 = 24;
const BAR_WIDTH: u32 = 16;
/// seconds the timing bar takes to cross the screen
const BAR_SWEEP_SECONDS: u64 = 2;

#[derive(Default)]
struct State {
    /// fingers currently on the screen, by pointer id
    pointers: BTreeMap<u32, (u32, u32)>,
    trail: VecDeque<(u32, u32)>,
    /// the last button or key, as shown on screen
    button: Option<String>,
    touches: u64,
}

impl State {
    fn remember(&mut self, positions: impl IntoIterator<Item = (u32, u32)>) {
        self.trail.extend(positions);
        while self.trail.len() > TRAIL_LENGTH {
            self.trail.pop_front();
        }
    }
}

struct Canvas {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

impl Canvas {
    fn new(width: u32, height: u32) -> Self {
        Self { width, height, pixels: vec![0; (width * height * 3) as usize] }
    }

    fn fill(&mut self, colour: [u8; 3]) {
        self.pixels.chunks_exact_mut(3).for_each(|pixel| pixel.copy_from_slice(&colour));
    }

    /// a filled rectangle, clipped to the canvas
    fn rect(&mut self, x: i64, y: i64, width: u32, height: u32, colour: [u8; 3]) {
        let clip = |start: i64, length: u32, limit: u32| {
            (start.clamp(0, limit as i64) as usize, (start + length as i64).clamp(0, limit as i64) as usize)
        };
        let ((left, right), (top, bottom)) = (clip(x, width, self.width), clip(y, height, self.height));
        for row in top..bottom {
            let start = (row * self.width as usize + left) * 3;
            self.pixels[start..start + (right - left) * 3].chunks_exact_mut(3).for_each(|p| p.copy_from_slice(&colour));
        }
    }

    fn text(&mut self, x: i64, y: i64, scale: u32, colour: [u8; 3], text: &str) {
        for (index, c) in text.chars().enumerate() {
            let left = x + (index as u32 * (font::WIDTH + 1) * scale) as i64;
            for (row, bits) in font::glyph(c).iter().enumerate() {
                for column in (0..font::WIDTH).filter(|column| bits & (1 << (font::WIDTH - 1 - column)) != 0) {
                    let (px, py) = (left + (column * scale) as i64, y + (row as u32 * scale) as i64);
                    self.rect(px, py, scale, scale, colour);
                }
            }
        }
    }
}

fn parse_point(token: &str) -> Option<(u32, (u32, u32))> {
    let (id, position) = token.split_once(':')?;
    let (x, y) = position.split_once(',')?;
    Some((id.parse().ok()?, (x.parse().ok()?, y.parse().ok()?)))
}

/// update the state from one line of server input. unknown lines are ignored.
fn apply(state: &mut State, line: &str) {
    let words: Vec<&str> = line.split_whitespace().collect();
    match words.as_slice() {
        ["touch", action, index, points @ ..] => {
            let points: Vec<(u32, (u32, u32))> = points.iter().filter_map(|p| parse_point(p)).collect();
            state.remember(points.iter().map(|&(_, position)| position));
            state.pointers = points.iter().copied().collect();
            match *action {
                "up" => state.pointers.clear(),
                "pointer-up" => {
                    let lifted = index.parse::<usize>().ok().and_then(|i| points.get(i));
                    lifted.map(|(id, _)| state.pointers.remove(id));
                }
                "down" | "pointer-down" => state.touches += 1,
                _ => {}
            }
        }
        ["button", code, pressed] => {
            if let Ok(code) = code.parse() {
                state.button = Some(format!("BUTTON {code} {} {}", button_name(code), up_or_down(*pressed == "down")));
            }
        }
        _ => {}
    }
}

fn up_or_down(down: bool) -> &'static str {
    match down {
        true => "DOWN",
        false => "UP",
    }
}

/// the demo as a wayland client: input from the compositor, any size
struct WaylandDemo {
    state: State,
    canvas: Canvas,
    fps: u32,
    frame: u64,
}

impl wayland::App for WaylandDemo {
    fn input(&mut self, input: wayland::Input) {
        let state = &mut self.state;
        match input {
            wayland::Input::Down(id, x, y) => {
                state.pointers.insert(id, (x, y));
                state.touches += 1;
                state.remember([(x, y)]);
            }
            wayland::Input::Motion(id, x, y) => {
                state.pointers.insert(id, (x, y));
                state.remember([(x, y)]);
            }
            wayland::Input::Up(id) => drop(state.pointers.remove(&id)),
            wayland::Input::Cancel => state.pointers.clear(),
            wayland::Input::Key(code, down) => state.button = Some(format!("KEY {code} {}", up_or_down(down))),
        }
    }

    fn draw(&mut self, width: u32, height: u32) -> &[u8] {
        if (self.canvas.width, self.canvas.height) != (width, height) {
            self.canvas = Canvas::new(width, height);
        }
        render(&mut self.canvas, &self.state, self.fps, self.frame);
        self.frame += 1;
        &self.canvas.pixels
    }
}

fn button_name(code: u32) -> &'static str {
    match code {
        3 => "HOME",
        4 => "BACK",
        5 => "PHONE",
        6 => "CALL END",
        19 => "UP",
        20 => "DOWN",
        21 => "LEFT",
        22 => "RIGHT",
        23 => "ENTER",
        84 => "VOICE",
        85 => "PLAY PAUSE",
        87 => "NEXT",
        88 => "PREVIOUS",
        126 => "PLAY",
        127 => "PAUSE",
        65536 => "SCROLL",
        _ => "",
    }
}

/// draw one frame: status text, a bar sweeping at a fixed speed so stutter
/// shows, the trail of recent touches, and a marker under every finger
fn render(canvas: &mut Canvas, state: &State, fps: u32, frame: u64) {
    canvas.fill(BACKGROUND);
    let period = (fps as u64 * BAR_SWEEP_SECONDS).max(1);
    let bar_x = (frame % period) * (canvas.width - BAR_WIDTH) as u64 / period;
    canvas.rect(bar_x as i64, (canvas.height - BAR_HEIGHT) as i64, BAR_WIDTH, BAR_HEIGHT, BAR);
    for &(x, y) in &state.trail {
        canvas.rect(x as i64 - (TRAIL_DOT / 2) as i64, y as i64 - (TRAIL_DOT / 2) as i64, TRAIL_DOT, TRAIL_DOT, TRAIL);
    }
    let lines = [
        format!("{}X{} {} FPS  FRAME {frame}", canvas.width, canvas.height, fps),
        format!("TOUCHES {}  FINGERS {}", state.touches, state.pointers.len()),
        state.button.clone().unwrap_or("PRESS A BUTTON".into()),
    ];
    for (index, line) in lines.iter().enumerate() {
        canvas.text(MARGIN as i64, (MARGIN + index as u32 * LINE_HEIGHT) as i64, TEXT_SCALE, TEXT, line);
    }
    for (&id, &(x, y)) in &state.pointers {
        let (left, top) = (x as i64 - (MARKER / 2) as i64, y as i64 - (MARKER / 2) as i64);
        canvas.rect(left, top, MARKER, MARKER, POINTER_COLOURS[id as usize % POINTER_COLOURS.len()]);
        canvas.text(left + 2, top + 2, LABEL_SCALE, LABEL, &id.to_string());
    }
}

enum Mode {
    Pipe { width: u32, height: u32, fps: u32 },
    Wayland { fps: u32 },
}

fn parse_args(mut args: impl Iterator<Item = String>) -> Option<Mode> {
    let (mut width, mut height, mut fps, mut wayland) = (None, None, None, false);
    while let Some(flag) = args.next() {
        if flag == "--wayland" {
            wayland = true;
            continue;
        }
        let value = args.next()?.parse().ok()?;
        match flag.as_str() {
            "--width" => width = Some(value),
            "--height" => height = Some(value),
            "--fps" => fps = Some(value),
            _ => return None,
        }
    }
    if wayland {
        return (fps != Some(0)).then_some(Mode::Wayland { fps: fps.unwrap_or(DEFAULT_FPS) });
    }
    let (width, height, fps) = (width?, height?, fps?);
    (width > BAR_WIDTH && height > BAR_HEIGHT && fps > 0).then_some(Mode::Pipe { width, height, fps })
}

fn main() {
    match parse_args(std::env::args().skip(1)) {
        Some(Mode::Pipe { width, height, fps }) => run_pipe(width, height, fps),
        Some(Mode::Wayland { fps }) => {
            let mut demo = WaylandDemo { state: State::default(), canvas: Canvas::new(0, 0), fps, frame: 0 };
            if let Err(e) = wayland::run(&mut demo) {
                eprintln!("{e}");
                std::process::exit(1);
            }
        }
        None => {
            eprintln!("{USAGE}");
            std::process::exit(2);
        }
    }
}

/// frames to stdout at a steady rate, input lines from stdin
fn run_pipe(width: u32, height: u32, fps: u32) {
    let state = Arc::new(Mutex::new(State::default()));
    let input = state.clone();
    std::thread::spawn(move || {
        std::io::stdin().lock().lines().map_while(Result::ok).for_each(|line| apply(&mut input.lock().unwrap(), &line))
    });
    let (mut canvas, mut stdout) = (Canvas::new(width, height), std::io::stdout().lock());
    let (mut due, period) = (Instant::now(), Duration::from_secs(1) / fps);
    for frame in 0.. {
        render(&mut canvas, &state.lock().unwrap(), fps, frame);
        if stdout.write_all(&canvas.pixels).and_then(|_| stdout.flush()).is_err() {
            break;
        }
        due = next_due(due, period, Instant::now());
        std::thread::sleep(due.saturating_duration_since(Instant::now()));
    }
}

/// when the next frame is due: one period after the last, so a slow frame
/// does not shift the rest, unless that moment has already passed. a live
/// screen skips frames it fell behind on rather than sending them in a burst,
/// which is what happened while the encoder started up.
fn next_due(last: Instant, period: Duration, now: Instant) -> Instant {
    (last + period).max(now)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pixel(canvas: &Canvas, x: u32, y: u32) -> [u8; 3] {
        let offset = ((y * canvas.width + x) * 3) as usize;
        canvas.pixels[offset..offset + 3].try_into().unwrap()
    }

    #[test]
    fn tracks_fingers_through_a_two_finger_gesture() {
        let mut state = State::default();
        apply(&mut state, "touch down 0 0:100,100");
        apply(&mut state, "touch pointer-down 1 0:100,100 1:300,200");
        assert_eq!(state.pointers.len(), 2);
        apply(&mut state, "touch pointer-up 1 0:110,110 1:300,200");
        assert_eq!(state.pointers.into_iter().collect::<Vec<_>>(), vec![(0, (110, 110))]);
        assert_eq!(state.touches, 2);
    }

    #[test]
    fn remembers_the_last_button() {
        let mut state = State::default();
        apply(&mut state, "button 23 down");
        assert_eq!(state.button.as_deref(), Some("BUTTON 23 ENTER DOWN"));
        apply(&mut state, "nonsense");
        assert_eq!(state.button.as_deref(), Some("BUTTON 23 ENTER DOWN"));
    }

    #[test]
    fn draws_a_marker_under_each_finger() {
        let (mut canvas, mut state) = (Canvas::new(800, 480), State::default());
        apply(&mut state, "touch pointer-down 1 0:600,300 1:200,400");
        render(&mut canvas, &state, 30, 0);
        assert_eq!(pixel(&canvas, 600 + MARKER / 3, 300 + MARKER / 3), POINTER_COLOURS[0]);
        assert_eq!(pixel(&canvas, 200 + MARKER / 3, 400 + MARKER / 3), POINTER_COLOURS[1]);
        assert_eq!(pixel(&canvas, 400, 240), BACKGROUND);
    }

    #[test]
    fn keeps_a_steady_cadence() {
        let (start, period) = (Instant::now(), Duration::from_millis(33));
        assert_eq!(next_due(start, period, start + Duration::from_millis(5)), start + period);
    }

    #[test]
    fn skips_frames_it_fell_behind_on_instead_of_bursting() {
        let (start, period) = (Instant::now(), Duration::from_millis(33));
        let late = start + period * 5;
        assert_eq!(next_due(start, period, late), late);
    }

    #[test]
    fn markers_at_the_edge_are_clipped() {
        let (mut canvas, mut state) = (Canvas::new(800, 480), State::default());
        apply(&mut state, "touch down 0 0:0,479");
        render(&mut canvas, &state, 30, 0);
        assert_eq!(pixel(&canvas, 1, 478), POINTER_COLOURS[0]);
    }
}
