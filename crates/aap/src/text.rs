//! what a headunit sends as text lines, in the format the README documents:
//! one line per touch, button or control event. a host prints them, pipes
//! them to its video source, or draws from them.

use crate::proto;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// how much of something a headunit sent that nothing here reads is shown
const UNKNOWN_BYTES: usize = 32;

/// the start of what a headunit sent, in hex, for an `unknown` line
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().take(UNKNOWN_BYTES).map(|byte| format!("{byte:02x}")).collect()
}

/// a touch on the touchscreen or a touchpad as a line: every finger down,
/// and which of them the action is about
fn touch_line(surface: &str, touch: &proto::TouchEvent) -> String {
    let action = match touch.touch_action.unwrap_or(-1) {
        proto::TOUCH_DOWN => "down".to_string(),
        proto::TOUCH_UP => "up".to_string(),
        proto::TOUCH_MOVED => "move".to_string(),
        proto::TOUCH_POINTER_DOWN => "pointer-down".to_string(),
        proto::TOUCH_POINTER_UP => "pointer-up".to_string(),
        other => other.to_string(),
    };
    let points: Vec<String> = touch
        .touch_location
        .iter()
        .map(|p| format!("{}:{},{}", p.pointer_id.unwrap_or(0), p.x.unwrap_or(0), p.y.unwrap_or(0)))
        .collect();
    format!("{surface} {action} {} {}", touch.action_index.unwrap_or(0), points.join(" "))
}

/// what a headunit's input services offer, one line each, for the log: a
/// control that sends nothing is usually one the car never listed
pub fn describe_inputs(info: &proto::ServiceDiscoveryResponse) -> Vec<String> {
    let size = |name: &str, config: &Option<proto::TouchConfig>| {
        config.as_ref().map(|c| format!(", {name} {}x{}", c.width.unwrap_or(0), c.height.unwrap_or(0)))
    };
    let inputs = info.channels.iter().filter_map(|c| Some((c.channel_id.unwrap_or(0), c.input_channel.as_ref()?)));
    inputs
        .map(|(channel, input)| {
            let keys: Vec<String> = input.supported_keycodes.iter().map(u32::to_string).collect();
            let keys = if keys.is_empty() { "no keys".to_string() } else { format!("keys {}", keys.join(" ")) };
            let screen = size("touchscreen", &input.touch_screen_config).unwrap_or_default();
            let pad = size("touchpad", &input.touch_pad_config).unwrap_or_default();
            format!("input channel {channel}: {keys}{screen}{pad}")
        })
        .collect()
}

/// one text line per touch, button or control event
pub fn input_lines(event: &proto::InputEvent) -> Vec<String> {
    let surfaces = [("touch", &event.touch_event), ("touchpad", &event.touchpad_event)];
    let mut lines: Vec<String> =
        surfaces.iter().filter_map(|(surface, touch)| Some(touch_line(surface, touch.as_ref()?))).collect();
    for button in event.button_event.iter().flat_map(|b| &b.button_events) {
        let state = if button.is_pressed.unwrap_or(false) { "down" } else { "up" };
        lines.push(format!("button {} {state}", button.scan_code.unwrap_or(0)));
    }
    for control in event.absolute_event.iter().flat_map(|a| &a.absolute_events) {
        lines.push(format!("absolute {} {}", control.scan_code.unwrap_or(0), control.value.unwrap_or(0)));
    }
    for control in event.relative_event.iter().flat_map(|r| &r.relative_events) {
        lines.push(format!("relative {} {}", control.scan_code.unwrap_or(0), control.delta.unwrap_or(0)));
    }
    lines
}

/// the lines for what a headunit sent that nothing here reads: a whole
/// message no channel handles, or one field of an event
pub fn unknown_message(channel: u8, id: u16, body: &[u8]) -> String {
    format!("unknown message {channel} {id:#06x} {}", hex(body))
}

pub fn unknown_field(kind: &str, channel: u8, field: u32, value: &[u8]) -> String {
    format!("unknown {kind} {channel} field {field} {}", hex(value))
}
