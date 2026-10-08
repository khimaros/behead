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

mod wayland;

use behead_demo::{apply, render, up_or_down, Canvas, State, BAR_HEIGHT, BAR_WIDTH};
use std::io::{BufRead, Write};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const USAGE: &str = "usage: behead-demo --width N --height N --fps N
       behead-demo --wayland [--fps N]";
/// the rate the wayland mode assumes when timing its sweeping bar
const DEFAULT_FPS: u32 = 30;

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
                state.log(format!("TOUCH DOWN {id} {x},{y}"));
            }
            wayland::Input::Motion(id, x, y) => {
                state.pointers.insert(id, (x, y));
                state.remember([(x, y)]);
            }
            wayland::Input::Up(id) => {
                state.pointers.remove(&id);
                state.log(format!("TOUCH UP {id}"));
            }
            wayland::Input::Cancel => state.pointers.clear(),
            wayland::Input::Key(code, down) => state.log(format!("KEY {code} {}", up_or_down(down))),
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
}
