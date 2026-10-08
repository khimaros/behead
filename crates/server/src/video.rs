//! the --video-cmd process. it outlives video focus and connections, so a
//! desktop session keeps running while the headunit shows its own screen or
//! reconnects; only a new mode, or the command exiting, starts another.

use crate::{command, send_when_ready, Link, Shared, READ_BUFFER};
use aap::h264::{self, AccessUnitSplitter};
use aap::VideoMode;
use std::fs::File;
use std::io::Read;
use std::os::fd::OwnedFd;
use std::process::{Child, ChildStdout, Stdio};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// a pause in the encoder's output this long ends the picture before it.
/// encoders write a picture in one go, so it only has to cover pipe reads
const ENCODER_QUIET: Duration = Duration::from_millis(10);

/// the link pictures go to while the headunit shows them
#[derive(Default)]
struct Target {
    link: Option<(Shared, u64)>,
    /// a headunit can only pick up a stream it missed part of at a keyframe.
    /// one that sees the command's output from the start needs none
    needs_keyframe: bool,
    sent_any: bool,
    /// the parameter sets the command wrote last
    config: Vec<u8>,
}

pub struct Source {
    mode: VideoMode,
    child: Child,
    /// the command's stdin, which receives the same input lines as stdout
    input: File,
    target: Arc<Mutex<Target>>,
    reader: JoinHandle<()>,
}

impl Source {
    pub fn start(command: &str, mode: VideoMode) -> Result<Self, String> {
        let mut child = command::spawn(command, Stdio::piped()).map_err(|e| format!("video: {command}: {e}"))?;
        let stdout = child.stdout.take().unwrap();
        let input = File::from(OwnedFd::from(child.stdin.take().unwrap()));
        command::never_block(&input);
        let target = Arc::new(Mutex::new(Target::default()));
        let reader = std::thread::spawn({
            let target = target.clone();
            move || forward(stdout, &target)
        });
        Ok(Self { mode, child, input, target, reader })
    }

    /// whether this source can serve the mode, without a restart
    pub fn serves(&self, mode: VideoMode) -> bool {
        self.mode == mode && !self.reader.is_finished()
    }

    /// send pictures to this link from the next keyframe on. returns where
    /// to write the input lines for the command
    pub fn attach(&self, shared: Shared, epoch: u64) -> Option<File> {
        let mut target = self.target.lock().unwrap();
        target.link = Some((shared, epoch));
        target.needs_keyframe |= target.sent_any;
        self.input.try_clone().ok()
    }

    /// keep the command running, but let its pictures go nowhere
    pub fn detach(&self) {
        self.target.lock().unwrap().link = None;
    }
}

impl Drop for Source {
    fn drop(&mut self) {
        command::stop(&mut self.child);
    }
}

/// where a picture goes now, and in what form: to the attached link, unless
/// it is waiting for a keyframe. x264 puts the parameter sets in front of
/// every keyframe of a raw stream. a hardware encoder writes them once, at
/// the start, so a keyframe without them gets the last ones seen
fn destination(target: &Mutex<Target>, unit: Vec<u8>) -> Option<((Shared, u64), Vec<u8>)> {
    let mut target = target.lock().unwrap();
    let config = h264::split_config(&unit).0;
    let has_config = !config.is_empty();
    if has_config {
        target.config = config.to_vec();
    }
    let Some(link) = target.link.clone() else {
        target.needs_keyframe = true;
        return None;
    };
    if target.needs_keyframe && !has_config && !h264::is_keyframe(&unit) {
        return None;
    }
    let resumes = std::mem::replace(&mut target.needs_keyframe, false);
    target.sent_any = true;
    let unit = if resumes && !has_config { [&target.config[..], &unit].concat() } else { unit };
    Some((link, unit))
}

/// read access units until the command exits, sending each to whichever
/// link is attached and dropping it otherwise, so the command never blocks
fn forward(mut stdout: ChildStdout, target: &Mutex<Target>) {
    let (mut splitter, mut buffer, started) = (AccessUnitSplitter::default(), vec![0; READ_BUFFER], Instant::now());
    loop {
        // an encoder goes quiet while the screen is still, so a buffered
        // picture cannot wait for the next one to show where it ends
        let quiet = splitter.has_picture() && !command::readable_within(&stdout, ENCODER_QUIET);
        let (units, eof) = match quiet {
            true => (splitter.idle().into_iter().collect(), false),
            false => match stdout.read(&mut buffer) {
                Ok(0) | Err(_) => (splitter.finish().into_iter().collect::<Vec<_>>(), true),
                Ok(n) => (splitter.push(&buffer[..n]), false),
            },
        };
        for unit in units {
            let Some(((shared, epoch), unit)) = destination(target, unit) else { continue };
            let (live, ready) =
                (|link: &Link| link.host.video_epoch == epoch, |link: &Link| link.session.video_ready());
            let timestamp = started.elapsed().as_micros() as u64;
            send_when_ready(&shared, live, ready, |link| link.session.send_video(timestamp, &unit));
        }
        if eof {
            return;
        }
    }
}
