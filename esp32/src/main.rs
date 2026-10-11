//! android auto server for the esp32-p4: the chip is the phone, on its high
//! speed usb port, and shows the launcher, encoded with its h.264 encoder,
//! with the music of a microsd card as the car's sound. the session, the
//! connection and the launcher are the linux programs'.

#![no_main]

mod card;
mod encoder;
mod tls;
mod usb;

use aap::{text, AudioFormat, Event, Options, Session, VideoMode};
use behead_demo::Packed;
use behead_launcher::{App, Player, Source, MIN_SIZE};
use behead_link::send_when_ready;
use encoder::{Buffer, Encoder};
use esp_idf_svc::sys;
use std::path::Path;
use std::sync::mpsc::{channel, sync_channel, Receiver, Sender, SyncSender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// the phone certificate and key, as pem. `make certs-esp32` puts them there
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
/// what the music is decoded to
const AUDIO_BITS: u32 = 16;
const MICROS_PER_SECOND: u64 = 1_000_000;
/// stack of the thread that reads the card and decodes music, which the
/// filesystem and the decoders need more of than a thread gets otherwise.
/// the opus decoder works on its stack: on a laptop it runs in 100 KB and
/// overflows in less, and here it overflowed 64
const MUSIC_STACK: usize = 160 * 1024;

/// what the firmware keeps for one headunit connection
struct Host {
    /// bumped whenever the video stream stops, so stale encoder threads exit
    video_epoch: u64,
    /// the same for the sound, and the thread sending it
    audio_epoch: u64,
    stage: Stage,
}

/// what outlives a connection: a car that comes back finds the screen and
/// the track it left
#[derive(Clone)]
struct Stage {
    /// which input changes and the drawing thread draws
    app: Arc<Mutex<App>>,
    /// where a car that wants sound is handed to the music thread
    listeners: Sender<Listener>,
}

/// a car's media sound, once it has started: the link to send it on, a
/// number that is the link's while that sound has not stopped, and its form
type Listener = (Shared, u64, AudioFormat);

type Link = behead_link::Link<tls::MbedTls, Host>;
type Shared = behead_link::Shared<tls::MbedTls, Host>;

/// held by the thread that has the chip's one encoder
static ENCODER: Mutex<()> = Mutex::new(());

/// a picture, and how long after the stream started it was drawn
type Stamped<T> = (T, Duration);

/// a picture in the encoder's form, and what the launcher said of the one
/// drawn in it last
struct Picture {
    yuv: Buffer,
    shows: Option<u64>,
}

/// draw the launcher at the mode's rate, straight into the encoder's yuv,
/// in buffers handed back once they are encoded, until nobody takes the
/// pictures any more. a buffer that holds the picture already is left as it
/// is: the launcher is still most of the time, and a picture drawn pushes
/// the music decoder out of the cache
fn draw(mode: VideoMode, app: Arc<Mutex<App>>, spare: Receiver<Picture>, drawn: SyncSender<Stamped<Picture>>) {
    let (started, period) = (Instant::now(), Duration::from_secs(1) / mode.fps.max(1));
    let mut due = started;
    for (frame, mut picture) in spare.into_iter().enumerate() {
        let app = app.lock().unwrap();
        let shows = Some(app.shows(frame as u64));
        if picture.shows != shows {
            app.render(&mut Packed::new(mode.width, mode.height, picture.yuv.bytes()), mode.fps, frame as u64);
            picture.shows = shows;
        }
        drop(app);
        if drawn.send((picture, started.elapsed())).is_err() {
            break;
        }
        // a frame that came late is not made up for with a burst
        due = (due + period).max(Instant::now());
        std::thread::sleep(due.saturating_duration_since(Instant::now()));
    }
}

/// draw and encode the launcher until the stream stops. the chip has two
/// cores: one draws a picture while the encoder is at the one before
fn stream(shared: Shared, epoch: u64, mode: VideoMode, app: Arc<Mutex<App>>) -> Result<(), String> {
    let _only = ENCODER.lock().unwrap();
    let mut encoder = Encoder::new(mode)?;
    let ((spare_yuv, yuvs), (drawn, frames)) = (channel(), sync_channel(0));
    for _ in 0..SPARES {
        drop(spare_yuv.send(Picture { yuv: Buffer::yuv(mode)?, shows: None }));
    }
    std::thread::spawn(move || draw(mode, app, yuvs, drawn));
    for (picture, elapsed) in frames {
        let unit = encoder.encode(&picture.yuv, elapsed.as_millis() as u32)?;
        let (live, ready) = (|link: &Link| link.host.video_epoch == epoch, |link: &Link| link.session.video_ready());
        if !send_when_ready(&shared, live, ready, |link| link.session.send_video(elapsed.as_micros() as u64, unit)) {
            break;
        }
        drop(spare_yuv.send(picture));
    }
    Ok(())
}

/// send what the player plays as the car's media sound until the stream
/// stops, a packet at a time and no faster than it is heard
fn play((shared, epoch, format): Listener, source: &mut Source) {
    if format.bits != AUDIO_BITS {
        return log::warn!("audio: music is {AUDIO_BITS} bit, and the headunit wants {}", format.bits);
    }
    let (started, rate) = (Instant::now(), format.rate.max(1) as u64);
    let packet = rate as usize * AUDIO_PACKET_MS / 1000;
    for frames in (0..).step_by(packet) {
        let pcm = source.pcm(format.rate, format.channels, packet);
        let timestamp = frames * MICROS_PER_SECOND / rate;
        let (live, ready) = (|link: &Link| link.host.audio_epoch == epoch, |link: &Link| link.session.audio_ready());
        if !send_when_ready(&shared, live, ready, |link| link.session.send_audio(timestamp, &pcm)) {
            break;
        }
        let due = started + Duration::from_micros(timestamp);
        std::thread::sleep(due.saturating_duration_since(Instant::now()));
    }
}

/// find the card's music, then play it to one car after another. this is
/// the one thread with a stack for the decoders, made when the firmware
/// starts and kept: the chip's own memory, where stacks are, is soon in too
/// many pieces to hold another that large, and a thread made for each car
/// got its stack once
fn music(player: Player, listeners: Receiver<Listener>) {
    if let Err(e) = card::mount() {
        log::warn!("no card: {e}");
    }
    player.scan(Path::new(card::ROOT));
    log::info!("card: {} tracks", player.tracks().len());
    let mut source = player.source();
    listeners.into_iter().for_each(|listener| play(listener, &mut source));
}

/// act on session events. returns false when the headunit asked to shut down.
fn handle(shared: &Shared, host: &mut Host, event: Event) -> bool {
    let apply = |lines: Vec<String>| lines.iter().for_each(|line| host.stage.app.lock().unwrap().apply(line));
    match event {
        Event::Authenticated => log::info!("headunit authenticated"),
        Event::Discovered(info) => {
            log::info!("headunit offers {} channels", info.channels.len());
            text::describe_inputs(&info).iter().for_each(|line| log::info!("{line}"));
        }
        Event::VideoStarted(mode) => {
            log::info!("video started {}x{}@{}", mode.width, mode.height, mode.fps);
            let (shared, epoch, app) = (shared.clone(), host.video_epoch, host.stage.app.clone());
            app.lock().unwrap().resize(mode.width, mode.height);
            std::thread::spawn(move || stream(shared, epoch, mode, app).unwrap_or_else(|e| log::error!("{e}")));
        }
        Event::VideoStopped => host.video_epoch += 1,
        Event::AudioStarted(format) => {
            log::info!("audio started {}hz {} bit {} channels", format.rate, format.bits, format.channels);
            drop(host.stage.listeners.send((shared.clone(), host.audio_epoch, format)));
        }
        Event::AudioStopped => host.audio_epoch += 1,
        Event::Input(input) => apply(text::input_lines(&input)),
        Event::Unhandled { channel, id, body } => apply(vec![text::unknown_message(channel, id, &body)]),
        Event::UnknownField { channel, kind, field, value } => {
            apply(vec![text::unknown_field(kind, channel, field, &value)])
        }
        Event::Shutdown => return false,
        // the launcher hears nothing, and draws nothing from the car's sensors
        _ => {}
    }
    true
}

/// drive one headunit connection to completion
fn serve(reader: usb::FromHost, writer: usb::ToHost, stage: &Stage) -> Result<(), String> {
    let options = Options { media_audio: true, max_video_height: Some(MAX_HEIGHT), ..Options::default() };
    let session = Session::new(tls::MbedTls::new(CERTIFICATE, KEY)?, options);
    let host = Host { video_epoch: 0, audio_epoch: 0, stage: stage.clone() };
    let shared: Shared = behead_link::open(session, writer, host);
    behead_link::serve(&shared, reader, |link, event| handle(&shared, &mut link.host, event))
}

/// the launcher, with the card's music being found behind it: a full card
/// takes a while to read, and the car should not wait for its picture
fn open_stage() -> Stage {
    let (player, (listeners, waiting)) = (Player::default(), channel());
    let finder = player.clone();
    // SAFETY: here and below, plain settings for the threads this one makes
    // next, which esp-idf copies
    let usual = unsafe { sys::esp_pthread_get_default_config() };
    let roomy = sys::esp_pthread_cfg_t { stack_alloc_caps: sys::MALLOC_CAP_SPIRAM | sys::MALLOC_CAP_8BIT, ..usual };
    // the music thread's stack goes in the psram: the chip's own memory
    // has no room for it beside what the encoder needs for 720 lines
    unsafe { sys::esp_pthread_set_cfg(&roomy) };
    let thread = std::thread::Builder::new().name("music".into()).stack_size(MUSIC_STACK);
    if let Err(e) = thread.spawn(move || music(finder, waiting)) {
        log::error!("music: {e}");
    }
    // the threads made for each car go back to stacks in the chip's memory
    unsafe { sys::esp_pthread_set_cfg(&usual) };
    Stage { app: Arc::new(Mutex::new(App::new(player, MIN_SIZE.0, MIN_SIZE.1))), listeners }
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
    let stage = open_stage();
    loop {
        let (reader, writer) = usb::accept();
        log::info!("headunit connected");
        match serve(reader, writer, &stage) {
            Ok(()) => log::info!("headunit disconnected"),
            Err(e) => log::warn!("session ended: {e}"),
        }
        std::thread::sleep(RETRY_DELAY);
    }
}
