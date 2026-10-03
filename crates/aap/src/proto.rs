//! protobuf messages and message ids. every scalar is `optional` so zero
//! values are still written: headunits parse these as proto2 `required`.

use alloc::string::String;
use alloc::vec::Vec;
use prost::Message;

pub mod control {
    pub const VERSION_REQUEST: u16 = 0x0001;
    pub const VERSION_RESPONSE: u16 = 0x0002;
    pub const SSL_HANDSHAKE: u16 = 0x0003;
    pub const AUTH_COMPLETE: u16 = 0x0004;
    pub const SERVICE_DISCOVERY_REQUEST: u16 = 0x0005;
    pub const SERVICE_DISCOVERY_RESPONSE: u16 = 0x0006;
    pub const CHANNEL_OPEN_REQUEST: u16 = 0x0007;
    pub const CHANNEL_OPEN_RESPONSE: u16 = 0x0008;
    pub const PING_REQUEST: u16 = 0x000b;
    pub const PING_RESPONSE: u16 = 0x000c;
    pub const SHUTDOWN_REQUEST: u16 = 0x000f;
    pub const SHUTDOWN_RESPONSE: u16 = 0x0010;
    pub const AUDIO_FOCUS_REQUEST: u16 = 0x0012;
    pub const AUDIO_FOCUS_RESPONSE: u16 = 0x0013;
}

pub mod av {
    pub const MEDIA_WITH_TIMESTAMP: u16 = 0x0000;
    pub const MEDIA: u16 = 0x0001;
    pub const SETUP_REQUEST: u16 = 0x8000;
    pub const START_INDICATION: u16 = 0x8001;
    pub const STOP_INDICATION: u16 = 0x8002;
    pub const SETUP_RESPONSE: u16 = 0x8003;
    pub const MEDIA_ACK: u16 = 0x8004;
    pub const MICROPHONE_REQUEST: u16 = 0x8005;
    pub const MICROPHONE_RESPONSE: u16 = 0x8006;
    pub const VIDEO_FOCUS_REQUEST: u16 = 0x8007;
    pub const VIDEO_FOCUS_INDICATION: u16 = 0x8008;
}

pub mod input {
    pub const EVENT: u16 = 0x8001;
    pub const BINDING_REQUEST: u16 = 0x8002;
    pub const BINDING_RESPONSE: u16 = 0x8003;
}

pub mod sensor {
    pub const START_REQUEST: u16 = 0x8001;
    pub const START_RESPONSE: u16 = 0x8002;
    pub const EVENT: u16 = 0x8003;
}

pub const STATUS_OK: i32 = 0;
pub const STREAM_TYPE_AUDIO: i32 = 1;
pub const STREAM_TYPE_VIDEO: i32 = 3;
pub const AUDIO_TYPE_MEDIA: i32 = 3;
pub const CODEC_PCM: u32 = 1;
pub const CODEC_H264_BP: u32 = 3;
pub const AUDIO_FOCUS_GAIN: i32 = 1;
/// audio focus states the headunit grants. playing continues while ducked
/// (LOSS_TRANSIENT_CAN_DUCK), which is the headunit's to apply.
pub const AUDIO_FOCUS_PLAYABLE: [i32; 4] = [1, 2, 4, 6];
pub const SETUP_STATUS_OK: i32 = 2;
pub const VIDEO_FOCUSED: i32 = 1;
/// the sensor types `SensorEvent` can carry. each is also its field's tag:
/// location, compass, speed, rpm, odometer, fuel level, parking brake, gear,
/// night mode, environment and driving status
pub const SENSOR_TYPES: [i32; 11] = [1, 2, 3, 4, 5, 6, 7, 8, 10, 11, 13];

pub const TOUCH_DOWN: i32 = 0;
pub const TOUCH_UP: i32 = 1;
pub const TOUCH_MOVED: i32 = 2;
/// a further finger going down or lifting while others stay on the screen
pub const TOUCH_POINTER_DOWN: i32 = 5;
pub const TOUCH_POINTER_UP: i32 = 6;

#[derive(Clone, PartialEq, Message)]
pub struct ServiceDiscoveryRequest {
    #[prost(string, optional, tag = "4")]
    pub device_name: Option<String>,
    #[prost(string, optional, tag = "5")]
    pub device_brand: Option<String>,
}

#[derive(Clone, PartialEq, Message)]
pub struct ServiceDiscoveryResponse {
    #[prost(message, repeated, tag = "1")]
    pub channels: Vec<ChannelDescriptor>,
    #[prost(string, optional, tag = "2")]
    pub head_unit_name: Option<String>,
    #[prost(string, optional, tag = "3")]
    pub car_model: Option<String>,
}

#[derive(Clone, PartialEq, Message)]
pub struct ChannelDescriptor {
    #[prost(uint32, optional, tag = "1")]
    pub channel_id: Option<u32>,
    #[prost(message, optional, tag = "2")]
    pub sensor_channel: Option<SensorChannel>,
    #[prost(message, optional, tag = "3")]
    pub av_channel: Option<AvChannel>,
    #[prost(message, optional, tag = "4")]
    pub input_channel: Option<InputChannel>,
    #[prost(message, optional, tag = "5")]
    pub av_input_channel: Option<AvInputChannel>,
}

#[derive(Clone, PartialEq, Message)]
pub struct AvChannel {
    #[prost(int32, optional, tag = "1")]
    pub stream_type: Option<i32>,
    #[prost(int32, optional, tag = "2")]
    pub audio_type: Option<i32>,
    #[prost(message, repeated, tag = "3")]
    pub audio_configs: Vec<AudioConfig>,
    #[prost(message, repeated, tag = "4")]
    pub video_configs: Vec<VideoConfig>,
}

/// the headunit's microphone
#[derive(Clone, PartialEq, Message)]
pub struct AvInputChannel {
    #[prost(int32, optional, tag = "1")]
    pub stream_type: Option<i32>,
    #[prost(message, optional, tag = "2")]
    pub audio_config: Option<AudioConfig>,
}

#[derive(Clone, Copy, PartialEq, Message)]
pub struct AudioConfig {
    #[prost(uint32, optional, tag = "1")]
    pub sample_rate: Option<u32>,
    #[prost(uint32, optional, tag = "2")]
    pub bit_depth: Option<u32>,
    #[prost(uint32, optional, tag = "3")]
    pub channel_count: Option<u32>,
}

#[derive(Clone, PartialEq, Message)]
pub struct VideoConfig {
    #[prost(int32, optional, tag = "1")]
    pub video_resolution: Option<i32>,
    #[prost(int32, optional, tag = "2")]
    pub video_fps: Option<i32>,
    #[prost(uint32, optional, tag = "3")]
    pub margin_width: Option<u32>,
    #[prost(uint32, optional, tag = "4")]
    pub margin_height: Option<u32>,
    #[prost(uint32, optional, tag = "5")]
    pub dpi: Option<u32>,
}

#[derive(Clone, PartialEq, Message)]
pub struct InputChannel {
    #[prost(uint32, repeated, packed = "false", tag = "1")]
    pub supported_keycodes: Vec<u32>,
    #[prost(message, optional, tag = "2")]
    pub touch_screen_config: Option<TouchConfig>,
}

#[derive(Clone, PartialEq, Message)]
pub struct TouchConfig {
    #[prost(uint32, optional, tag = "1")]
    pub width: Option<u32>,
    #[prost(uint32, optional, tag = "2")]
    pub height: Option<u32>,
}

#[derive(Clone, PartialEq, Message)]
pub struct ChannelOpenRequest {
    #[prost(int32, optional, tag = "1")]
    pub priority: Option<i32>,
    #[prost(int32, optional, tag = "2")]
    pub channel_id: Option<i32>,
}

/// shared shape of channel open, binding and auth complete responses
#[derive(Clone, PartialEq, Message)]
pub struct StatusResponse {
    #[prost(int32, optional, tag = "1")]
    pub status: Option<i32>,
}

/// shared shape of ping request and response
#[derive(Clone, PartialEq, Message)]
pub struct Ping {
    #[prost(int64, optional, tag = "1")]
    pub timestamp: Option<i64>,
}

#[derive(Clone, PartialEq, Message)]
pub struct AvSetupRequest {
    #[prost(uint32, optional, tag = "1")]
    pub codec: Option<u32>,
}

#[derive(Clone, PartialEq, Message)]
pub struct AvSetupResponse {
    #[prost(int32, optional, tag = "1")]
    pub media_status: Option<i32>,
    #[prost(uint32, optional, tag = "2")]
    pub max_unacked: Option<u32>,
    #[prost(uint32, repeated, packed = "false", tag = "3")]
    pub configs: Vec<u32>,
}

#[derive(Clone, PartialEq, Message)]
pub struct AvStartIndication {
    #[prost(int32, optional, tag = "1")]
    pub session: Option<i32>,
    #[prost(uint32, optional, tag = "2")]
    pub config: Option<u32>,
}

#[derive(Clone, PartialEq, Message)]
pub struct AvMediaAck {
    #[prost(int32, optional, tag = "1")]
    pub session: Option<i32>,
    #[prost(uint32, optional, tag = "2")]
    pub value: Option<u32>,
}

#[derive(Clone, PartialEq, Message)]
pub struct AudioFocusRequest {
    #[prost(int32, optional, tag = "1")]
    pub focus_type: Option<i32>,
}

#[derive(Clone, PartialEq, Message)]
pub struct AudioFocusResponse {
    #[prost(int32, optional, tag = "1")]
    pub state: Option<i32>,
}

#[derive(Clone, PartialEq, Message)]
pub struct MicrophoneRequest {
    #[prost(bool, optional, tag = "1")]
    pub open: Option<bool>,
    #[prost(bool, optional, tag = "2")]
    pub noise_cancellation: Option<bool>,
    #[prost(bool, optional, tag = "3")]
    pub echo_cancellation: Option<bool>,
    #[prost(int32, optional, tag = "4")]
    pub max_unacked: Option<i32>,
}

#[derive(Clone, PartialEq, Message)]
pub struct VideoFocusRequest {
    #[prost(int32, optional, tag = "2")]
    pub mode: Option<i32>,
}

#[derive(Clone, PartialEq, Message)]
pub struct VideoFocusIndication {
    #[prost(int32, optional, tag = "1")]
    pub focus_mode: Option<i32>,
    #[prost(bool, optional, tag = "2")]
    pub unrequested: Option<bool>,
}

#[derive(Clone, PartialEq, Message)]
pub struct BindingRequest {
    #[prost(int32, repeated, packed = "false", tag = "1")]
    pub scan_codes: Vec<i32>,
}

#[derive(Clone, PartialEq, Message)]
pub struct InputEvent {
    #[prost(uint64, optional, tag = "1")]
    pub timestamp: Option<u64>,
    #[prost(message, optional, tag = "3")]
    pub touch_event: Option<TouchEvent>,
    #[prost(message, optional, tag = "4")]
    pub button_event: Option<ButtonEvents>,
}

#[derive(Clone, PartialEq, Message)]
pub struct TouchEvent {
    #[prost(message, repeated, tag = "1")]
    pub touch_location: Vec<TouchLocation>,
    #[prost(uint32, optional, tag = "2")]
    pub action_index: Option<u32>,
    #[prost(int32, optional, tag = "3")]
    pub touch_action: Option<i32>,
}

#[derive(Clone, PartialEq, Message)]
pub struct TouchLocation {
    #[prost(uint32, optional, tag = "1")]
    pub x: Option<u32>,
    #[prost(uint32, optional, tag = "2")]
    pub y: Option<u32>,
    #[prost(uint32, optional, tag = "3")]
    pub pointer_id: Option<u32>,
}

#[derive(Clone, PartialEq, Message)]
pub struct ButtonEvents {
    #[prost(message, repeated, tag = "1")]
    pub button_events: Vec<ButtonEvent>,
}

#[derive(Clone, PartialEq, Message)]
pub struct ButtonEvent {
    #[prost(uint32, optional, tag = "1")]
    pub scan_code: Option<u32>,
    #[prost(bool, optional, tag = "2")]
    pub is_pressed: Option<bool>,
}

#[derive(Clone, PartialEq, Message)]
pub struct SensorChannel {
    #[prost(message, repeated, tag = "1")]
    pub sensors: Vec<Sensor>,
}

#[derive(Clone, PartialEq, Message)]
pub struct Sensor {
    #[prost(int32, optional, tag = "1")]
    pub sensor_type: Option<i32>,
}

#[derive(Clone, PartialEq, Message)]
pub struct SensorStartRequest {
    #[prost(int32, optional, tag = "1")]
    pub sensor_type: Option<i32>,
    #[prost(int64, optional, tag = "2")]
    pub refresh_interval: Option<i64>,
}

/// a batch of readings. field names and numbers are aasdk's; the scales in
/// the comments are the ones openauto sends
#[derive(Clone, PartialEq, Message)]
pub struct SensorEvent {
    #[prost(message, repeated, tag = "1")]
    pub location: Vec<Location>,
    #[prost(message, repeated, tag = "2")]
    pub compass: Vec<Compass>,
    #[prost(message, repeated, tag = "3")]
    pub speed: Vec<Speed>,
    #[prost(message, repeated, tag = "4")]
    pub rpm: Vec<Rpm>,
    #[prost(message, repeated, tag = "5")]
    pub odometer: Vec<Odometer>,
    #[prost(message, repeated, tag = "6")]
    pub fuel_level: Vec<FuelLevel>,
    #[prost(message, repeated, tag = "7")]
    pub parking_brake: Vec<ParkingBrake>,
    #[prost(message, repeated, tag = "8")]
    pub gear: Vec<Gear>,
    #[prost(message, repeated, tag = "10")]
    pub night_mode: Vec<NightMode>,
    #[prost(message, repeated, tag = "11")]
    pub environment: Vec<Environment>,
    #[prost(message, repeated, tag = "13")]
    pub driving_status: Vec<DrivingStatus>,
}

#[derive(Clone, Copy, PartialEq, Message)]
pub struct Location {
    /// milliseconds since the epoch
    #[prost(uint64, optional, tag = "1")]
    pub timestamp: Option<u64>,
    /// degrees times 1e7
    #[prost(int32, optional, tag = "2")]
    pub latitude: Option<i32>,
    #[prost(int32, optional, tag = "3")]
    pub longitude: Option<i32>,
    /// metres times 1e3
    #[prost(uint32, optional, tag = "4")]
    pub accuracy: Option<u32>,
    /// metres times 1e2
    #[prost(int32, optional, tag = "5")]
    pub altitude: Option<i32>,
    /// times 1e3
    #[prost(int32, optional, tag = "6")]
    pub speed: Option<i32>,
    /// degrees times 1e6
    #[prost(int32, optional, tag = "7")]
    pub bearing: Option<i32>,
}

/// degrees times 1e6
#[derive(Clone, Copy, PartialEq, Message)]
pub struct Compass {
    #[prost(int32, optional, tag = "1")]
    pub bearing: Option<i32>,
    #[prost(int32, optional, tag = "2")]
    pub pitch: Option<i32>,
    #[prost(int32, optional, tag = "3")]
    pub roll: Option<i32>,
}

#[derive(Clone, Copy, PartialEq, Message)]
pub struct Speed {
    /// times 1e3
    #[prost(int32, optional, tag = "1")]
    pub speed: Option<i32>,
}

#[derive(Clone, Copy, PartialEq, Message)]
pub struct Rpm {
    #[prost(int32, optional, tag = "1")]
    pub rpm: Option<i32>,
}

#[derive(Clone, Copy, PartialEq, Message)]
pub struct Odometer {
    #[prost(int32, optional, tag = "1")]
    pub total: Option<i32>,
    #[prost(int32, optional, tag = "2")]
    pub trip: Option<i32>,
}

#[derive(Clone, Copy, PartialEq, Message)]
pub struct FuelLevel {
    #[prost(int32, optional, tag = "1")]
    pub level: Option<i32>,
    #[prost(int32, optional, tag = "2")]
    pub range: Option<i32>,
    #[prost(bool, optional, tag = "3")]
    pub low: Option<bool>,
}

#[derive(Clone, Copy, PartialEq, Message)]
pub struct ParkingBrake {
    #[prost(bool, optional, tag = "1")]
    pub engaged: Option<bool>,
}

#[derive(Clone, Copy, PartialEq, Message)]
pub struct Gear {
    /// 0 neutral, 1 to 10 the gear, 100 drive, 101 park, 102 reverse
    #[prost(int32, optional, tag = "1")]
    pub gear: Option<i32>,
}

#[derive(Clone, Copy, PartialEq, Message)]
pub struct NightMode {
    #[prost(bool, optional, tag = "1")]
    pub is_night: Option<bool>,
}

#[derive(Clone, Copy, PartialEq, Message)]
pub struct Environment {
    #[prost(int32, optional, tag = "1")]
    pub temperature: Option<i32>,
    #[prost(int32, optional, tag = "2")]
    pub pressure: Option<i32>,
    #[prost(int32, optional, tag = "3")]
    pub rain: Option<i32>,
}

#[derive(Clone, Copy, PartialEq, Message)]
pub struct DrivingStatus {
    /// restriction bits: 0 unrestricted, 1 no video, 2 no keyboard, 4 no
    /// voice input, 8 no setup, 16 limit message length
    #[prost(int32, optional, tag = "1")]
    pub status: Option<i32>,
}
