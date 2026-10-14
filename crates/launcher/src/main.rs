//! the launcher as a video source for the behead server: it reads the input
//! lines the server writes to its stdin and writes raw rgb24 frames to
//! stdout, for an encoder to compress, as behead-demo does:
//!
//!     behead-launcher --width 800 --height 480 --fps 30 --music DIR | ffmpeg -f rawvideo ...
//!
//! its sound goes to a named pipe, made beforehand with mkfifo, which the
//! server's audio command reads:
//!
//!     behead --video-cmd '... --audio PIPE | ffmpeg ...' --audio-cmd 'cat PIPE'

use behead_demo::{next_due, Canvas};
use behead_launcher::{App, Player, Source, MIN_SIZE};
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const USAGE: &str = "usage: behead-launcher --width N --height N --fps N --music DIR [--maps DIR] [--audio PIPE]

  --music DIR   where the music is: mp3, flac, opus and wav files, in folders or not
  --maps DIR    where the maps are: the .bmap files tools/fetch-map.py makes
  --audio PIPE  a named pipe to write what plays to, as signed 16 bit little
                endian stereo pcm at 48000 hz, at the speed it is heard";
/// the format written to --audio, which is what cars take as media audio
const RATE: u32 = 48000;
const CHANNELS: u32 = 2;
/// length of each piece of sound written, as the server sends them
const PACKET: Duration = Duration::from_millis(40);

struct Config {
    width: u32,
    height: u32,
    fps: u32,
    music: PathBuf,
    maps: Option<PathBuf>,
    audio: Option<PathBuf>,
}

fn parse_args(mut args: impl Iterator<Item = String>) -> Option<Config> {
    let (mut width, mut height, mut fps, mut music, mut maps, mut audio) = (None, None, None, None, None, None);
    while let Some(flag) = args.next() {
        let value = args.next()?;
        match flag.as_str() {
            "--width" => width = value.parse().ok(),
            "--height" => height = value.parse().ok(),
            "--fps" => fps = value.parse().ok(),
            "--music" => music = Some(PathBuf::from(value)),
            "--maps" => maps = Some(PathBuf::from(value)),
            "--audio" => audio = Some(PathBuf::from(value)),
            _ => return None,
        }
    }
    let config = Config { width: width?, height: height?, fps: fps?, music: music?, maps, audio };
    (config.width >= MIN_SIZE.0 && config.height >= MIN_SIZE.1 && config.fps > 0).then_some(config)
}

/// write what plays to the pipe, a packet at a time and no faster than it
/// is heard, for as long as something reads it. opening the pipe waits for
/// a reader, and so for a car that wants the sound
fn play(mut source: Source, path: PathBuf) {
    let frames = (RATE as u128 * PACKET.as_millis() / 1000) as usize;
    loop {
        let mut pipe = match std::fs::OpenOptions::new().write(true).open(&path) {
            Ok(pipe) => pipe,
            Err(e) => return eprintln!("{}: {e}", path.display()),
        };
        let mut due = Instant::now();
        while pipe.write_all(&source.pcm(RATE, CHANNELS, frames)).is_ok() {
            due = next_due(due, PACKET, Instant::now());
            std::thread::sleep(due.saturating_duration_since(Instant::now()));
        }
    }
}

/// frames to stdout at a steady rate, input lines from stdin
fn run(config: Config) {
    let player = Player::default();
    let app = Arc::new(Mutex::new(App::new(player.clone(), config.width, config.height)));
    for map in config.maps.iter().flat_map(|maps| behead_launcher::maps_in(maps)) {
        app.lock().unwrap().add_map(map);
    }
    let (input, source) = (app.clone(), player.source());
    std::thread::spawn(move || {
        std::io::stdin().lock().lines().map_while(Result::ok).for_each(|line| input.lock().unwrap().apply(&line))
    });
    if let Some(path) = config.audio {
        std::thread::spawn(move || play(source, path));
    }
    std::thread::spawn(move || player.scan(&config.music));
    let (mut canvas, mut stdout) = (Canvas::new(config.width, config.height), std::io::stdout().lock());
    let (mut due, period) = (Instant::now(), Duration::from_secs(1) / config.fps);
    for frame in 0.. {
        app.lock().unwrap().render(&mut canvas, config.fps, frame);
        if stdout.write_all(&canvas.pixels).and_then(|_| stdout.flush()).is_err() {
            break;
        }
        due = next_due(due, period, Instant::now());
        std::thread::sleep(due.saturating_duration_since(Instant::now()));
    }
}

fn main() {
    match parse_args(std::env::args().skip(1)) {
        Some(config) => run(config),
        None => {
            eprintln!("{USAGE}");
            std::process::exit(2);
        }
    }
}
