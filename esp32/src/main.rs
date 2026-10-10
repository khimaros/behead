//! android auto server for the esp32-p4: the chip is the phone, on its high
//! speed usb port, and shows the demo, encoded with its h.264 encoder. the
//! session, the connection and what the demo draws are the linux server's.

#![no_main]

mod encoder;
mod tls;
mod usb;

use aap::{text, AudioFormat, Event, Options, Session, VideoMode};
use behead_demo::{apply, render, Packed, State, Tone};
use behead_link::send_when_ready;
use encoder::{Buffer, Encoder};
use std::sync::mpsc::{channel, sync_channel, Receiver, SyncSender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// the phone certificate and key, as pem. `make esp32-certs` puts them there
const CERTIFICATE: &str = include_str!("../certs/phone.crt");
const KEY: &str = include_str!("../certs/phone.key");
/// pause between sessions, so a dying link is not retried in a tight loop
const RETRY_DELAY: Duration = Duration::from_millis(100);
/// the tallest picture tried: 30 of these a second take the encoder 16 ms
/// each. 1080 lines are not whole macroblocks, which the encoder wants
const MAX_HEIGHT: u32 = 720;
/// pictures in the encoder's form: one being drawn, and one being encoded
const SPARES: usize = 2;
/// length of each packet of sound, as the linux server sends them
const AUDIO_PACKET_MS: usize = 40;
/// what the tone is made in
const TONE_BITS: u32 = 16;
const MICROS_PER_SECOND: u64 = 1_000_000;

/// what the firmware keeps for one headunit connection
struct Host {
    /// bumped whenever the video stream stops, so stale encoder threads exit
    video_epoch: u64,
    /// the same for the sound, and the thread playing the tone
    audio_epoch: u64,
    /// what the demo shows, which input changes and the encoder thread draws
    state: Arc<Mutex<State>>,
}

type Link = behead_link::Link<tls::MbedTls, Host>;
type Shared = behead_link::Shared<tls::MbedTls, Host>;

/// held by the thread that has the chip's one encoder
static ENCODER: Mutex<()> = Mutex::new(());

/// a picture, and how long after the stream started it was drawn
type Stamped<T> = (T, Duration);

/// draw the demo at the mode's rate, straight into the encoder's yuv, in
/// buffers handed back once they are encoded, until nobody takes the
/// pictures any more
fn draw(mode: VideoMode, state: Arc<Mutex<State>>, spare: Receiver<Buffer>, drawn: SyncSender<Stamped<Buffer>>) {
    let (started, period) = (Instant::now(), Duration::from_secs(1) / mode.fps.max(1));
    let mut due = started;
    for (frame, mut yuv) in spare.into_iter().enumerate() {
        let mut picture = Packed::new(mode.width, mode.height, yuv.bytes());
        render(&mut picture, &state.lock().unwrap(), mode.fps, frame as u64);
        if drawn.send((yuv, started.elapsed())).is_err() {
            break;
        }
        // a frame that came late is not made up for with a burst
        due = (due + period).max(Instant::now());
        std::thread::sleep(due.saturating_duration_since(Instant::now()));
    }
}

/// draw and encode the demo until the stream stops. the chip has two cores:
/// one draws a picture while the encoder is at the one before
fn stream(shared: Shared, epoch: u64, mode: VideoMode, state: Arc<Mutex<State>>) -> Result<(), String> {
    let _only = ENCODER.lock().unwrap();
    let mut encoder = Encoder::new(mode)?;
    let ((spare_yuv, yuvs), (drawn, frames)) = (channel(), sync_channel(0));
    for _ in 0..SPARES {
        drop(spare_yuv.send(Buffer::yuv(mode)?));
    }
    std::thread::spawn(move || draw(mode, state, yuvs, drawn));
    for (yuv, elapsed) in frames {
        let unit = encoder.encode(&yuv, elapsed.as_millis() as u32)?;
        let (live, ready) = (|link: &Link| link.host.video_epoch == epoch, |link: &Link| link.session.video_ready());
        if !send_when_ready(&shared, live, ready, |link| link.session.send_video(elapsed.as_micros() as u64, unit)) {
            break;
        }
        drop(spare_yuv.send(yuv));
    }
    Ok(())
}

/// play the demo's test tone as the car's media sound until the stream
/// stops, a packet at a time and no faster than it is heard
fn play(shared: Shared, epoch: u64, format: AudioFormat) {
    if format.bits != TONE_BITS {
        return log::warn!("audio: the tone is {TONE_BITS} bit, and the headunit wants {}", format.bits);
    }
    let (started, mut tone, rate) = (Instant::now(), Tone::default(), format.rate.max(1) as u64);
    let packet = rate as usize * AUDIO_PACKET_MS / 1000;
    for frames in (0..).step_by(packet) {
        let (pcm, timestamp) = (tone.pcm(format.rate, format.channels, packet), frames * MICROS_PER_SECOND / rate);
        let (live, ready) = (|link: &Link| link.host.audio_epoch == epoch, |link: &Link| link.session.audio_ready());
        if !send_when_ready(&shared, live, ready, |link| link.session.send_audio(timestamp, &pcm)) {
            break;
        }
        let due = started + Duration::from_micros(timestamp);
        std::thread::sleep(due.saturating_duration_since(Instant::now()));
    }
}

/// act on session events. returns false when the headunit asked to shut down.
fn handle(shared: &Shared, host: &mut Host, event: Event) -> bool {
    let show = |lines: Vec<String>| lines.iter().for_each(|line| apply(&mut host.state.lock().unwrap(), line));
    match event {
        Event::Authenticated => log::info!("headunit authenticated"),
        Event::Discovered(info) => {
            log::info!("headunit offers {} channels", info.channels.len());
            text::describe_inputs(&info).iter().for_each(|line| log::info!("{line}"));
        }
        Event::VideoStarted(mode) => {
            log::info!("video started {}x{}@{}", mode.width, mode.height, mode.fps);
            let (shared, epoch, state) = (shared.clone(), host.video_epoch, host.state.clone());
            std::thread::spawn(move || stream(shared, epoch, mode, state).unwrap_or_else(|e| log::error!("{e}")));
        }
        Event::VideoStopped => host.video_epoch += 1,
        Event::AudioStarted(format) => {
            log::info!("audio started {}hz {} bit {} channels", format.rate, format.bits, format.channels);
            let (shared, epoch) = (shared.clone(), host.audio_epoch);
            std::thread::spawn(move || play(shared, epoch, format));
        }
        Event::AudioStopped => host.audio_epoch += 1,
        Event::Input(input) => show(text::input_lines(&input)),
        Event::Unhandled { channel, id, body } => show(vec![text::unknown_message(channel, id, &body)]),
        Event::UnknownField { channel, kind, field, value } => {
            show(vec![text::unknown_field(kind, channel, field, &value)])
        }
        Event::Shutdown => return false,
        // the demo hears nothing, and draws nothing from the car's sensors
        _ => {}
    }
    true
}

/// drive one headunit connection to completion
fn serve(reader: usb::FromHost, writer: usb::ToHost) -> Result<(), String> {
    let options = Options { media_audio: true, max_video_height: Some(MAX_HEIGHT), ..Options::default() };
    let session = Session::new(tls::MbedTls::new(CERTIFICATE, KEY)?, options);
    let host = Host { video_epoch: 0, audio_epoch: 0, state: Arc::default() };
    let shared: Shared = behead_link::open(session, writer, host);
    behead_link::serve(&shared, reader, |link, event| handle(&shared, &mut link.host, event))
}

/// where esp-idf starts the program. esp-idf-sys has this too, as `binstart`,
/// but there it first moves the c standard streams, and on this esp-idf they
/// end up on descriptors that reach nothing, which silences the log
#[no_mangle]
extern "C" fn app_main() {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();
    if let Err(e) = usb::install() {
        return log::error!("{e}");
    }
    log::info!("usb device ready");
    loop {
        let (reader, writer) = usb::accept();
        log::info!("headunit connected");
        match serve(reader, writer) {
            Ok(()) => log::info!("headunit disconnected"),
            Err(e) => log::warn!("session ended: {e}"),
        }
        std::thread::sleep(RETRY_DELAY);
    }
}
