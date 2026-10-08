//! android auto server for the esp32-p4: the chip is the phone, on its high
//! speed usb port, and shows the demo, encoded with its h.264 encoder. the
//! session, the connection and what the demo draws are the linux server's.

mod encoder;
mod tls;
mod usb;

use aap::{text, Event, Options, Session, VideoMode};
use behead_demo::{apply, render, Canvas, State};
use behead_link::send_when_ready;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// the phone certificate and key, as pem. `make esp32-certs` puts them there
const CERTIFICATE: &str = include_str!("../certs/phone.crt");
const KEY: &str = include_str!("../certs/phone.key");
/// pause between sessions, so a dying link is not retried in a tight loop
const RETRY_DELAY: Duration = Duration::from_millis(100);

/// what the firmware keeps for one headunit connection
struct Host {
    /// bumped whenever the video stream stops, so stale encoder threads exit
    video_epoch: u64,
    /// what the demo shows, which input changes and the encoder thread draws
    state: Arc<Mutex<State>>,
}

type Link = behead_link::Link<tls::MbedTls, Host>;
type Shared = behead_link::Shared<tls::MbedTls, Host>;

/// held by the thread that has the chip's one encoder
static ENCODER: Mutex<()> = Mutex::new(());

/// draw and encode the demo at the mode's rate until the stream stops
fn stream(shared: Shared, epoch: u64, mode: VideoMode, state: Arc<Mutex<State>>) -> Result<(), String> {
    let _only = ENCODER.lock().unwrap();
    let (mut encoder, mut canvas) = (encoder::Encoder::new(mode)?, Canvas::new(mode.width, mode.height));
    let (started, period) = (Instant::now(), Duration::from_secs(1) / mode.fps.max(1));
    let mut due = started;
    for frame in 0.. {
        render(&mut canvas, &state.lock().unwrap(), mode.fps, frame);
        let elapsed = started.elapsed();
        let unit = encoder.encode(&canvas, elapsed.as_millis() as u32)?;
        let (live, ready) = (|link: &Link| link.host.video_epoch == epoch, |link: &Link| link.session.video_ready());
        if !send_when_ready(&shared, live, ready, |link| link.session.send_video(elapsed.as_micros() as u64, unit)) {
            break;
        }
        // a frame that came late is not made up for with a burst
        due = (due + period).max(Instant::now());
        std::thread::sleep(due.saturating_duration_since(Instant::now()));
    }
    Ok(())
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
        Event::Input(input) => show(text::input_lines(&input)),
        Event::Unhandled { channel, id, body } => show(vec![text::unknown_message(channel, id, &body)]),
        Event::UnknownField { channel, kind, field, value } => {
            show(vec![text::unknown_field(kind, channel, field, &value)])
        }
        Event::Shutdown => return false,
        // the demo has no sound, and draws nothing from the car's sensors
        _ => {}
    }
    true
}

/// drive one headunit connection to completion
fn serve(reader: usb::FromHost, writer: usb::ToHost) -> Result<(), String> {
    let session = Session::new(tls::MbedTls::new(CERTIFICATE, KEY)?, Options::default());
    let host = Host { video_epoch: 0, state: Arc::default() };
    let shared: Shared = behead_link::open(session, writer, host);
    behead_link::serve(&shared, reader, |link, event| handle(&shared, &mut link.host, event))
}

fn main() {
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
