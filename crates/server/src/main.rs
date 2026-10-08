//! android auto server: presents this machine to a car headunit as a phone,
//! streaming h.264 from a child process and reporting touch input on stdout.

mod audio;
mod command;
mod gadget;
mod nmea;
mod sensors;
mod tls;
mod uinput;
mod video;
mod x509;

use aap::{text, Event, Options, Session, VideoMode};
use behead_link::send_when_ready;
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

const USAGE: &str =
    "usage: behead (--cert FILE --key FILE | --cert-dir DIR)... --video-cmd CMD [--audio-cmd CMD] [--mic-cmd CMD]
              [--audio-delay MS] [--usb UDC | --listen ADDR] [--uinput] [--knob KEYS]
              [--nmea-socket PATH]
       behead teardown

  --cert FILE      phone certificate, pem
  --key FILE       phone private key, pem, for the --cert before it
  --cert-dir DIR   every NAME.crt in DIR with its NAME.key, in name order.
                   when the headunit refuses a certificate, its next
                   connection gets the next one given
  --video-cmd CMD  shell command writing annex-b h.264 to stdout.
                   {width}, {height} and {fps} are replaced with the negotiated mode
  --audio-cmd CMD  shell command writing signed little endian pcm to stdout, in real
                   time, played as the headunit's media audio. {rate}, {bits} and
                   {channels} are replaced with the negotiated format
  --audio-delay MS  hold media audio back this many milliseconds, for when the
                   sound runs ahead of the picture
  --mic-cmd CMD    shell command reading the headunit microphone as pcm on stdin,
                   with the same replacements
  --usb UDC       appear to the headunit as a usb device on this controller,
                   or on the first one found when UDC is 'auto'. needs root
  --listen ADDR    tcp address to accept the headunit on (default 127.0.0.1:5277)
  --uinput         also deliver touch and buttons as kernel input devices,
                   'behead touchscreen' and 'behead keys'. needs /dev/uinput
  --knob KEYS      what a turn of the car's rotary knob types with --uinput:
                   'focus' for tab and shift+tab (default), 'arrows' for
                   down and up
  --nmea-socket PATH  offer the car's location as nmea sentences on a unix
                   socket, for a location service such as geoclue

  teardown         remove a usb gadget left behind by an earlier run";
const DEFAULT_LISTEN: &str = "127.0.0.1:5277";
const TEARDOWN_COMMAND: &str = "teardown";
const AUTO_UDC: &str = "auto";
const CERT_EXTENSION: &str = "crt";
const KEY_EXTENSION: &str = "key";
const UNPAIRED_CERT: &str = "--cert needs a --key right after it";
const READ_BUFFER: usize = 64 * 1024;
/// pause between usb sessions, so a dying link is not retried in a tight loop
const RETRY_DELAY: Duration = Duration::from_millis(100);
enum Transport {
    Tcp(String),
    Usb(String),
}

/// where one or more phone certificates come from, in order of preference
enum Credential {
    Pair(PathBuf, PathBuf),
    Dir(PathBuf),
}

struct Config {
    transport: Transport,
    credentials: Vec<Credential>,
    video_cmd: String,
    audio_cmd: Option<String>,
    audio_delay: Duration,
    mic_cmd: Option<String>,
    uinput: bool,
    knob: uinput::Knob,
    nmea_socket: Option<PathBuf>,
}

/// what this server keeps for one headunit connection
#[derive(Default)]
struct Host {
    /// bumped whenever the video stream stops, so stale encoder threads exit
    video_epoch: u64,
    /// the video command's stdin, which receives the same input lines as stdout
    video_input: Option<File>,
    /// bumped whenever media audio stops, so stale audio threads exit
    audio_epoch: u64,
    /// kernel input devices for this headunit, with --uinput
    input_devices: Option<uinput::Devices>,
    /// the --mic-cmd process, while the headunit microphone is open
    microphone: Option<audio::Microphone>,
    /// the latest sensor line of each kind, by its first word
    readings: BTreeMap<String, String>,
}

type Link = behead_link::Link<tls::RustlsTls, Host>;
type Shared = behead_link::Shared<tls::RustlsTls, Host>;

fn parse_args(mut args: impl Iterator<Item = String>) -> Result<Config, String> {
    let (mut transport, mut credentials, mut cert, mut video_cmd, mut uinput) =
        (Transport::Tcp(DEFAULT_LISTEN.into()), Vec::new(), None, None, false);
    let (mut audio_cmd, mut mic_cmd, mut nmea_socket, mut audio_delay) = (None, None, None, Duration::ZERO);
    let mut knob = uinput::Knob::default();
    while let Some(flag) = args.next() {
        if cert.is_some() && flag != "--key" {
            return Err(UNPAIRED_CERT.into());
        }
        if flag == "--uinput" {
            uinput = true;
            continue;
        }
        let value = args.next().ok_or(format!("missing value for {flag}"))?;
        match flag.as_str() {
            "--listen" => transport = Transport::Tcp(value),
            "--usb" => transport = Transport::Usb(value),
            "--cert" => cert = Some(PathBuf::from(value)),
            "--key" => {
                let cert = cert.take().ok_or("--key needs a --cert before it")?;
                credentials.push(Credential::Pair(cert, value.into()))
            }
            "--cert-dir" => credentials.push(Credential::Dir(value.into())),
            "--video-cmd" => video_cmd = Some(value),
            // a service unit passes these from its environment, where one
            // left unset comes out empty
            "--audio-cmd" => audio_cmd = Some(value).filter(|command| !command.is_empty()),
            "--mic-cmd" => mic_cmd = Some(value).filter(|command| !command.is_empty()),
            "--audio-delay" if value.is_empty() => {}
            "--audio-delay" => {
                let ms = value.parse().map_err(|_| format!("--audio-delay takes milliseconds, not {value}"))?;
                audio_delay = Duration::from_millis(ms)
            }
            "--knob" if value.is_empty() => {}
            "--knob" => knob = value.parse()?,
            "--nmea-socket" => nmea_socket = Some(PathBuf::from(value)),
            _ => return Err(format!("unknown flag {flag}")),
        }
    }
    if cert.is_some() {
        return Err(UNPAIRED_CERT.into());
    }
    if credentials.is_empty() {
        return Err("--cert and --key, or --cert-dir, is required".into());
    }
    let video_cmd = video_cmd.ok_or("--video-cmd is required")?;
    Ok(Config { transport, credentials, video_cmd, audio_cmd, audio_delay, mic_cmd, uinput, knob, nmea_socket })
}

/// every certificate file in a directory with its key, in name order
fn dir_credentials(dir: &Path) -> Result<Vec<(PathBuf, PathBuf)>, String> {
    let entries = std::fs::read_dir(dir).map_err(|e| format!("reading {}: {e}", dir.display()))?;
    let mut certs: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|extension| extension == CERT_EXTENSION))
        .collect();
    certs.sort();
    certs
        .into_iter()
        .map(|cert| match cert.with_extension(KEY_EXTENSION) {
            key if key.is_file() => Ok((cert, key)),
            _ => Err(format!("no key for {}", cert.display())),
        })
        .collect()
}

/// the certificate and key files named on the command line, in order
fn credential_files(credentials: &[Credential]) -> Result<Vec<(PathBuf, PathBuf)>, String> {
    let mut pairs = Vec::new();
    for credential in credentials {
        match credential {
            Credential::Pair(cert, key) => pairs.push((cert.clone(), key.clone())),
            Credential::Dir(dir) => pairs.extend(dir_credentials(dir)?),
        }
    }
    Ok(pairs)
}

/// kernel input devices matching the headunit's input services: the keys of
/// all of them, and the first touchscreen. a failure is logged rather than
/// fatal, since input still reaches stdout
fn create_input_devices(info: &aap::proto::ServiceDiscoveryResponse, knob: uinput::Knob) -> Option<uinput::Devices> {
    let inputs: Vec<_> = info.channels.iter().filter_map(|channel| channel.input_channel.as_ref()).collect();
    let merged = aap::proto::InputChannel {
        supported_keycodes: inputs.iter().flat_map(|input| input.supported_keycodes.clone()).collect(),
        touch_screen_config: inputs.iter().find_map(|input| input.touch_screen_config.clone()),
        touch_pad_config: None,
    };
    (!inputs.is_empty()).then(|| uinput::Devices::create(&merged, knob).inspect_err(|e| eprintln!("{e}")).ok())?
}

/// print input to stdout, inject it with --uinput, and hand it to the video command
fn deliver_input(host: &mut Host, event: &aap::proto::InputEvent) {
    if let Some(Err(e)) = host.input_devices.as_mut().map(|devices| devices.deliver(event)) {
        eprintln!("{e}");
    }
    deliver_lines(host, text::input_lines(event));
}

/// hand one line to the video command, if it is running
fn write_to_video(host: &mut Host, line: &str) {
    let Some(input) = host.video_input.as_mut() else { return };
    // lines are far below PIPE_BUF, so each write is all or nothing
    match input.write(format!("{line}\n").as_bytes()) {
        Err(e) if e.kind() != std::io::ErrorKind::WouldBlock => host.video_input = None,
        _ => {}
    }
}

/// print input and sensor lines to stdout and hand them to the video command
fn deliver_lines(host: &mut Host, lines: Vec<String>) {
    for line in lines {
        println!("{line}");
        write_to_video(host, &line);
    }
}

/// deliver sensor lines, and keep the latest of each kind for a video
/// command that starts later: a headunit reports a state such as night mode
/// once, when asked, which is before it shows the phone
fn deliver_readings(host: &mut Host, lines: Vec<String>) {
    for line in &lines {
        let kind = line.split(' ').next().unwrap_or_default().to_string();
        host.readings.insert(kind, line.clone());
    }
    deliver_lines(host, lines);
}

/// show the video command's pictures on this link, starting the command
/// unless the one already running serves this mode
fn start_video(shared: &Shared, host: &mut Host, config: &Config, video: &mut Option<video::Source>, mode: VideoMode) {
    if !video.as_ref().is_some_and(|source| source.serves(mode)) {
        let values = [("width", mode.width), ("height", mode.height), ("fps", mode.fps)];
        let command = command::fill(&config.video_cmd, &values);
        // the old command stops before the new one starts
        *video = None;
        *video = video::Source::start(&command, mode).inspect_err(|e| eprintln!("{e}")).ok();
    }
    host.video_input = video.as_ref().and_then(|source| source.attach(shared.clone(), host.video_epoch));
    for line in host.readings.clone().into_values() {
        write_to_video(host, &line);
    }
}

/// act on session events. returns false when the headunit asked to shut down.
fn handle(shared: &Shared, host: &mut Host, config: &Config, video: &mut Option<video::Source>, event: Event) -> bool {
    match event {
        Event::Authenticated => eprintln!("headunit authenticated"),
        Event::Discovered(info) => {
            eprintln!("headunit offers {} channels", info.channels.len());
            text::describe_inputs(&info).iter().for_each(|line| eprintln!("{line}"));
            if config.uinput {
                host.input_devices = create_input_devices(&info, config.knob);
            }
        }
        Event::VideoStarted(mode) => {
            eprintln!("video started {}x{}@{}", mode.width, mode.height, mode.fps);
            start_video(shared, host, config, video, mode);
        }
        Event::VideoStopped => {
            host.video_epoch += 1;
            host.video_input = None;
            video.iter().for_each(video::Source::detach);
        }
        Event::AudioStarted(format) => {
            eprintln!("audio started {}hz {} bit {} channels", format.rate, format.bits, format.channels);
            let command = command::fill(config.audio_cmd.as_deref().unwrap_or_default(), &audio::values(format));
            let (shared, epoch) = (shared.clone(), host.audio_epoch);
            let delay = config.audio_delay;
            std::thread::spawn(move || audio::stream(shared, epoch, command, format, delay));
        }
        Event::AudioStopped => host.audio_epoch += 1,
        Event::MicrophoneOpened(format) => {
            eprintln!("microphone open {}hz {} bit {} channels", format.rate, format.bits, format.channels);
            let command = command::fill(config.mic_cmd.as_deref().unwrap_or_default(), &audio::values(format));
            host.microphone = audio::Microphone::start(&command);
        }
        Event::Microphone(pcm) => {
            if let Some(microphone) = host.microphone.as_mut() {
                microphone.write(&pcm);
            }
        }
        Event::Input(input) => deliver_input(host, &input),
        Event::Sensors(readings) => {
            readings.location.iter().for_each(nmea::publish);
            deliver_readings(host, sensors::lines(&readings));
        }
        Event::Unhandled { channel, id, body } => {
            eprintln!("unhandled message {id:#06x} on channel {channel}");
            deliver_lines(host, vec![text::unknown_message(channel, id, &body)]);
        }
        Event::UnknownField { channel, kind, field, value } => {
            eprintln!("unknown field {field} in a {kind} event on channel {channel}");
            deliver_lines(host, vec![text::unknown_field(kind, channel, field, &value)]);
        }
        Event::Shutdown => return false,
    }
    true
}

/// drive one headunit connection to completion, moving on to the next
/// certificate if the headunit refused the one presented. the video command
/// outlives the connection
fn serve(
    reader: impl Read,
    writer: impl Write + Send + 'static,
    config: &Config,
    credentials: &mut tls::Credentials,
    video: &mut Option<video::Source>,
) -> Result<(), String> {
    let options = Options { media_audio: config.audio_cmd.is_some(), microphone: config.mic_cmd.is_some() };
    let session = Session::new(credentials.endpoint()?, options);
    let shared: Shared = behead_link::open(session, writer, Host::default());
    let result =
        behead_link::serve(&shared, reader, |link, event| handle(&shared, &mut link.host, config, video, event));
    let mut link = shared.0.lock().unwrap();
    // encoder threads may hold the link a while longer; the devices and the
    // microphone command go now
    link.host.input_devices = None;
    link.host.microphone = None;
    video.iter().for_each(video::Source::detach);
    if link.session.tls().refused() {
        credentials.advance();
    }
    result
}

fn report(result: Result<(), String>) {
    match result {
        Ok(()) => eprintln!("headunit disconnected"),
        Err(e) => eprintln!("session ended: {e}"),
    }
}

fn run_tcp(address: &str, config: &Config, mut credentials: tls::Credentials) -> Result<(), String> {
    let listener = TcpListener::bind(address).map_err(|e| format!("listen {address}: {e}"))?;
    eprintln!("listening on {}", listener.local_addr().map_err(|e| e.to_string())?);
    let mut video = None;
    for stream in listener.incoming() {
        let stream = stream.map_err(|e| format!("accept: {e}"))?;
        // small writes, such as acks and the pictures of a still screen,
        // should not wait for nagle's algorithm
        stream.set_nodelay(true).map_err(|e| format!("nodelay: {e}"))?;
        let writer = stream.try_clone().map_err(|e| format!("clone: {e}"))?;
        eprintln!("headunit connected");
        report(serve(stream, writer, config, &mut credentials, &mut video));
    }
    Ok(())
}

/// on SIGINT or SIGTERM, replace this process with `behead teardown`.
/// exec closes every endpoint file at once, which the kernel requires
/// before the gadget can be removed, and gives cleanup a single code path.
fn teardown_on_signal() {
    // SAFETY: the set is initialised by sigemptyset before use, and blocking
    // signals before any thread is spawned makes every thread inherit the mask
    let set = unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        libc::sigaddset(&mut set, libc::SIGINT);
        libc::sigaddset(&mut set, libc::SIGTERM);
        libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
        set
    };
    std::thread::spawn(move || {
        let mut signal = 0;
        // SAFETY: both pointers refer to live locals
        unsafe { libc::sigwait(&set, &mut signal) };
        let error = Command::new("/proc/self/exe").arg(TEARDOWN_COMMAND).exec();
        eprintln!("cannot run teardown: {error}");
        std::process::exit(1);
    });
}

fn run_usb(udc: &str, config: &Config, mut credentials: tls::Credentials) -> Result<(), String> {
    teardown_on_signal();
    let gadget = gadget::Gadget::create(Some(udc).filter(|&name| name != AUTO_UDC))?;
    eprintln!("usb gadget ready");
    let mut video = None;
    loop {
        let (reader, writer) = gadget.accept()?;
        eprintln!("headunit connected");
        report(serve(reader, writer, config, &mut credentials, &mut video));
        std::thread::sleep(RETRY_DELAY);
    }
}

fn run() -> Result<(), String> {
    if std::env::args().nth(1).as_deref() == Some(TEARDOWN_COMMAND) {
        return gadget::teardown();
    }
    let config = parse_args(std::env::args().skip(1)).map_err(|e| format!("{e}\n{USAGE}"))?;
    let credentials = tls::Credentials::load(&credential_files(&config.credentials)?)?;
    config.nmea_socket.as_deref().map_or(Ok(()), nmea::listen)?;
    match &config.transport {
        Transport::Tcp(address) => run_tcp(address, &config, credentials),
        Transport::Usb(udc) => run_usb(udc, &config, credentials),
    }
}

fn main() {
    if let Err(e) = run() {
        eprintln!("{e}");
        std::process::exit(1);
    }
}
