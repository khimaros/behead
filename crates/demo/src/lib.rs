//! what the demo draws and how it reacts to input, apart from where its
//! pictures go: the behead-demo program writes them to a pipe or a wayland
//! compositor, and the esp32 firmware to its h.264 encoder.

mod font;

use std::collections::{BTreeMap, VecDeque};

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
/// the input events listed under the status lines. at this many and this
/// size the list ends above the middle of an 800x480 screen
const EVENT_LOG: usize = 6;
const LOG_SCALE: u32 = 2;
const LOG_LINE_HEIGHT: u32 = (font::HEIGHT + 3) * LOG_SCALE;
/// how many characters of an event fit across an 800 pixel screen
const LOG_COLUMNS: usize = 64;
const WAITING: &str = "PRESS A BUTTON OR TURN THE KNOB";
/// android's key code for a rotary controller, whose turns arrive as relative events
const ROTARY: u32 = 65536;
pub const BAR_HEIGHT: u32 = 24;
pub const BAR_WIDTH: u32 = 16;
/// seconds the timing bar takes to cross the screen
const BAR_SWEEP_SECONDS: u64 = 2;

#[derive(Default)]
pub struct State {
    /// fingers currently on the screen, by pointer id
    pub pointers: BTreeMap<u32, (u32, u32)>,
    trail: VecDeque<(u32, u32)>,
    /// the latest input events, oldest first, as shown on screen
    events: VecDeque<String>,
    /// where the car's rotary knob stands, in detents from where it started
    knob: i64,
    /// where a finger rests on the car's touchpad, in the pad's own coordinates
    pad: Option<(u32, u32)>,
    pub touches: u64,
}

impl State {
    pub fn remember(&mut self, positions: impl IntoIterator<Item = (u32, u32)>) {
        self.trail.extend(positions);
        while self.trail.len() > TRAIL_LENGTH {
            self.trail.pop_front();
        }
    }

    pub fn log(&mut self, event: String) {
        self.events.push_back(event);
        while self.events.len() > EVENT_LOG {
            self.events.pop_front();
        }
    }
}

/// a picture as rgb24, top row first
pub struct Canvas {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

impl Canvas {
    pub fn new(width: u32, height: u32) -> Self {
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

/// bt.601 studio range, in 1/256ths: luma, and the two colour differences
const LUMA: ([i32; 3], i32) = ([66, 129, 25], 16);
const BLUE_DIFFERENCE: ([i32; 3], i32) = ([-38, -74, 112], 128);
const RED_DIFFERENCE: ([i32; 3], i32) = ([112, -94, -18], 128);

fn component(pixel: &[u8], (weights, offset): ([i32; 3], i32)) -> u8 {
    let sum: i32 = pixel.iter().zip(weights).map(|(&value, weight)| value as i32 * weight).sum();
    (((sum + 128) >> 8) + offset) as u8
}

impl Canvas {
    /// the picture as the esp32-p4's h.264 encoder reads it: yuv 4:2:0 with
    /// every pair of pixels as a colour byte and two of luma, the colour
    /// being blue difference on the first row of each two and red difference
    /// on the second. `out` holds three bytes for every two pixels, and the
    /// width is even
    pub fn pack_yuv420(&self, out: &mut [u8]) {
        let row_bytes = self.width as usize * 3;
        for (row, (pixels, packed)) in
            self.pixels.chunks_exact(row_bytes).zip(out.chunks_exact_mut(row_bytes / 2)).enumerate()
        {
            let colour = if row % 2 == 0 { BLUE_DIFFERENCE } else { RED_DIFFERENCE };
            for (pair, packed) in pixels.chunks_exact(6).zip(packed.chunks_exact_mut(3)) {
                packed.copy_from_slice(&[component(pair, colour), component(pair, LUMA), component(&pair[3..], LUMA)]);
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
pub fn apply(state: &mut State, line: &str) {
    let words: Vec<&str> = line.split_whitespace().collect();
    match words.as_slice() {
        ["touch", action, index, points @ ..] => {
            let points: Vec<(u32, (u32, u32))> = points.iter().filter_map(|p| parse_point(p)).collect();
            let acted = index.parse::<usize>().ok().and_then(|i| points.get(i)).copied();
            state.remember(points.iter().map(|&(_, position)| position));
            state.pointers = points.iter().copied().collect();
            match *action {
                "up" => state.pointers.clear(),
                "pointer-up" => drop(acted.map(|(id, _)| state.pointers.remove(&id))),
                "down" | "pointer-down" => state.touches += 1,
                _ => {}
            }
            // a moving finger would fill the log by itself, and shows as a marker anyway
            if let Some((id, (x, y))) = acted.filter(|_| *action != "move") {
                state.log(format!("TOUCH {} {id} {x},{y}", action.to_uppercase()));
            }
        }
        ["touchpad", action, index, points @ ..] => {
            let acted = index.parse::<usize>().ok().and_then(|i| parse_point(points.get(i)?));
            if let Some((id, (x, y))) = acted {
                state.pad = (*action != "up").then_some((x, y));
                if *action != "move" {
                    state.log(format!("TOUCHPAD {} {id} {x},{y}", action.to_uppercase()));
                }
            }
        }
        // what the server could not read, as it gave it: the kind, the channel and the bytes
        ["unknown", rest @ ..] => {
            let event = format!("UNKNOWN {}", rest.join(" ").to_uppercase());
            state.log(event.chars().take(LOG_COLUMNS).collect());
        }
        ["button", code, pressed] => {
            if let Ok(code) = code.parse() {
                state.log(format!("BUTTON {} {}", named(code), up_or_down(*pressed == "down")));
            }
        }
        ["relative", code, delta] => {
            if let (Ok(code), Ok(delta)) = (code.parse(), delta.parse::<i64>()) {
                if code == ROTARY {
                    state.knob += delta;
                }
                state.log(format!("RELATIVE {} {delta:+}", named(code)));
            }
        }
        ["absolute", code, value] => {
            if let (Ok(code), Ok(value)) = (code.parse(), value.parse::<i64>()) {
                state.log(format!("ABSOLUTE {} {value}", named(code)));
            }
        }
        _ => {}
    }
}

/// a key code with its name, where it has one
fn named(code: u32) -> String {
    format!("{code} {}", button_name(code)).trim_end().to_string()
}

pub fn up_or_down(down: bool) -> &'static str {
    match down {
        true => "DOWN",
        false => "UP",
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
        24 => "VOLUME UP",
        25 => "VOLUME DOWN",
        66 => "ENTER",
        84 => "VOICE",
        85 => "PLAY PAUSE",
        87 => "NEXT",
        88 => "PREVIOUS",
        126 => "PLAY",
        127 => "PAUSE",
        164 => "MUTE",
        ROTARY => "ROTARY",
        _ => "",
    }
}

/// draw one frame: status text, a bar sweeping at a fixed speed so stutter
/// shows, the trail of recent touches, and a marker under every finger
pub fn render(canvas: &mut Canvas, state: &State, fps: u32, frame: u64) {
    canvas.fill(BACKGROUND);
    let period = (fps as u64 * BAR_SWEEP_SECONDS).max(1);
    let bar_x = (frame % period) * (canvas.width - BAR_WIDTH) as u64 / period;
    canvas.rect(bar_x as i64, (canvas.height - BAR_HEIGHT) as i64, BAR_WIDTH, BAR_HEIGHT, BAR);
    for &(x, y) in &state.trail {
        canvas.rect(x as i64 - (TRAIL_DOT / 2) as i64, y as i64 - (TRAIL_DOT / 2) as i64, TRAIL_DOT, TRAIL_DOT, TRAIL);
    }
    let lines = [
        format!("{}X{} {} FPS  FRAME {frame}", canvas.width, canvas.height, fps),
        format!("TOUCHES {}  FINGERS {}  KNOB {}", state.touches, state.pointers.len(), state.knob),
        state.pad.map_or("PAD -".to_string(), |(x, y)| format!("PAD {x},{y}")),
    ];
    for (index, line) in lines.iter().enumerate() {
        canvas.text(MARGIN as i64, (MARGIN + index as u32 * LINE_HEIGHT) as i64, TEXT_SCALE, TEXT, line);
    }
    let log_top = MARGIN + lines.len() as u32 * LINE_HEIGHT;
    let waiting = state.events.is_empty().then_some(WAITING);
    for (index, event) in state.events.iter().map(String::as_str).chain(waiting).enumerate() {
        canvas.text(MARGIN as i64, (log_top + index as u32 * LOG_LINE_HEIGHT) as i64, LOG_SCALE, TEXT, event);
    }
    for (&id, &(x, y)) in &state.pointers {
        let (left, top) = (x as i64 - (MARKER / 2) as i64, y as i64 - (MARKER / 2) as i64);
        canvas.rect(left, top, MARKER, MARKER, POINTER_COLOURS[id as usize % POINTER_COLOURS.len()]);
        canvas.text(left + 2, top + 2, LABEL_SCALE, LABEL, &id.to_string());
    }
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
    fn lists_every_input_event_but_a_moving_finger() {
        let mut state = State::default();
        for line in ["button 23 down", "nonsense", "touch down 0 0:5,6", "touch move 0 0:7,8", "button 999 up"] {
            apply(&mut state, line);
        }
        apply(&mut state, "absolute 65536 40");
        let listed: Vec<&str> = state.events.iter().map(String::as_str).collect();
        assert_eq!(listed, ["BUTTON 23 ENTER DOWN", "TOUCH DOWN 0 5,6", "BUTTON 999 UP", "ABSOLUTE 65536 ROTARY 40"]);
    }

    #[test]
    fn follows_a_finger_on_the_touchpad_apart_from_the_screen() {
        let mut state = State::default();
        for line in ["touchpad down 0 0:10,20", "touchpad move 0 0:400,20"] {
            apply(&mut state, line);
        }
        assert_eq!(state.pad, Some((400, 20)));
        assert!(state.pointers.is_empty(), "a finger on the pad is not one on the screen");
        apply(&mut state, "touchpad up 0 0:400,20");
        assert_eq!(state.pad, None);
        let listed: Vec<&str> = state.events.iter().map(String::as_str).collect();
        assert_eq!(listed, ["TOUCHPAD DOWN 0 10,20", "TOUCHPAD UP 0 400,20"]);
    }

    #[test]
    fn lists_what_the_server_could_not_read() {
        let mut state = State::default();
        apply(&mut state, "unknown input 1 field 9 0102ab");
        apply(&mut state, &format!("unknown message 1 0x80f9 {}", "ab".repeat(40)));
        assert_eq!(state.events.front().map(String::as_str), Some("UNKNOWN INPUT 1 FIELD 9 0102AB"));
        assert_eq!(state.events.back().map(String::len), Some(LOG_COLUMNS));
    }

    #[test]
    fn follows_the_knob_both_ways() {
        let mut state = State::default();
        for line in ["relative 65536 3", "relative 65536 -1", "relative 7 5"] {
            apply(&mut state, line);
        }
        assert_eq!(state.knob, 2);
        assert_eq!(state.events.back().map(String::as_str), Some("RELATIVE 7 +5"));
        assert_eq!(state.events.front().map(String::as_str), Some("RELATIVE 65536 ROTARY +3"));
    }

    #[test]
    fn keeps_only_the_latest_events_and_clear_of_the_middle() {
        let (mut canvas, mut state) = (Canvas::new(800, 480), State::default());
        (0..EVENT_LOG + 3).for_each(|turn| apply(&mut state, &format!("relative 65536 {turn}")));
        assert_eq!(state.events.len(), EVENT_LOG);
        assert_eq!(state.events.back().map(String::as_str), Some("RELATIVE 65536 ROTARY +8"));
        render(&mut canvas, &state, 30, 0);
        assert!((0..800).all(|x| pixel(&canvas, x, 240) == BACKGROUND), "the list reaches the middle row");
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
    fn packs_a_picture_for_the_esp32_encoder() {
        let mut canvas = Canvas::new(4, 2);
        canvas.fill([255, 255, 255]);
        canvas.rect(2, 0, 2, 2, [255, 0, 0]);
        let mut packed = [0; 12];
        canvas.pack_yuv420(&mut packed);
        // white then red: no colour then little blue on the first row, much red on the second
        assert_eq!(packed, [128, 235, 235, 90, 82, 82, 128, 235, 235, 240, 82, 82]);
    }

    #[test]
    fn markers_at_the_edge_are_clipped() {
        let (mut canvas, mut state) = (Canvas::new(800, 480), State::default());
        apply(&mut state, "touch down 0 0:0,479");
        render(&mut canvas, &state, 30, 0);
        assert_eq!(pixel(&canvas, 1, 478), POINTER_COLOURS[0]);
    }
}
