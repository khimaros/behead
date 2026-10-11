//! what plays: the player is what the screen and the car's buttons hold,
//! and changes at once, and its source is where the sound is pulled from, a
//! packet at a time. files are opened and decoded by whoever pulls, so a
//! slow card keeps no button and no picture waiting.

use crate::decode::{self, Decoder, Format};
use crate::library;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

const READING: &str = "READING THE CARD";
const NO_CARD: &str = "NO CARD";
/// names the kinds that play: a card full of another kind looks like music
const NO_MUSIC: &str = "NO MP3, FLAC, OPUS OR WAV ON THE CARD";
const SAMPLE_BYTES: usize = size_of::<i16>();
const NANOS_PER_SECOND: u64 = 1_000_000_000;

/// what is playing, for the screen
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Now {
    /// the track, by its place among all of them
    pub track: usize,
    pub paused: bool,
    /// how much of it has been heard
    pub elapsed: Duration,
}

struct Controls {
    root: PathBuf,
    tracks: Arc<Vec<String>>,
    /// shown where the tracks would be, while there are none
    note: &'static str,
    now: Option<Now>,
    /// counts the tracks started, so the source knows one it has open from
    /// the same one asked for again
    started: u64,
}

/// the controls of the music, shared by everything that works them
#[derive(Clone)]
pub struct Player(Arc<Mutex<Controls>>);

impl Default for Player {
    fn default() -> Self {
        let controls = Controls { root: PathBuf::new(), tracks: Arc::default(), note: READING, now: None, started: 0 };
        Self(Arc::new(Mutex::new(controls)))
    }
}

impl Controls {
    fn play(&mut self, track: usize) {
        if track < self.tracks.len() {
            self.now = Some(Now { track, paused: false, elapsed: Duration::ZERO });
            self.started += 1;
        }
    }
}

impl Player {
    fn controls(&self) -> MutexGuard<'_, Controls> {
        self.0.lock().unwrap()
    }

    /// find the music under a directory. slow on a full card, so the screen
    /// says the card is being read until this returns
    pub fn scan(&self, root: &Path) {
        let found = library::scan(root);
        let mut controls = self.controls();
        controls.note = if found.is_ok() { NO_MUSIC } else { NO_CARD };
        (controls.root, controls.tracks) = (root.to_path_buf(), Arc::new(found.unwrap_or_default()));
    }

    pub fn tracks(&self) -> Arc<Vec<String>> {
        self.controls().tracks.clone()
    }

    pub fn note(&self) -> &'static str {
        self.controls().note
    }

    pub fn now(&self) -> Option<Now> {
        self.controls().now
    }

    /// play a track from its start
    pub fn play(&self, track: usize) {
        self.controls().play(track);
    }

    /// pause or go on, or with nothing chosen yet, start at the first track
    pub fn pause(&self, paused: impl FnOnce(bool) -> bool) {
        let mut controls = self.controls();
        match controls.now.as_mut() {
            Some(now) => now.paused = paused(now.paused),
            None if !paused(true) => controls.play(0),
            None => {}
        }
    }

    /// the track after the one playing, or with -1 the one before
    pub fn step(&self, by: i64) {
        let mut controls = self.controls();
        if let Some(track) = controls.now.and_then(|now| now.track.checked_add_signed(by as isize)) {
            controls.play(track);
        }
    }

    /// where the sound comes out. one at a time: two would each play
    /// half of a track
    pub fn source(&self) -> Source {
        Source {
            player: self.clone(),
            open: None,
            piece: Vec::new(),
            ready: VecDeque::new(),
            resampler: Resampler::default(),
        }
    }

    /// the track the source had open is over, or would not open: on to the
    /// next, and after the last, nothing
    fn finished(&self, started: u64) {
        let mut controls = self.controls();
        let same = controls.started == started;
        if let Some(now) = controls.now.take_if(|_| same) {
            controls.play(now.track + 1);
        }
    }

    fn heard(&self, started: u64, time: Duration) {
        let mut controls = self.controls();
        if controls.started == started {
            controls.now.iter_mut().for_each(|now| now.elapsed += time);
        }
    }
}

/// makes sound of one rate into sound of another, a piece at a time, by
/// drawing a line from each sample to the next. that dulls the highest
/// notes a little, which a car hides
#[derive(Default)]
struct Resampler {
    /// the frame before the one being read
    last: [i32; 2],
    /// how far past it the next frame to make lies, in parts of the new rate
    phase: u64,
}

impl Resampler {
    /// add a piece to the stereo frames made so far. of more than two
    /// channels the first two are kept, and one becomes both
    fn feed(&mut self, samples: &[i16], format: Format, rate: u32, made: &mut VecDeque<[i16; 2]>) {
        let (from, to) = (format.rate as u64, rate.max(1) as u64);
        if from == 0 || format.channels == 0 {
            return;
        }
        for frame in samples.chunks_exact(format.channels) {
            let next = [frame[0] as i32, frame[format.channels.min(2) - 1] as i32];
            while self.phase < to {
                let between = |channel: usize| {
                    let (before, after) = (self.last[channel] as i64, next[channel] as i64);
                    (before + (after - before) * self.phase as i64 / to as i64) as i16
                };
                made.push_back([between(0), between(1)]);
                self.phase += from;
            }
            self.phase -= to;
            self.last = next;
        }
    }
}

/// a frame as bytes for a headunit with this many channels
fn bytes(frame: [i16; 2], channels: usize, pcm: &mut Vec<u8>) {
    match channels {
        1 => pcm.extend(((frame[0] as i32 + frame[1] as i32) / 2).to_le_bytes().iter().take(SAMPLE_BYTES)),
        _ => {
            frame.iter().for_each(|sample| pcm.extend(sample.to_le_bytes()));
            pcm.resize(pcm.len() + (channels - frame.len()) * SAMPLE_BYTES, 0);
        }
    }
}

/// the sound of what the player plays, decoded as it is asked for
pub struct Source {
    player: Player,
    /// the file being played, and which start of a track it belongs to
    open: Option<(u64, Box<dyn Decoder>)>,
    piece: Vec<i16>,
    /// frames decoded and not yet handed out
    ready: VecDeque<[i16; 2]>,
    resampler: Resampler,
}

impl Source {
    /// open the track the player is at, unless it is open already. true
    /// while there is something to decode
    fn follow(&mut self) -> bool {
        let (started, paused, path) = {
            let controls = self.player.controls();
            let path = controls.now.and_then(|now| Some(controls.root.join(controls.tracks.get(now.track)?)));
            (controls.started, controls.now.is_none_or(|now| now.paused), path)
        };
        if self.open.as_ref().is_some_and(|(open, _)| *open == started) {
            return !paused;
        }
        (self.open, self.resampler) = (None, Resampler::default());
        self.ready.clear();
        let Some(path) = path else { return false };
        match decode::open(&path) {
            Ok(decoder) => self.open = Some((started, decoder)),
            Err(e) => {
                eprintln!("music: {e}");
                self.player.finished(started);
            }
        }
        true
    }

    /// decode the open file's next piece, or close it at its end
    fn decode(&mut self, rate: u32) {
        let Some((started, decoder)) = self.open.as_mut() else { return };
        match decoder.next(&mut self.piece) {
            Some(format) => self.resampler.feed(&self.piece, format, rate, &mut self.ready),
            None => {
                self.player.finished(*started);
                self.open = None;
            }
        }
    }

    /// the next `frames` of sound as signed 16 bit little endian pcm, with
    /// silence where nothing plays. a track that ends is followed by the
    /// next without a gap
    pub fn pcm(&mut self, rate: u32, channels: u32, frames: usize) -> Vec<u8> {
        while self.ready.len() < frames && self.follow() {
            self.decode(rate);
        }
        let playing = self.player.now().is_some_and(|now| !now.paused);
        let taken = if playing { frames.min(self.ready.len()) } else { 0 };
        let mut pcm = Vec::with_capacity(frames * channels as usize * SAMPLE_BYTES);
        self.ready.drain(..taken).for_each(|frame| bytes(frame, channels as usize, &mut pcm));
        pcm.resize(frames * channels as usize * SAMPLE_BYTES, 0);
        if let Some((started, _)) = self.open {
            let heard = Duration::from_nanos(taken as u64 * NANOS_PER_SECOND / rate.max(1) as u64);
            self.player.heard(started, heard);
        }
        pcm
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode::tests::wav;

    const RATE: u32 = 48000;

    fn sine(rate: u32, frequency: f32, frames: usize) -> Vec<i16> {
        (0..frames)
            .map(|n| ((n as f32 * frequency / rate as f32 * std::f32::consts::TAU).sin() * 8000.0) as i16)
            .collect()
    }

    fn rises(samples: impl Iterator<Item = i16>) -> usize {
        let samples: Vec<i16> = samples.collect();
        samples.windows(2).filter(|pair| pair[0] < 0 && pair[1] >= 0).count()
    }

    fn left(pcm: &[u8]) -> impl Iterator<Item = i16> + '_ {
        pcm.chunks_exact(4).map(|frame| i16::from_le_bytes([frame[0], frame[1]]))
    }

    /// a directory of wav files, each a second of one pitch at 44100 hz
    fn card(name: &str, pitches: &[f32]) -> PathBuf {
        let root = std::env::temp_dir().join(format!("behead-player-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        for (index, &pitch) in pitches.iter().enumerate() {
            let sound: Vec<u8> = sine(44100, pitch, 44100).iter().flat_map(|sample| sample.to_le_bytes()).collect();
            std::fs::write(root.join(format!("{index}.wav")), wav(44100, 1, 16, &sound)).unwrap();
        }
        root
    }

    fn loaded(name: &str, pitches: &[f32]) -> (Player, Source, PathBuf) {
        let (player, root) = (Player::default(), card(name, pitches));
        player.scan(&root);
        (player.clone(), player.source(), root)
    }

    #[test]
    fn keeps_the_pitch_at_another_rate_and_fills_both_channels() {
        let (mut resampler, mut made) = (Resampler::default(), VecDeque::new());
        // in pieces that end where no cycle does
        for piece in sine(44100, 440.0, 44100).chunks(1000) {
            resampler.feed(piece, Format { rate: 44100, channels: 1 }, RATE, &mut made);
        }
        assert!(made.len().abs_diff(RATE as usize) <= 1, "{} frames of a second", made.len());
        assert!(made.iter().all(|frame| frame[0] == frame[1]));
        assert!(rises(made.iter().map(|frame| frame[0])).abs_diff(440) <= 1);
    }

    #[test]
    fn passes_sound_of_the_same_rate_through_a_frame_late() {
        let (mut resampler, mut made) = (Resampler::default(), VecDeque::new());
        resampler.feed(&[1, 2, 3, 4, 5, 6], Format { rate: RATE, channels: 2 }, RATE, &mut made);
        assert_eq!(made, [[0, 0], [1, 2], [3, 4]]);
    }

    #[test]
    fn is_silent_until_a_track_is_chosen_and_while_paused() {
        let (player, mut source, root) = loaded("pause", &[440.0]);
        assert_eq!(source.pcm(RATE, 2, 480), vec![0; 1920]);
        player.play(0);
        assert!(rises(left(&source.pcm(RATE, 2, 4800))).abs_diff(44) <= 1);
        player.pause(|paused| !paused);
        assert_eq!(source.pcm(RATE, 2, 480), vec![0; 1920]);
        assert_eq!(player.now().map(|now| (now.paused, now.elapsed)), Some((true, Duration::from_millis(100))));
        player.pause(|paused| !paused);
        assert!(rises(left(&source.pcm(RATE, 2, 4800))).abs_diff(44) <= 1);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn plays_the_next_track_after_one_ends_and_stops_after_the_last() {
        let (player, mut source, root) = loaded("next", &[440.0, 880.0]);
        player.play(0);
        let first = source.pcm(RATE, 2, RATE as usize);
        assert_eq!(player.now().map(|now| now.track), Some(0));
        let second = source.pcm(RATE, 2, RATE as usize);
        assert_eq!(player.now().map(|now| now.track), Some(1));
        assert!(rises(left(&first)).abs_diff(440) <= 2 && rises(left(&second)).abs_diff(880) <= 2);
        source.pcm(RATE, 2, 4800);
        assert_eq!(player.now(), None);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn steps_between_tracks_and_starts_at_the_first_when_told_to_play() {
        let (player, mut source, root) = loaded("step", &[440.0, 880.0]);
        player.pause(|_| true);
        assert_eq!(player.now(), None, "pausing nothing starts nothing");
        player.pause(|_| false);
        player.step(1);
        player.step(1);
        assert_eq!(player.now().map(|now| now.track), Some(1), "there is no third track");
        assert!(rises(left(&source.pcm(RATE, 2, RATE as usize / 2))).abs_diff(440) <= 2);
        player.step(-1);
        assert!(rises(left(&source.pcm(RATE, 2, RATE as usize / 2))).abs_diff(220) <= 2);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn skips_a_file_that_will_not_open() {
        let (player, mut source, root) = loaded("broken", &[440.0, 880.0]);
        std::fs::write(root.join("0.wav"), b"not a wav file at all").unwrap();
        player.play(0);
        assert!(rises(left(&source.pcm(RATE, 2, RATE as usize / 2))).abs_diff(440) <= 2);
        assert_eq!(player.now().map(|now| now.track), Some(1));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn says_why_there_is_nothing_to_list() {
        let player = Player::default();
        assert_eq!(player.note(), READING);
        player.scan(Path::new("/nonexistent/behead"));
        assert_eq!(player.note(), NO_CARD);
    }

    #[test]
    fn mixes_down_for_one_channel_and_leaves_more_silent() {
        let (mut one, mut four) = (Vec::new(), Vec::new());
        bytes([100, 300], 1, &mut one);
        bytes([100, 300], 4, &mut four);
        assert_eq!(one, 200i16.to_le_bytes());
        assert_eq!(four, [100, 0, 44, 1, 0, 0, 0, 0]);
    }
}
