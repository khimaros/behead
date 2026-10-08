//! media audio from --audio-cmd to the headunit, and the headunit
//! microphone to --mic-cmd

use crate::{command, send_when_ready, Shared};
use aap::AudioFormat;
use std::collections::VecDeque;
use std::io::{ErrorKind, Read, Write};
use std::process::{Child, ChildStdin, Stdio};
use std::time::{Duration, Instant};

/// length of each media audio packet, close to what openauto expects
const PACKET_MS: usize = 40;
const MICROS_PER_SECOND: u64 = 1_000_000;
/// pipe writes up to this size are all or nothing, so a dropped piece of
/// microphone audio never splits a sample
const PIPE_BUF: usize = 4096;

/// the replacements for an audio command template
pub fn values(format: AudioFormat) -> [(&'static str, u32); 3] {
    [("rate", format.rate), ("bits", format.bits), ("channels", format.channels)]
}

/// run the audio command and send its pcm until the stream stops. packets
/// are timestamped by the samples sent before them, and each waits `delay`
/// after it was read: a picture takes longer to reach the car's screen than
/// sound its speakers, and holding the sound back keeps the two together.
pub fn stream(shared: Shared, epoch: u64, command: String, format: AudioFormat, delay: Duration) {
    let mut child = match command::spawn(&command, Stdio::piped()) {
        Ok(child) => child,
        Err(e) => return eprintln!("audio: cannot run {command}: {e}"),
    };
    let mut stdout = child.stdout.take().unwrap();
    let frame_bytes = (format.bits as usize / 8 * format.channels as usize).max(1);
    let mut packet = vec![0; format.rate as usize * PACKET_MS / 1000 * frame_bytes];
    let (mut frames, mut held) = (0, VecDeque::new());
    'stream: while stdout.read_exact(&mut packet).is_ok() {
        held.push_back((Instant::now(), packet.clone()));
        while held.front().is_some_and(|(read, _)| read.elapsed() >= delay) {
            let (_, pcm) = held.pop_front().unwrap();
            let timestamp = frames * MICROS_PER_SECOND / format.rate.max(1) as u64;
            let live = |link: &crate::Link| link.host.audio_epoch == epoch;
            let ready = |link: &crate::Link| link.session.audio_ready();
            if !send_when_ready(&shared, live, ready, |link| link.session.send_audio(timestamp, &pcm)) {
                break 'stream;
            }
            frames += (pcm.len() / frame_bytes) as u64;
        }
    }
    command::stop(&mut child);
}

/// the running --mic-cmd. dropping it closes its input and stops it
pub struct Microphone {
    child: Child,
    input: Option<ChildStdin>,
}

impl Microphone {
    pub fn start(command: &str) -> Option<Self> {
        match command::spawn(command, Stdio::null()) {
            Ok(mut child) => {
                let input = child.stdin.take();
                input.iter().for_each(command::never_block);
                Some(Self { child, input })
            }
            Err(e) => {
                eprintln!("microphone: cannot run {command}: {e}");
                None
            }
        }
    }

    /// hand recorded pcm to the command. pieces it has no room for are dropped
    pub fn write(&mut self, pcm: &[u8]) {
        for piece in pcm.chunks(PIPE_BUF) {
            let Some(input) = self.input.as_mut() else { return };
            match input.write(piece) {
                Err(e) if e.kind() != ErrorKind::WouldBlock => self.input = None,
                _ => {}
            }
        }
    }
}

impl Drop for Microphone {
    fn drop(&mut self) {
        self.input = None;
        command::finish(&mut self.child);
    }
}
