//! phone side session state machine. sans-io: feed received bytes to
//! `receive`, write whatever `take_output` returns back to the transport.

use crate::frame::{self, Frame, FrameReader};
use crate::h264;
use crate::proto::{self, av, control, input, sensor};
use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use prost::Message;

const CONTROL_CHANNEL: u8 = 0;
const PROTOCOL_MAJOR: u16 = 1;
const PROTOCOL_MINOR: u16 = 5;
const VERSION_MATCH: u16 = 0;
const DEVICE_NAME: &str = "behead";
const DEVICE_BRAND: &str = "khimaros";
const MESSAGE_ID_LEN: usize = 2;
const TIMESTAMP_LEN: usize = 8;
/// microphone packets the headunit may send before the phone acks one
const MICROPHONE_MAX_UNACKED: i32 = 1;

/// resolutions indexed by the codec resolution enum, which starts at 1
const RESOLUTIONS: [(u32, u32); 9] = [
    (800, 480),
    (1280, 720),
    (1920, 1080),
    (2560, 1440),
    (3840, 2160),
    (720, 1280),
    (1080, 1920),
    (1440, 2560),
    (2160, 3840),
];
/// google's enum has 60 = 1 and 30 = 2. aasdk and AACS name them the other way round.
const FPS_60: i32 = 1;
/// assumed for any field a headunit leaves out of an audio config
const DEFAULT_AUDIO: AudioFormat = AudioFormat { rate: 48000, bits: 16, channels: 2 };

#[derive(Debug)]
pub enum Error {
    Tls(String),
    Protocol(&'static str),
}

/// memory-bio style tls endpoint, so the core stays free of io and of any
/// particular tls library. the phone is the tls server.
pub trait Tls {
    /// consume handshake bytes from the peer, return bytes to send back
    fn handshake(&mut self, input: &[u8]) -> Result<Vec<u8>, Error>;
    fn encrypt(&mut self, plain: &[u8]) -> Result<Vec<u8>, Error>;
    fn decrypt(&mut self, cipher: &[u8]) -> Result<Vec<u8>, Error>;
}

/// streams the phone has a source or sink for. only these are opened:
/// opening media audio takes audio focus from the car's own sound.
#[derive(Debug, Clone, Copy, Default)]
pub struct Options {
    pub media_audio: bool,
    pub microphone: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VideoMode {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
}

/// signed little endian pcm
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AudioFormat {
    pub rate: u32,
    pub bits: u32,
    pub channels: u32,
}

#[derive(Debug, PartialEq)]
pub enum Event {
    Authenticated,
    Discovered(proto::ServiceDiscoveryResponse),
    VideoStarted(VideoMode),
    VideoStopped,
    AudioStarted(AudioFormat),
    AudioStopped,
    MicrophoneOpened(AudioFormat),
    /// pcm recorded by the headunit microphone
    Microphone(Vec<u8>),
    Input(proto::InputEvent),
    /// readings from the headunit's sensors
    Sensors(proto::SensorEvent),
    Shutdown,
    Unhandled {
        channel: u8,
        id: u16,
    },
}

/// what video and media audio share: setup, focus and the ack window. the
/// stream runs while it is both set up and focused.
#[derive(Default)]
struct Flow {
    channel: u8,
    config: u32,
    max_unacked: u32,
    unacked: u32,
    setup: bool,
    focused: bool,
    started: bool,
}

impl Flow {
    fn ready(&self) -> bool {
        self.started && self.unacked < self.max_unacked
    }

    fn set_up(&mut self, body: &[u8]) -> Result<(), Error> {
        let response: proto::AvSetupResponse = decode(body)?;
        if response.media_status != Some(proto::SETUP_STATUS_OK) {
            return Err(Error::Protocol("headunit refused media setup"));
        }
        self.max_unacked = response.max_unacked.unwrap_or(1).max(1);
        self.config = response.configs.first().copied().unwrap_or(0);
        self.setup = true;
        Ok(())
    }

    fn acked(&mut self, body: &[u8]) -> Result<(), Error> {
        let ack: proto::AvMediaAck = decode(body)?;
        self.unacked = self.unacked.saturating_sub(ack.value.unwrap_or(1));
        Ok(())
    }

    /// Some(true) when the stream should start now, Some(false) when it should stop
    fn toggle(&mut self) -> Option<bool> {
        let want = self.setup && self.focused;
        if want == self.started {
            return None;
        }
        (self.started, self.unacked) = (want, 0);
        Some(want)
    }
}

#[derive(Default)]
struct Video {
    flow: Flow,
    configs: Vec<proto::VideoConfig>,
    sent_config: Vec<u8>,
}

struct Audio {
    flow: Flow,
    configs: Vec<proto::AudioConfig>,
}

struct Microphone {
    channel: u8,
    format: AudioFormat,
}

/// the headunit's sensor channel, and the sensors on it this phone reads
struct Sensors {
    channel: u8,
    types: Vec<i32>,
}

struct Input {
    channel: u8,
    keycodes: Vec<u32>,
}

pub struct Session<T: Tls> {
    tls: T,
    options: Options,
    reader: FrameReader,
    partial: BTreeMap<u8, Vec<u8>>,
    out: Vec<u8>,
    video: Option<Video>,
    media: Option<Audio>,
    microphone: Option<Microphone>,
    input: Option<Input>,
    sensors: Option<Sensors>,
}

fn mode_of(config: &proto::VideoConfig) -> VideoMode {
    let index = (config.video_resolution.unwrap_or(1).max(1) as usize - 1).min(RESOLUTIONS.len() - 1);
    let (width, height) = RESOLUTIONS[index];
    VideoMode {
        width: width - config.margin_width.unwrap_or(0),
        height: height - config.margin_height.unwrap_or(0),
        fps: if config.video_fps == Some(FPS_60) { 60 } else { 30 },
    }
}

fn format_of(config: &proto::AudioConfig) -> AudioFormat {
    AudioFormat {
        rate: config.sample_rate.unwrap_or(DEFAULT_AUDIO.rate),
        bits: config.bit_depth.unwrap_or(DEFAULT_AUDIO.bits),
        channels: config.channel_count.unwrap_or(DEFAULT_AUDIO.channels),
    }
}

fn decode<M: Message + Default>(body: &[u8]) -> Result<M, Error> {
    M::decode(body).map_err(|_| Error::Protocol("malformed protobuf"))
}

impl<T: Tls> Session<T> {
    pub fn new(tls: T, options: Options) -> Self {
        Self {
            tls,
            options,
            reader: FrameReader::default(),
            partial: BTreeMap::new(),
            out: Vec::new(),
            video: None,
            media: None,
            microphone: None,
            input: None,
            sensors: None,
        }
    }

    /// bytes queued for the transport. must be written in the order returned.
    pub fn take_output(&mut self) -> Vec<u8> {
        core::mem::take(&mut self.out)
    }

    pub fn tls(&self) -> &T {
        &self.tls
    }

    /// true when a video frame may be sent without exceeding the ack window
    pub fn video_ready(&self) -> bool {
        self.video.as_ref().is_some_and(|v| v.flow.ready())
    }

    /// true when a media audio packet may be sent without exceeding the ack window
    pub fn audio_ready(&self) -> bool {
        self.media.as_ref().is_some_and(|m| m.flow.ready())
    }

    pub fn receive(&mut self, data: &[u8]) -> Result<Vec<Event>, Error> {
        self.reader.push(data);
        let mut events = Vec::new();
        while let Some(frame) = self.reader.next_frame() {
            if let Some(message) = self.reassemble(frame)? {
                let (channel, flags, plain) = message;
                if plain.len() < MESSAGE_ID_LEN {
                    return Err(Error::Protocol("message too short"));
                }
                let id = u16::from_be_bytes([plain[0], plain[1]]);
                self.dispatch(channel, flags, id, &plain[MESSAGE_ID_LEN..], &mut events)?;
            }
        }
        Ok(events)
    }

    /// queue one h.264 access unit. parameter sets go out once, untimestamped,
    /// the way a phone delivers codec config.
    pub fn send_video(&mut self, timestamp_us: u64, unit: &[u8]) -> Result<(), Error> {
        let Some(video) = self.video.as_mut().filter(|v| v.flow.started) else {
            return Err(Error::Protocol("video not started"));
        };
        let channel = video.flow.channel;
        let (config, picture) = h264::split_config(unit);
        if !config.is_empty() && config != video.sent_config {
            video.sent_config = config.to_vec();
            self.send(channel, 0, av::MEDIA, config, true)?;
        }
        if picture.is_empty() {
            return Ok(());
        }
        let body = [&timestamp_us.to_be_bytes(), picture].concat();
        self.send(channel, 0, av::MEDIA_WITH_TIMESTAMP, &body, true)?;
        self.video.as_mut().unwrap().flow.unacked += 1;
        Ok(())
    }

    /// queue one packet of media audio pcm, in the format AudioStarted gave
    pub fn send_audio(&mut self, timestamp_us: u64, pcm: &[u8]) -> Result<(), Error> {
        let Some(media) = self.media.as_mut().filter(|m| m.flow.started) else {
            return Err(Error::Protocol("audio not started"));
        };
        media.flow.unacked += 1;
        let channel = media.flow.channel;
        let body = [&timestamp_us.to_be_bytes(), pcm].concat();
        self.send(channel, 0, av::MEDIA_WITH_TIMESTAMP, &body, true)
    }

    fn reassemble(&mut self, frame: Frame) -> Result<Option<(u8, u8, Vec<u8>)>, Error> {
        let Frame { channel, flags, payload } = frame;
        let chunk = if flags & frame::FLAG_ENCRYPTED != 0 { self.tls.decrypt(&payload)? } else { payload };
        if flags & frame::FLAG_FIRST != 0 {
            self.partial.insert(channel, chunk);
        } else {
            self.partial.get_mut(&channel).ok_or(Error::Protocol("continuation without first frame"))?.extend(chunk);
        }
        Ok(if flags & frame::FLAG_LAST != 0 {
            self.partial.remove(&channel).map(|m| (channel, flags, m))
        } else {
            None
        })
    }

    fn send(&mut self, channel: u8, flags: u8, id: u16, body: &[u8], encrypted: bool) -> Result<(), Error> {
        let plain = [&id.to_be_bytes(), body].concat();
        if !encrypted {
            frame::encode_frame(channel, flags | frame::FLAG_BULK, plain.len() as u32, &plain, &mut self.out);
            return Ok(());
        }
        let chunks = plain.chunks(frame::MAX_FRAME_PAYLOAD);
        let last = chunks.len() - 1;
        for (i, chunk) in chunks.enumerate() {
            let position = if i == 0 { frame::FLAG_FIRST } else { 0 } | if i == last { frame::FLAG_LAST } else { 0 };
            let cipher = self.tls.encrypt(chunk)?;
            frame::encode_frame(
                channel,
                flags | position | frame::FLAG_ENCRYPTED,
                plain.len() as u32,
                &cipher,
                &mut self.out,
            );
        }
        Ok(())
    }

    fn send_proto(&mut self, channel: u8, flags: u8, id: u16, message: &impl Message) -> Result<(), Error> {
        self.send(channel, flags, id, &message.encode_to_vec(), true)
    }

    fn dispatch(&mut self, channel: u8, flags: u8, id: u16, body: &[u8], events: &mut Vec<Event>) -> Result<(), Error> {
        if channel == CONTROL_CHANNEL || flags & frame::FLAG_CONTROL != 0 {
            return self.on_control(channel, id, body, events);
        }
        if self.video.as_ref().is_some_and(|v| v.flow.channel == channel) {
            return self.on_video(id, body, events);
        }
        if self.media.as_ref().is_some_and(|m| m.flow.channel == channel) {
            return self.on_media(id, body, events);
        }
        if self.microphone.as_ref().is_some_and(|m| m.channel == channel) {
            return self.on_microphone(id, body, events);
        }
        if self.sensors.as_ref().is_some_and(|s| s.channel == channel) {
            match id {
                sensor::EVENT => events.push(Event::Sensors(decode(body)?)),
                sensor::START_RESPONSE => {}
                _ => events.push(Event::Unhandled { channel, id }),
            }
            return Ok(());
        }
        match id {
            input::EVENT if self.input.as_ref().is_some_and(|i| i.channel == channel) => {
                events.push(Event::Input(decode(body)?))
            }
            input::BINDING_RESPONSE => {}
            _ => events.push(Event::Unhandled { channel, id }),
        }
        Ok(())
    }

    fn on_control(&mut self, channel: u8, id: u16, body: &[u8], events: &mut Vec<Event>) -> Result<(), Error> {
        match id {
            control::VERSION_REQUEST => {
                let reply = [PROTOCOL_MAJOR, PROTOCOL_MINOR, VERSION_MATCH].map(u16::to_be_bytes).concat();
                self.send(CONTROL_CHANNEL, 0, control::VERSION_RESPONSE, &reply, false)?;
            }
            control::SSL_HANDSHAKE => {
                let reply = self.tls.handshake(body)?;
                if !reply.is_empty() {
                    self.send(CONTROL_CHANNEL, 0, control::SSL_HANDSHAKE, &reply, false)?;
                }
            }
            control::AUTH_COMPLETE => {
                let request = proto::ServiceDiscoveryRequest {
                    device_name: Some(DEVICE_NAME.to_string()),
                    device_brand: Some(DEVICE_BRAND.to_string()),
                };
                self.send_proto(CONTROL_CHANNEL, 0, control::SERVICE_DISCOVERY_REQUEST, &request)?;
                events.push(Event::Authenticated);
            }
            control::SERVICE_DISCOVERY_RESPONSE => {
                let response: proto::ServiceDiscoveryResponse = decode(body)?;
                self.open_channels(&response)?;
                events.push(Event::Discovered(response));
            }
            control::CHANNEL_OPEN_RESPONSE => self.on_channel_open(channel, decode(body)?)?,
            control::PING_REQUEST => {
                let ping: proto::Ping = decode(body)?;
                self.send_proto(CONTROL_CHANNEL, 0, control::PING_RESPONSE, &ping)?;
            }
            control::SHUTDOWN_REQUEST => {
                self.send(CONTROL_CHANNEL, 0, control::SHUTDOWN_RESPONSE, &[], true)?;
                events.push(Event::Shutdown);
            }
            // the answer to our request, or the headunit taking focus back unprompted
            control::AUDIO_FOCUS_RESPONSE => {
                let response: proto::AudioFocusResponse = decode(body)?;
                if let Some(media) = self.media.as_mut() {
                    media.flow.focused = proto::AUDIO_FOCUS_PLAYABLE.contains(&response.state.unwrap_or(0));
                    self.sync_media(events)?;
                }
            }
            _ => events.push(Event::Unhandled { channel, id }),
        }
        Ok(())
    }

    /// request the advertised channels this phone has a use for
    fn open_channels(&mut self, response: &proto::ServiceDiscoveryResponse) -> Result<(), Error> {
        for descriptor in &response.channels {
            let channel = descriptor.channel_id.unwrap_or(0) as u8;
            let av = descriptor.av_channel.as_ref();
            let is_media = |av: &&proto::AvChannel| {
                av.stream_type == Some(proto::STREAM_TYPE_AUDIO) && av.audio_type == Some(proto::AUDIO_TYPE_MEDIA)
            };
            if let Some(av) = av.filter(|av| av.stream_type == Some(proto::STREAM_TYPE_VIDEO)) {
                let flow = Flow { channel, ..Flow::default() };
                self.video = Some(Video { flow, configs: av.video_configs.clone(), ..Video::default() });
            } else if let Some(av) = av.filter(is_media).filter(|_| self.options.media_audio && self.media.is_none()) {
                let flow = Flow { channel, ..Flow::default() };
                self.media = Some(Audio { flow, configs: av.audio_configs.clone() });
            } else if let Some(input) = &descriptor.input_channel {
                self.input = Some(Input { channel, keycodes: input.supported_keycodes.clone() });
            } else if let Some(source) =
                descriptor.av_input_channel.as_ref().filter(|_| self.options.microphone && self.microphone.is_none())
            {
                let format = format_of(&source.audio_config.unwrap_or_default());
                self.microphone = Some(Microphone { channel, format });
            } else if let Some(offered) = descriptor.sensor_channel.as_ref().filter(|_| self.sensors.is_none()) {
                let offered = offered.sensors.iter().filter_map(|sensor| sensor.sensor_type);
                let types = offered.filter(|sensor| proto::SENSOR_TYPES.contains(sensor)).collect();
                self.sensors = Some(Sensors { channel, types });
            } else {
                continue;
            }
            let request = proto::ChannelOpenRequest { priority: Some(0), channel_id: Some(channel as i32) };
            self.send_proto(channel, frame::FLAG_CONTROL, control::CHANNEL_OPEN_REQUEST, &request)?;
        }
        Ok(())
    }

    fn on_channel_open(&mut self, channel: u8, response: proto::StatusResponse) -> Result<(), Error> {
        if response.status != Some(proto::STATUS_OK) {
            return Err(Error::Protocol("headunit refused channel open"));
        }
        let audio = self.media.as_ref().is_some_and(|m| m.flow.channel == channel)
            || self.microphone.as_ref().is_some_and(|m| m.channel == channel);
        if self.video.as_ref().is_some_and(|v| v.flow.channel == channel) {
            let request = proto::AvSetupRequest { codec: Some(proto::CODEC_H264_BP) };
            self.send_proto(channel, 0, av::SETUP_REQUEST, &request)?;
        } else if audio {
            self.send_proto(channel, 0, av::SETUP_REQUEST, &proto::AvSetupRequest { codec: Some(proto::CODEC_PCM) })?;
        } else if let Some(keycodes) = self.input.as_ref().filter(|i| i.channel == channel).map(|i| &i.keycodes) {
            let request = proto::BindingRequest { scan_codes: keycodes.iter().map(|&k| k as i32).collect() };
            self.send_proto(channel, 0, input::BINDING_REQUEST, &request)?;
        } else if let Some(types) = self.sensors.as_ref().filter(|s| s.channel == channel).map(|s| s.types.clone()) {
            // the headunit sends nothing from a sensor until asked for it
            for sensor_type in types {
                let request = proto::SensorStartRequest { sensor_type: Some(sensor_type), refresh_interval: Some(0) };
                self.send_proto(channel, 0, sensor::START_REQUEST, &request)?;
            }
        }
        Ok(())
    }

    fn on_video(&mut self, id: u16, body: &[u8], events: &mut Vec<Event>) -> Result<(), Error> {
        let video = self.video.as_mut().unwrap();
        match id {
            av::SETUP_RESPONSE => {
                video.flow.set_up(body)?;
                // the headunit stays on its native screen until the phone asks to project
                if !video.flow.focused {
                    let (channel, request) =
                        (video.flow.channel, proto::VideoFocusRequest { mode: Some(proto::VIDEO_FOCUSED) });
                    self.send_proto(channel, 0, av::VIDEO_FOCUS_REQUEST, &request)?;
                }
            }
            av::VIDEO_FOCUS_INDICATION => {
                let indication: proto::VideoFocusIndication = decode(body)?;
                video.flow.focused = indication.focus_mode == Some(proto::VIDEO_FOCUSED);
            }
            av::MEDIA_ACK => return video.flow.acked(body),
            _ => {
                events.push(Event::Unhandled { channel: video.flow.channel, id });
                return Ok(());
            }
        }
        self.sync_video(events)
    }

    /// start or stop the stream so it tracks setup and focus state
    fn sync_video(&mut self, events: &mut Vec<Event>) -> Result<(), Error> {
        let video = self.video.as_mut().unwrap();
        let channel = video.flow.channel;
        match video.flow.toggle() {
            None => Ok(()),
            Some(false) => {
                events.push(Event::VideoStopped);
                self.send(channel, 0, av::STOP_INDICATION, &[], true)
            }
            Some(true) => {
                video.sent_config.clear();
                let config =
                    video.configs.get(video.flow.config as usize).ok_or(Error::Protocol("unknown video config"))?;
                events.push(Event::VideoStarted(mode_of(config)));
                let start = proto::AvStartIndication { session: Some(0), config: Some(video.flow.config) };
                self.send_proto(channel, 0, av::START_INDICATION, &start)
            }
        }
    }

    fn on_media(&mut self, id: u16, body: &[u8], events: &mut Vec<Event>) -> Result<(), Error> {
        let media = self.media.as_mut().unwrap();
        match id {
            // focus is the phone's to request, once the channel can play
            av::SETUP_RESPONSE => {
                media.flow.set_up(body)?;
                let request = proto::AudioFocusRequest { focus_type: Some(proto::AUDIO_FOCUS_GAIN) };
                self.send_proto(CONTROL_CHANNEL, 0, control::AUDIO_FOCUS_REQUEST, &request)
            }
            av::MEDIA_ACK => media.flow.acked(body),
            _ => {
                events.push(Event::Unhandled { channel: media.flow.channel, id });
                Ok(())
            }
        }
    }

    /// start or stop media audio so it tracks setup and focus state
    fn sync_media(&mut self, events: &mut Vec<Event>) -> Result<(), Error> {
        let media = self.media.as_mut().unwrap();
        let channel = media.flow.channel;
        match media.flow.toggle() {
            None => Ok(()),
            Some(false) => {
                events.push(Event::AudioStopped);
                self.send(channel, 0, av::STOP_INDICATION, &[], true)
            }
            Some(true) => {
                let config =
                    media.configs.get(media.flow.config as usize).ok_or(Error::Protocol("unknown audio config"))?;
                events.push(Event::AudioStarted(format_of(config)));
                let start = proto::AvStartIndication { session: Some(0), config: Some(media.flow.config) };
                self.send_proto(channel, 0, av::START_INDICATION, &start)
            }
        }
    }

    /// the headunit microphone: opened once set up, each packet acked as it arrives
    fn on_microphone(&mut self, id: u16, body: &[u8], events: &mut Vec<Event>) -> Result<(), Error> {
        let Microphone { channel, format } = *self.microphone.as_ref().unwrap();
        match id {
            av::SETUP_RESPONSE => {
                let response: proto::AvSetupResponse = decode(body)?;
                if response.media_status != Some(proto::SETUP_STATUS_OK) {
                    return Err(Error::Protocol("headunit refused microphone setup"));
                }
                let request = proto::MicrophoneRequest {
                    open: Some(true),
                    noise_cancellation: Some(false),
                    echo_cancellation: Some(false),
                    max_unacked: Some(MICROPHONE_MAX_UNACKED),
                };
                self.send_proto(channel, 0, av::MICROPHONE_REQUEST, &request)?;
                events.push(Event::MicrophoneOpened(format));
            }
            // sources disagree on its field order, and nothing here depends on it
            av::MICROPHONE_RESPONSE => {}
            av::MEDIA_WITH_TIMESTAMP => {
                let pcm = body.get(TIMESTAMP_LEN..).ok_or(Error::Protocol("microphone packet too short"))?;
                events.push(Event::Microphone(pcm.to_vec()));
                self.send_proto(channel, 0, av::MEDIA_ACK, &proto::AvMediaAck { session: Some(0), value: Some(1) })?;
            }
            _ => events.push(Event::Unhandled { channel, id }),
        }
        Ok(())
    }
}
