//! kernel input devices for the headunit's touchscreen and buttons. any
//! compositor reading input through libinput sees the car as local input,
//! and maps the touchscreen's range onto whichever output it is assigned to.

use aap::proto;
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;

const UINPUT_PATH: &str = "/dev/uinput";
const TOUCHSCREEN_NAME: &str = "behead touchscreen";
const KEYS_NAME: &str = "behead keys";
const BUS_VIRTUAL: u16 = 0x06;
const EV_SYN: u16 = 0x00;
const EV_KEY: u16 = 0x01;
const EV_ABS: u16 = 0x03;
const SYN_REPORT: u16 = 0x00;
const BTN_TOUCH: u16 = 0x14a;
const ABS_X: u16 = 0x00;
const ABS_Y: u16 = 0x01;
const ABS_MT_SLOT: u16 = 0x2f;
const ABS_MT_POSITION_X: u16 = 0x35;
const ABS_MT_POSITION_Y: u16 = 0x36;
const ABS_MT_TRACKING_ID: u16 = 0x39;
/// fingers tracked at once. android pointer ids at or above this are dropped
const MAX_CONTACTS: u32 = 10;
const TRACKING_IDS: i32 = 0x10000;

const fn iow(nr: u32, size: usize) -> u32 {
    1 << 30 | (size as u32) << 16 | (b'U' as u32) << 8 | nr
}
const UI_DEV_CREATE: u32 = 0x5501;
const UI_DEV_SETUP: u32 = iow(3, size_of::<libc::uinput_setup>());
const UI_ABS_SETUP: u32 = iow(4, size_of::<libc::uinput_abs_setup>());
const UI_SET_EVBIT: u32 = iow(100, size_of::<libc::c_int>());
const UI_SET_KEYBIT: u32 = iow(101, size_of::<libc::c_int>());
const UI_SET_ABSBIT: u32 = iow(103, size_of::<libc::c_int>());
const UI_SET_PROPBIT: u32 = iow(110, size_of::<libc::c_int>());

/// android key codes the headunit may send, and the linux keys they become
const KEYMAP: [(u32, u16); 19] = [
    (3, 172),   // home: KEY_HOMEPAGE
    (4, 158),   // back: KEY_BACK
    (5, 169),   // call: KEY_PHONE
    (6, 0x1be), // end call: KEY_HANGUP_PHONE
    (19, 103),  // dpad up: KEY_UP
    (20, 108),  // dpad down: KEY_DOWN
    (21, 105),  // dpad left: KEY_LEFT
    (22, 106),  // dpad right: KEY_RIGHT
    (23, 28),   // dpad center: KEY_ENTER
    (24, 115),  // volume up: KEY_VOLUMEUP
    (25, 114),  // volume down: KEY_VOLUMEDOWN
    (66, 28),   // enter: KEY_ENTER
    (84, 217),  // search: KEY_SEARCH
    (85, 164),  // media play/pause: KEY_PLAYPAUSE
    (87, 163),  // media next: KEY_NEXTSONG
    (88, 165),  // media previous: KEY_PREVIOUSSONG
    (126, 200), // media play: KEY_PLAYCD
    (127, 201), // media pause: KEY_PAUSECD
    (164, 113), // volume mute: KEY_MUTE
];

/// the android key code a rotary knob's turns arrive under
const ROTARY: u32 = 65536;
const KEY_TAB: u16 = 15;
const KEY_LEFTSHIFT: u16 = 42;
const KEY_UP: u16 = 103;
const KEY_DOWN: u16 = 108;
/// detents typed out of one turn, so a wild delta cannot hold the session up
const MAX_DETENTS: u32 = 32;

/// what a turn of the car's rotary knob types, one detent at a time. linux
/// has no input for a knob that applications agree on, so it becomes keys
#[derive(Clone, Copy, Default)]
pub enum Knob {
    /// tab and shift+tab: the next and previous thing that takes focus
    #[default]
    Focus,
    /// down and up, for applications that give tab another meaning
    Arrows,
}

impl std::str::FromStr for Knob {
    type Err = String;

    fn from_str(name: &str) -> Result<Self, String> {
        match name {
            "focus" => Ok(Self::Focus),
            "arrows" => Ok(Self::Arrows),
            _ => Err(format!("--knob takes focus or arrows, not {name}")),
        }
    }
}

impl Knob {
    /// the keys held together for one detent, in the order they go down
    fn chord(self, clockwise: bool) -> &'static [u16] {
        match (self, clockwise) {
            (Self::Focus, true) => &[KEY_TAB],
            (Self::Focus, false) => &[KEY_LEFTSHIFT, KEY_TAB],
            (Self::Arrows, true) => &[KEY_DOWN],
            (Self::Arrows, false) => &[KEY_UP],
        }
    }

    fn keys(self) -> impl Iterator<Item = u16> {
        [true, false].into_iter().flat_map(move |clockwise| self.chord(clockwise).iter().copied())
    }

    /// one detent as key reports: the chord pressed, then released in reverse
    fn detent(self, clockwise: bool) -> Vec<Event> {
        let chord = self.chord(clockwise);
        let presses = chord.iter().map(|&key| (key, 1)).chain(chord.iter().rev().map(|&key| (key, 0)));
        presses.flat_map(|(key, value)| [(EV_KEY, key, value), (EV_SYN, SYN_REPORT, 0)]).collect()
    }
}

type Event = (u16, u16, i32);
/// positions of the fingers down, by android pointer id
type Contacts = BTreeMap<u32, (i32, i32)>;

fn linux_key(android: u32) -> Option<u16> {
    KEYMAP.iter().find(|(code, _)| *code == android).map(|(_, key)| *key)
}

fn ioctl_int(file: &File, request: u32, value: u16) -> Result<(), String> {
    // SAFETY: the request takes an int argument, passed by value
    let result = unsafe { libc::ioctl(file.as_raw_fd(), request as _, value as libc::c_int) };
    (result >= 0).then_some(()).ok_or_else(|| format!("uinput: {}", std::io::Error::last_os_error()))
}

fn ioctl_ptr<T>(file: &File, request: u32, value: &T) -> Result<(), String> {
    // SAFETY: the request reads a T, which outlives the call
    let result = unsafe { libc::ioctl(file.as_raw_fd(), request as _, value as *const T) };
    (result >= 0).then_some(()).ok_or_else(|| format!("uinput: {}", std::io::Error::last_os_error()))
}

/// one uinput device. closing the file removes it from the kernel
struct Device(File);

impl Device {
    fn create(name: &str, configure: impl FnOnce(&File) -> Result<(), String>) -> Result<Self, String> {
        let file = OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(UINPUT_PATH)
            .map_err(|e| format!("uinput: {UINPUT_PATH}: {e}"))?;
        configure(&file)?;
        // SAFETY: uinput_setup is plain data, valid when zeroed
        let mut setup: libc::uinput_setup = unsafe { std::mem::zeroed() };
        setup.id.bustype = BUS_VIRTUAL;
        setup.name.iter_mut().zip(name.bytes()).for_each(|(to, from)| *to = from as libc::c_char);
        ioctl_ptr(&file, UI_DEV_SETUP, &setup)?;
        ioctl_int(&file, UI_DEV_CREATE, 0)?;
        Ok(Self(file))
    }

    fn emit(&mut self, events: &[Event]) -> Result<(), String> {
        let mut bytes = Vec::with_capacity(events.len() * size_of::<libc::input_event>());
        for &(kind, code, value) in events {
            // SAFETY: input_event is plain data, valid when zeroed; the kernel stamps the time
            let mut event: libc::input_event = unsafe { std::mem::zeroed() };
            (event.type_, event.code, event.value) = (kind, code, value);
            // SAFETY: reads the bytes of a live, fully initialised struct
            bytes.extend_from_slice(unsafe {
                std::slice::from_raw_parts((&event as *const libc::input_event).cast::<u8>(), size_of_val(&event))
            });
        }
        self.0.write_all(&bytes).map_err(|e| format!("uinput: {e}"))
    }
}

/// the fingers down once an android touch event has taken effect
fn contacts_after(event: &proto::TouchEvent) -> Contacts {
    let action = event.touch_action.unwrap_or(-1);
    if action == proto::TOUCH_UP {
        return Contacts::new();
    }
    let lifted = (action == proto::TOUCH_POINTER_UP).then(|| event.action_index.unwrap_or(0) as usize);
    let points = event.touch_location.iter().enumerate().filter(|(index, _)| Some(*index) != lifted);
    points
        .map(|(_, p)| (p.pointer_id.unwrap_or(0), (p.x.unwrap_or(0) as i32, p.y.unwrap_or(0) as i32)))
        .filter(|(pointer, _)| *pointer < MAX_CONTACTS)
        .collect()
}

/// the evdev report moving one set of contacts to the next: multitouch
/// protocol b, one slot per android pointer id, plus single touch emulation
/// for the first finger, which libinput requires of a touchscreen
fn transition(old: &Contacts, new: &Contacts, next_tracking_id: &mut i32) -> Vec<Event> {
    let mut events = Vec::new();
    for (&pointer, &(x, y)) in new.iter().filter(|(pointer, at)| old.get(pointer) != Some(at)) {
        events.push((EV_ABS, ABS_MT_SLOT, pointer as i32));
        if !old.contains_key(&pointer) {
            events.push((EV_ABS, ABS_MT_TRACKING_ID, *next_tracking_id));
            *next_tracking_id = (*next_tracking_id + 1) % TRACKING_IDS;
        }
        events.extend([(EV_ABS, ABS_MT_POSITION_X, x), (EV_ABS, ABS_MT_POSITION_Y, y)]);
    }
    for &pointer in old.keys().filter(|pointer| !new.contains_key(pointer)) {
        events.extend([(EV_ABS, ABS_MT_SLOT, pointer as i32), (EV_ABS, ABS_MT_TRACKING_ID, -1)]);
    }
    if old.is_empty() != new.is_empty() {
        events.push((EV_KEY, BTN_TOUCH, !new.is_empty() as i32));
    }
    if let Some(&(x, y)) = new.values().next() {
        events.extend([(EV_ABS, ABS_X, x), (EV_ABS, ABS_Y, y)]);
    }
    events.push((EV_SYN, SYN_REPORT, 0));
    events
}

struct Touchscreen {
    device: Device,
    contacts: Contacts,
    next_tracking_id: i32,
}

impl Touchscreen {
    fn create(width: u32, height: u32) -> Result<Self, String> {
        let axes = [
            (ABS_X, width - 1),
            (ABS_Y, height - 1),
            (ABS_MT_SLOT, MAX_CONTACTS - 1),
            (ABS_MT_POSITION_X, width - 1),
            (ABS_MT_POSITION_Y, height - 1),
            (ABS_MT_TRACKING_ID, TRACKING_IDS as u32 - 1),
        ];
        let device = Device::create(TOUCHSCREEN_NAME, |file| {
            ioctl_int(file, UI_SET_PROPBIT, libc::INPUT_PROP_DIRECT)?;
            ioctl_int(file, UI_SET_EVBIT, EV_KEY)?;
            ioctl_int(file, UI_SET_KEYBIT, BTN_TOUCH)?;
            ioctl_int(file, UI_SET_EVBIT, EV_ABS)?;
            axes.iter().try_for_each(|&(code, maximum)| {
                ioctl_int(file, UI_SET_ABSBIT, code)?;
                let absinfo = libc::input_absinfo {
                    value: 0,
                    minimum: 0,
                    maximum: maximum as i32,
                    fuzz: 0,
                    flat: 0,
                    resolution: 0,
                };
                ioctl_ptr(file, UI_ABS_SETUP, &libc::uinput_abs_setup { code, absinfo })
            })
        })?;
        Ok(Self { device, contacts: Contacts::new(), next_tracking_id: 0 })
    }

    fn touch(&mut self, event: &proto::TouchEvent) -> Result<(), String> {
        let contacts = contacts_after(event);
        let events = transition(&self.contacts, &contacts, &mut self.next_tracking_id);
        self.contacts = contacts;
        self.device.emit(&events)
    }
}

/// the headunit's input as kernel devices: a touchscreen spanning its touch
/// area, and a keyboard with the keys it advertised that have a linux match,
/// plus the keys its knob types if it has one
pub struct Devices {
    touchscreen: Option<Touchscreen>,
    keys: Option<Device>,
    knob: Knob,
}

impl Devices {
    pub fn create(channel: &proto::InputChannel, knob: Knob) -> Result<Self, String> {
        let area = channel.touch_screen_config.as_ref().and_then(|c| Some((c.width?, c.height?)));
        let touchscreen = area.filter(|&(w, h)| w > 0 && h > 0).map(|(w, h)| Touchscreen::create(w, h)).transpose()?;
        let codes = &channel.supported_keycodes;
        let turned = codes.contains(&ROTARY).then(|| knob.keys()).into_iter().flatten();
        let keys: Vec<u16> = codes.iter().filter_map(|&code| linux_key(code)).chain(turned).collect();
        let keys = (!keys.is_empty())
            .then(|| {
                Device::create(KEYS_NAME, |file| {
                    ioctl_int(file, UI_SET_EVBIT, EV_KEY)?;
                    keys.iter().try_for_each(|&key| ioctl_int(file, UI_SET_KEYBIT, key))
                })
            })
            .transpose()?;
        Ok(Self { touchscreen, keys, knob })
    }

    pub fn deliver(&mut self, event: &proto::InputEvent) -> Result<(), String> {
        if let (Some(screen), Some(touch)) = (self.touchscreen.as_mut(), &event.touch_event) {
            screen.touch(touch)?;
        }
        let Some(keys) = self.keys.as_mut() else { return Ok(()) };
        for button in event.button_event.iter().flat_map(|b| &b.button_events) {
            if let Some(key) = button.scan_code.and_then(linux_key) {
                keys.emit(&[(EV_KEY, key, button.is_pressed.unwrap_or(false) as i32), (EV_SYN, SYN_REPORT, 0)])?;
            }
        }
        let turns = event.relative_event.iter().flat_map(|r| &r.relative_events);
        for delta in turns.filter(|turn| turn.scan_code == Some(ROTARY)).filter_map(|turn| turn.delta) {
            for _ in 0..delta.unsigned_abs().min(MAX_DETENTS) {
                keys.emit(&self.knob.detent(delta > 0))?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_detent_back_holds_shift_around_tab() {
        let report = |key, value| [(EV_KEY, key, value), (EV_SYN, SYN_REPORT, 0)];
        let expected = [report(KEY_LEFTSHIFT, 1), report(KEY_TAB, 1), report(KEY_TAB, 0), report(KEY_LEFTSHIFT, 0)];
        assert_eq!(Knob::Focus.detent(false), expected.concat());
    }
}
