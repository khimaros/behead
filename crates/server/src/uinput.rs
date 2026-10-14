//! kernel input devices for the headunit's touchscreen and buttons. any
//! compositor reading input through libinput sees the car as local input,
//! and maps the touchscreen's range onto whichever output it is assigned to.

use aap::proto;
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::time::{Duration, Instant};

const UINPUT_PATH: &str = "/dev/uinput";
const TOUCHSCREEN_NAME: &str = "behead touchscreen";
const KEYS_NAME: &str = "behead keys";
const WHEEL_NAME: &str = "behead knob";
/// where the wheel's pointer is: half of the positions an axis of it has,
/// which a screen of any size takes for its middle
const POINTER_MIDDLE: i32 = 1 << 14;
const BUS_VIRTUAL: u16 = 0x06;
const EV_SYN: u16 = 0x00;
const EV_KEY: u16 = 0x01;
const EV_REL: u16 = 0x02;
const EV_ABS: u16 = 0x03;
const SYN_REPORT: u16 = 0x00;
const BTN_TOUCH: u16 = 0x14a;
const BTN_LEFT: u16 = 0x110;
const REL_WHEEL: u16 = 0x08;
const REL_WHEEL_HI_RES: u16 = 0x0b;
/// what one notch of a wheel is in the kernel's finer unit
const WHEEL_NOTCH: i32 = 120;
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
const UI_SET_RELBIT: u32 = iow(102, size_of::<libc::c_int>());
const UI_SET_ABSBIT: u32 = iow(103, size_of::<libc::c_int>());
const UI_SET_PROPBIT: u32 = iow(110, size_of::<libc::c_int>());

/// android key codes the headunit may send, and the linux keys they become,
/// in android's order. a car has the buttons among them; a headunit with a
/// keyboard, as the laptop's is, has the rest, and a device gets only the
/// keys its headunit says it has
#[rustfmt::skip]
const KEYMAP: &[(u32, u16)] = &[
    (3, 172),   // home: KEY_HOMEPAGE
    (4, 158),   // back: KEY_BACK
    (5, 169),   // call: KEY_PHONE
    (6, 0x1be), // end call: KEY_HANGUP_PHONE
    // 0 to 9: KEY_0 comes after KEY_9
    (7, 11), (8, 2), (9, 3), (10, 4), (11, 5), (12, 6), (13, 7), (14, 8), (15, 9), (16, 10),
    (19, 103),  // dpad up: KEY_UP
    (20, 108),  // dpad down: KEY_DOWN
    (21, 105),  // dpad left: KEY_LEFT
    (22, 106),  // dpad right: KEY_RIGHT
    (23, 28),   // dpad center: KEY_ENTER
    (24, 115),  // volume up: KEY_VOLUMEUP
    (25, 114),  // volume down: KEY_VOLUMEDOWN
    // a to z, which linux numbers by their place on the keyboard
    (29, 30), (30, 48), (31, 46), (32, 32), (33, 18), (34, 33), (35, 34), (36, 35), (37, 23),
    (38, 36), (39, 37), (40, 38), (41, 50), (42, 49), (43, 24), (44, 25), (45, 16), (46, 19),
    (47, 31), (48, 20), (49, 22), (50, 47), (51, 17), (52, 45), (53, 21), (54, 44),
    (55, 51),   // comma: KEY_COMMA
    (56, 52),   // period: KEY_DOT
    (57, 56),   // alt left: KEY_LEFTALT
    (58, 100),  // alt right: KEY_RIGHTALT
    (59, 42),   // shift left: KEY_LEFTSHIFT
    (60, 54),   // shift right: KEY_RIGHTSHIFT
    (61, 15),   // tab: KEY_TAB
    (62, 57),   // space: KEY_SPACE
    (66, 28),   // enter: KEY_ENTER
    (67, 14),   // del, which is backspace: KEY_BACKSPACE
    (68, 41),   // grave: KEY_GRAVE
    (69, 12),   // minus: KEY_MINUS
    (70, 13),   // equals: KEY_EQUAL
    (71, 26),   // left bracket: KEY_LEFTBRACE
    (72, 27),   // right bracket: KEY_RIGHTBRACE
    (73, 43),   // backslash: KEY_BACKSLASH
    (74, 39),   // semicolon: KEY_SEMICOLON
    (75, 40),   // apostrophe: KEY_APOSTROPHE
    (76, 53),   // slash: KEY_SLASH
    (84, 217),  // search: KEY_SEARCH
    (85, 164),  // media play/pause: KEY_PLAYPAUSE
    (87, 163),  // media next: KEY_NEXTSONG
    (88, 165),  // media previous: KEY_PREVIOUSSONG
    (111, 1),   // escape: KEY_ESC
    (112, 111), // forward del: KEY_DELETE
    (113, 29),  // ctrl left: KEY_LEFTCTRL
    (114, 97),  // ctrl right: KEY_RIGHTCTRL
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
const KEY_ESC: u16 = 1;
const KEY_BACKSPACE: u16 = 14;
const KEY_ENTER: u16 = 28;
/// android's codes for the two buttons that go with a knob: its click and back
const CLICK: u32 = 23;
const BACK: u32 = 4;
/// what the knob types, in the order a turn goes through them: a to z, the
/// space, and 1 to 0 as a keyboard has them
#[rustfmt::skip]
const TYPED: [u16; 37] = [
    30, 48, 46, 32, 18, 33, 34, 35, 23, 36, 37, 38, 50, 49, 24, 25, 16, 19, 31, 20, 22, 47, 17, 45, 21, 44,
    57, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11,
];
/// a click held this long steps the knob to its next use
const LONG_CLICK: Duration = Duration::from_millis(600);
/// detents typed out of one turn, so a wild delta cannot hold the session up
const MAX_DETENTS: u32 = 32;

/// what a turn of the car's rotary knob becomes, one detent at a time. linux
/// has no input for a knob that applications agree on, so it becomes keys,
/// or the wheel of a mouse
#[derive(Clone, Copy, Default, PartialEq)]
pub enum Knob {
    /// tab and shift+tab: the next and previous thing that takes focus
    #[default]
    Focus,
    /// down and up, for applications that give tab another meaning
    Arrows,
    /// a notch of a mouse's wheel, which is how a map made for a desk zooms
    /// and has no other way that a car has: not a key, not two fingers
    Scroll,
    /// a letter into whatever takes text, the next or the one before in
    /// its place with every detent: a car's knob is all it has to write with
    Type,
}

/// the names `--knob` takes, as a turn's line to the session gives them too
const KNOBS: [(&str, Knob); 4] =
    [("focus", Knob::Focus), ("arrows", Knob::Arrows), ("scroll", Knob::Scroll), ("type", Knob::Type)];

impl std::str::FromStr for Knob {
    type Err = String;

    fn from_str(name: &str) -> Result<Self, String> {
        let known = KNOBS.iter().find(|(known, _)| *known == name).map(|(_, knob)| *knob);
        known.ok_or(format!("--knob takes focus, arrows, scroll or type, or several with , between, not {name}"))
    }
}

impl std::fmt::Display for Knob {
    fn fmt(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        formatter.write_str(KNOBS.iter().find(|(_, knob)| knob == self).map_or("", |(name, _)| name))
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
            (Self::Scroll | Self::Type, _) => &[],
        }
    }

    /// every key a turn or a click may come out as
    fn keys(self) -> impl Iterator<Item = u16> {
        let typed: &[u16] = if self == Self::Type { &TYPED } else { &[] };
        let beside: &[u16] = if self == Self::Type { &[KEY_ENTER, KEY_BACKSPACE] } else { &[] };
        let chords = [true, false].into_iter().flat_map(move |clockwise| self.chord(clockwise).iter().copied());
        chords.chain(typed.iter().copied()).chain(beside.iter().copied())
    }

    /// one detent as reports: the chord pressed, then released in reverse,
    /// or a notch of the wheel. clockwise is the wheel rolled toward the
    /// hand, as the headunit page has it, where a mouse's wheel is the knob:
    /// rolled down there it is a clockwise turn, and has to arrive as rolled
    /// down. the pointer is put in the middle of the screen first, since a
    /// map zooms about wherever the pointer is. from a step beside it,
    /// every time: the kernel passes on no position that is the one before,
    /// and a wheel turns for nothing once a finger on the screen has taken
    /// the pointer's place, until the pointer moves again
    fn detent(self, clockwise: bool) -> Vec<Event> {
        if self == Self::Scroll {
            let way = if clockwise { -1 } else { 1 };
            return vec![
                (EV_ABS, ABS_X, POINTER_MIDDLE + 1),
                (EV_ABS, ABS_Y, POINTER_MIDDLE),
                (EV_SYN, SYN_REPORT, 0),
                (EV_ABS, ABS_X, POINTER_MIDDLE),
                (EV_SYN, SYN_REPORT, 0),
                (EV_REL, REL_WHEEL, way),
                (EV_REL, REL_WHEEL_HI_RES, way * WHEEL_NOTCH),
                (EV_SYN, SYN_REPORT, 0),
            ];
        }
        let chord = self.chord(clockwise);
        let presses = chord.iter().map(|&key| (key, 1)).chain(chord.iter().rev().map(|&key| (key, 0)));
        presses.flat_map(|(key, value)| report(key, value)).collect()
    }
}

/// what the car's back button is: the key linux has for it, or escape, which
/// is what closes a menu or a dialog in applications made for a desk
#[derive(Clone, Copy, Default, PartialEq)]
pub enum Back {
    #[default]
    Back,
    Escape,
}

impl std::str::FromStr for Back {
    type Err = String;

    fn from_str(name: &str) -> Result<Self, String> {
        match name {
            "back" => Ok(Self::Back),
            "escape" => Ok(Self::Escape),
            _ => Err(format!("--back takes back or escape, not {name}")),
        }
    }
}

fn report(key: u16, value: i32) -> [Event; 2] {
    [(EV_KEY, key, value), (EV_SYN, SYN_REPORT, 0)]
}

/// a key down and up again
fn tap(key: u16) -> Vec<Event> {
    [report(key, 1), report(key, 0)].concat()
}

/// the knob and the two buttons that go with it, its click and back: which
/// of its uses the knob is on, stepped through by a long click when it has
/// several, and the letter being chosen when it types
pub struct Dial {
    modes: Vec<Knob>,
    mode: usize,
    back: Back,
    /// which of TYPED stands in the text, still to be kept or turned past
    letter: Option<usize>,
}

impl Dial {
    pub fn new(modes: Vec<Knob>, back: Back) -> Self {
        Self { modes, mode: 0, back, letter: None }
    }

    pub fn mode(&self) -> Knob {
        self.modes[self.mode]
    }

    /// whether the click means something here before it does to an
    /// application. where it does not, the application gets it as it is
    /// pressed and let go, and may tell a long press from a short one itself
    fn owns_click(&self) -> bool {
        self.modes.len() > 1 || self.mode() == Knob::Type
    }

    fn key(&self, android: u32) -> Option<u16> {
        if android == BACK && self.back == Back::Escape {
            Some(KEY_ESC)
        } else {
            linux_key(android)
        }
    }

    /// one detent. typing, it writes the next letter, over the one a detent
    /// before wrote
    fn turn(&mut self, clockwise: bool) -> Vec<Event> {
        if self.mode() != Knob::Type {
            return self.mode().detent(clockwise);
        }
        let count = TYPED.len();
        let next = match (self.letter, clockwise) {
            (None, true) => 0,
            (None, false) => count - 1,
            (Some(letter), true) => (letter + 1) % count,
            (Some(letter), false) => (letter + count - 1) % count,
        };
        let over = if self.letter.replace(next).is_some() { tap(KEY_BACKSPACE) } else { vec![] };
        [over, tap(TYPED[next])].concat()
    }

    /// the click, let go: held long it steps to the knob's next use. short,
    /// it keeps the letter being chosen, and is enter where there is none
    fn click(&mut self, long: bool) -> Vec<Event> {
        if long && self.modes.len() > 1 {
            (self.mode, self.letter) = ((self.mode + 1) % self.modes.len(), None);
            return vec![];
        }
        if self.letter.take().is_some() {
            vec![]
        } else {
            tap(KEY_ENTER)
        }
    }

    /// the back button, which takes a letter away while typing
    fn back(&mut self, pressed: bool) -> Vec<Event> {
        match (self.mode(), pressed) {
            (Knob::Type, true) => {
                self.letter = None;
                tap(KEY_BACKSPACE)
            }
            (Knob::Type, false) => vec![],
            _ => self.key(BACK).map(|key| report(key, pressed as i32).to_vec()).unwrap_or_default(),
        }
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

/// give a device an axis that reports positions from 0 to `maximum`
fn absolute_axis(file: &File, code: u16, maximum: i32) -> Result<(), String> {
    ioctl_int(file, UI_SET_ABSBIT, code)?;
    let absinfo = libc::input_absinfo { value: 0, minimum: 0, maximum, fuzz: 0, flat: 0, resolution: 0 };
    ioctl_ptr(file, UI_ABS_SETUP, &libc::uinput_abs_setup { code, absinfo })
}

/// the knob as a mouse's wheel. a wheel is taken for one only on something
/// that is otherwise a mouse, so this one has a button, never pressed, and a
/// place on the screen, which a wheel turns out to need: what it zooms is
/// zoomed about the pointer. its place is given as a position, not as moves,
/// so that it can be said exactly
fn create_wheel() -> Result<Device, String> {
    Device::create(WHEEL_NAME, |file| {
        ioctl_int(file, UI_SET_EVBIT, EV_KEY)?;
        ioctl_int(file, UI_SET_KEYBIT, BTN_LEFT)?;
        ioctl_int(file, UI_SET_EVBIT, EV_ABS)?;
        [ABS_X, ABS_Y].into_iter().try_for_each(|axis| absolute_axis(file, axis, POINTER_MIDDLE * 2 - 1))?;
        ioctl_int(file, UI_SET_EVBIT, EV_REL)?;
        [REL_WHEEL, REL_WHEEL_HI_RES].into_iter().try_for_each(|axis| ioctl_int(file, UI_SET_RELBIT, axis))
    })
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
            axes.iter().try_for_each(|&(code, maximum)| absolute_axis(file, code, maximum as i32))
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
/// plus the keys its knob types if it has one, or a wheel for it to be
pub struct Devices {
    touchscreen: Option<Touchscreen>,
    keys: Option<Device>,
    wheel: Option<Device>,
    dial: Dial,
    /// when the knob's click went down, while it is down
    clicked: Option<Instant>,
}

impl Devices {
    pub fn create(channel: &proto::InputChannel, dial: Dial) -> Result<Self, String> {
        let area = channel.touch_screen_config.as_ref().and_then(|c| Some((c.width?, c.height?)));
        let touchscreen = area.filter(|&(w, h)| w > 0 && h > 0).map(|(w, h)| Touchscreen::create(w, h)).transpose()?;
        let codes = &channel.supported_keycodes;
        let turns = codes.contains(&ROTARY);
        let turned = dial.modes.iter().filter(|_| turns).flat_map(|mode| mode.keys());
        let wheel = (turns && dial.modes.contains(&Knob::Scroll)).then(create_wheel).transpose()?;
        let keys: Vec<u16> = codes.iter().filter_map(|&code| dial.key(code)).chain(turned).collect();
        let keys = (!keys.is_empty())
            .then(|| {
                Device::create(KEYS_NAME, |file| {
                    ioctl_int(file, UI_SET_EVBIT, EV_KEY)?;
                    keys.iter().try_for_each(|&key| ioctl_int(file, UI_SET_KEYBIT, key))
                })
            })
            .transpose()?;
        Ok(Self { touchscreen, keys, wheel, dial, clicked: None })
    }

    /// what a button is to the keys device. the knob's click, where the
    /// knob has a say in it, counts once it is let go, by how long it was down
    fn button(&mut self, code: u32, pressed: bool) -> Vec<Event> {
        match (code, pressed) {
            (CLICK, true) if self.dial.owns_click() => {
                self.clicked = Some(Instant::now());
                vec![]
            }
            (CLICK, false) if self.dial.owns_click() => {
                let long = self.clicked.take().is_some_and(|since| since.elapsed() >= LONG_CLICK);
                self.dial.click(long)
            }
            (BACK, _) => self.dial.back(pressed),
            _ => self.dial.key(code).map(|key| report(key, pressed as i32).to_vec()).unwrap_or_default(),
        }
    }

    /// pass on what the headunit sent. returns a line for the session when
    /// the knob has gone to another of its uses, so it can say so on screen
    pub fn deliver(&mut self, event: &proto::InputEvent) -> Result<Option<String>, String> {
        if let (Some(screen), Some(touch)) = (self.touchscreen.as_mut(), &event.touch_event) {
            screen.touch(touch)?;
        }
        let before = self.dial.mode();
        for button in event.button_event.iter().flat_map(|b| &b.button_events) {
            let Some(code) = button.scan_code else { continue };
            let events = self.button(code, button.is_pressed.unwrap_or(false));
            if let Some(keys) = self.keys.as_mut() {
                keys.emit(&events)?;
            }
        }
        // the knob's turn goes to the wheel when it is one, or is typed
        let turns = event.relative_event.iter().flat_map(|r| &r.relative_events);
        for delta in turns.filter(|turn| turn.scan_code == Some(ROTARY)).filter_map(|turn| turn.delta) {
            for _ in 0..delta.unsigned_abs().min(MAX_DETENTS) {
                let wheel = if self.dial.mode() == Knob::Scroll { self.wheel.as_mut() } else { None };
                let Some(turned) = wheel.or(self.keys.as_mut()) else { break };
                turned.emit(&self.dial.turn(delta > 0))?;
            }
        }
        let now = self.dial.mode();
        Ok((now != before).then(|| format!("knob {now}")))
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

    #[test]
    fn a_detent_can_be_a_notch_of_a_mouse_wheel() {
        let knob: Knob = "scroll".parse().unwrap();
        let notch = |way| {
            vec![
                (EV_ABS, ABS_X, POINTER_MIDDLE + 1),
                (EV_ABS, ABS_Y, POINTER_MIDDLE),
                (EV_SYN, SYN_REPORT, 0),
                (EV_ABS, ABS_X, POINTER_MIDDLE),
                (EV_SYN, SYN_REPORT, 0),
                (EV_REL, REL_WHEEL, way),
                (EV_REL, REL_WHEEL_HI_RES, way * WHEEL_NOTCH),
                (EV_SYN, SYN_REPORT, 0),
            ]
        };
        // the pointer has to move for every notch: the kernel passes on no
        // position that is the one before, and a compositor gives the wheel
        // to nothing once a finger has been on the screen since the pointer moved
        assert_eq!(knob.detent(true), notch(-1), "clockwise is the wheel rolled toward the hand");
        assert_eq!(knob.detent(false), notch(1));
        assert_eq!(knob.keys().count(), 0, "a wheel types nothing");
        assert_eq!(knob.to_string(), "scroll", "named to the session as it is on the command line");
    }

    fn dial(modes: &str, back: Back) -> Dial {
        Dial::new(modes.split(',').map(|mode| mode.parse().unwrap()).collect(), back)
    }

    #[test]
    fn a_long_click_takes_the_knob_to_its_next_use_and_a_short_one_is_enter() {
        let mut dial = dial("focus,scroll,type", Back::Back);
        assert_eq!(dial.click(false), tap(KEY_ENTER));
        assert!(dial.mode() == Knob::Focus);
        assert_eq!(dial.click(true), vec![], "a long click types nothing");
        assert!(dial.mode() == Knob::Scroll);
        dial.click(true);
        dial.click(true);
        assert!(dial.mode() == Knob::Focus, "and after the last comes the first");
        let mut only = super::tests::dial("arrows", Back::Back);
        assert!(!only.owns_click(), "a knob with one use leaves its click to the application, held or not");
        only.click(true);
        assert!(only.mode() == Knob::Arrows);
    }

    #[test]
    fn the_knob_types_a_letter_at_a_time_into_whatever_takes_text() {
        let mut dial = dial("type", Back::Back);
        let (a, b, last) = (TYPED[0], TYPED[1], TYPED[TYPED.len() - 1]);
        assert_eq!(dial.turn(true), tap(a), "the first detent writes the first letter");
        assert_eq!(dial.turn(true), [tap(KEY_BACKSPACE), tap(b)].concat(), "the next writes another in its place");
        assert_eq!(dial.turn(false), [tap(KEY_BACKSPACE), tap(a)].concat());
        assert_eq!(dial.click(false), vec![], "a click keeps the letter");
        assert_eq!(dial.turn(false), tap(last), "and the next letter starts from either end");
        dial.click(false);
        assert_eq!(dial.click(false), tap(KEY_ENTER), "a click with no letter to keep is enter");
    }

    #[test]
    fn back_takes_a_letter_away_while_typing_and_is_escape_where_asked() {
        let mut typing = dial("type", Back::Escape);
        typing.turn(true);
        assert_eq!(typing.back(true), tap(KEY_BACKSPACE));
        assert_eq!(typing.back(false), vec![], "the key went down and up already");
        assert_eq!(typing.turn(true), tap(TYPED[0]), "the letter that went is not written over");
        let report = |key, value| vec![(EV_KEY, key, value), (EV_SYN, SYN_REPORT, 0)];
        assert_eq!(dial("focus", Back::Escape).back(true), report(KEY_ESC, 1), "what closes a menu on a desk");
        assert_eq!(dial("focus", Back::Back).back(false), report(linux_key(BACK).unwrap(), 0));
        assert!("escape".parse::<Back>().is_ok() && "back".parse::<Back>().is_ok() && "home".parse::<Back>().is_err());
    }

    #[test]
    fn a_keyboards_keys_become_the_same_keys_here() {
        // android's code, then linux's: a, z, 0, 9, space, backspace, shift, slash
        for (android, linux) in [(29, 30), (54, 44), (7, 11), (16, 10), (62, 57), (67, 14), (59, 42), (76, 53)] {
            assert_eq!(linux_key(android), Some(linux), "android key {android}");
        }
        let mut codes: Vec<u32> = KEYMAP.iter().map(|(android, _)| *android).collect();
        codes.dedup();
        assert_eq!(codes.len(), KEYMAP.len(), "an android key listed twice");
        assert!(codes.is_sorted(), "kept in android's order, to be found in");
    }
}
