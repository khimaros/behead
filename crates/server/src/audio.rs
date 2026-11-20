//! media audio from --audio-cmd to the headunit, and the headunit
//! microphone to --mic-cmd

use crate::{command, send_when_ready, Shared};
use aap::{AudioFormat, Event};
use std::collections::VecDeque;
use std::io::{ErrorKind, Read, Write};
use std::process::{Child, ChildStdin, ChildStdout, Stdio};
use std::time::{Duration, Instant};

/// length of each media audio packet, close to what openauto expects
const PACKET_MS: usize = 40;
/// silence this long releases audio focus so the car's radio can resume
const SILENCE_MS: usize = 2000;
const SILENCE_PACKETS: usize = SILENCE_MS / PACKET_MS;
const SILENCE_TIMEOUT: Duration = Duration::from_millis(SILENCE_MS as u64);
/// how often an idle reader checks whether the headunit has left
const POLL_INTERVAL: Duration = Duration::from_millis(200);
const MICROS_PER_SECOND: u64 = 1_000_000;
/// pipe writes up to this size are all or nothing, so a dropped piece of
/// microphone audio never splits a sample
const PIPE_BUF: usize = 4096;

/// the replacements for an audio command template
pub fn values(format: AudioFormat) -> [(&'static str, u32); 3] {
    [("rate", format.rate), ("bits", format.bits), ("channels", format.channels)]
}

fn is_silent(pcm: &[u8]) -> bool {
    pcm.iter().all(|&byte| byte == 0)
}

fn is_live(shared: &Shared, epoch: u64) -> bool {
    let link = shared.0.lock().unwrap();
    !link.closed && link.host.audio_epoch == epoch
}

/// request or release media audio focus for this stream
fn set_focus(shared: &Shared, epoch: u64, want: bool) -> bool {
    let mut link = shared.0.lock().unwrap();
    if link.closed || link.host.audio_epoch != epoch {
        return false;
    }
    if want {
        let _ = link.session.request_audio_focus();
    } else if let Ok(events) = link.session.release_audio_focus() {
        if events.contains(&Event::AudioStopped) {
            eprintln!("audio stopped");
        }
    }
    let ok = link.flush().is_ok();
    shared.1.notify_all();
    ok
}

enum ReadStatus {
    Packet,
    Quiet,
    Eof,
}

/// read one full packet from the audio command, checking periodically whether
/// the headunit is still connected, and returning Quiet if an active stream
/// writes nothing for `quiet_limit`
fn read_packet(
    stdout: &mut ChildStdout,
    packet: &mut [u8],
    filled: &mut usize,
    shared: &Shared,
    epoch: u64,
    quiet_limit: Option<Duration>,
) -> ReadStatus {
    let started = Instant::now();
    while *filled < packet.len() {
        if !is_live(shared, epoch) {
            return ReadStatus::Eof;
        }
        if quiet_limit.is_some_and(|limit| started.elapsed() >= limit) {
            return ReadStatus::Quiet;
        }
        let wait =
            quiet_limit.map_or(POLL_INTERVAL, |limit| POLL_INTERVAL.min(limit.saturating_sub(started.elapsed())));
        if !command::readable_within(stdout, wait) {
            continue;
        }
        match stdout.read(&mut packet[*filled..]) {
            Ok(0) | Err(_) => return ReadStatus::Eof,
            Ok(n) => *filled += n,
        }
    }
    *filled = 0;
    ReadStatus::Packet
}

/// run the audio command and send its pcm while it is not silent: audio focus
/// is claimed once sound starts and released after sustained silence so the
/// car's own radio keeps playing otherwise. packets are timestamped by the
/// samples sent before them, and each waits `delay` after it was read.
pub fn stream(shared: Shared, epoch: u64, command: String, format: AudioFormat, delay: Duration) {
    let mut child = match command::spawn(&command, Stdio::piped()) {
        Ok(child) => child,
        Err(e) => return eprintln!("audio: cannot run {command}: {e}"),
    };
    let mut stdout = child.stdout.take().unwrap();
    let frame_bytes = (format.bits as usize / 8 * format.channels as usize).max(1);
    let mut packet = vec![0; format.rate as usize * PACKET_MS / 1000 * frame_bytes];
    let (mut filled, mut active, mut silent, mut frames) = (0, false, 0, 0u64);
    let mut held: VecDeque<(Instant, Vec<u8>)> = VecDeque::new();
    'stream: loop {
        match read_packet(&mut stdout, &mut packet, &mut filled, &shared, epoch, active.then_some(SILENCE_TIMEOUT)) {
            ReadStatus::Eof => break 'stream,
            ReadStatus::Quiet => {
                (active, silent, frames) = (false, 0, 0);
                held.clear();
                set_focus(&shared, epoch, false);
                continue;
            }
            ReadStatus::Packet => {}
        }
        if !active {
            if is_silent(&packet) {
                continue;
            }
            (active, silent, frames) = (true, 0, 0);
            if !set_focus(&shared, epoch, true) {
                break 'stream;
            }
        }
        held.push_back((Instant::now(), packet.clone()));
        while held.front().is_some_and(|(read, _)| read.elapsed() >= delay) {
            let (_, pcm) = held.pop_front().unwrap();
            silent = if is_silent(&pcm) { silent + 1 } else { 0 };
            if silent >= SILENCE_PACKETS {
                (active, silent, frames) = (false, 0, 0);
                held.clear();
                set_focus(&shared, epoch, false);
                break;
            }
            let timestamp = frames * MICROS_PER_SECOND / format.rate.max(1) as u64;
            let live = |link: &crate::Link| link.host.audio_epoch == epoch && link.session.audio_active();
            let ready = |link: &crate::Link| link.session.audio_ready();
            if send_when_ready(&shared, live, ready, |link| link.session.send_audio(timestamp, &pcm)) {
                frames += (pcm.len() / frame_bytes) as u64;
            } else if !is_live(&shared, epoch) {
                break 'stream;
            }
        }
    }
    if active {
        set_focus(&shared, epoch, false);
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

    /// whether the command is gone, or no longer reads what it is handed
    pub fn ended(&mut self) -> bool {
        self.input.is_none() || !matches!(self.child.try_wait(), Ok(None))
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
