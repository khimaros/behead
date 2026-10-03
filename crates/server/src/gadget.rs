//! usb gadget transport. the headunit is the usb host, so this machine shows
//! up as a phone: first as an ordinary device, then, once the headunit asks
//! through the android open accessory requests, as an accessory with two
//! bulk endpoints carrying the session.

use std::ffi::CString;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::path::Path;
use std::sync::{Arc, Condvar, Mutex};

const CONFIGFS: &str = "/sys/kernel/config/usb_gadget";
const GADGET: &str = "/sys/kernel/config/usb_gadget/behead";
const FUNCTION: &str = "functions/ffs.aap";
const CONFIG: &str = "configs/c.1";
const CONFIG_LINK: &str = "configs/c.1/ffs.aap";
const STRINGS: &str = "strings/0x409";
const CONFIG_STRINGS: &str = "configs/c.1/strings/0x409";
const FFS_NAME: &str = "aap";
const FFS_MOUNT: &str = "/run/behead-ffs";
const UDC_CLASS: &str = "/sys/class/udc";
const MODULES: [&str; 2] = ["libcomposite", "usb_f_fs"];

const MANUFACTURER: &str = "khimaros";
const PRODUCT: &str = "behead";
const SERIAL: &str = "0001";
const INTERFACE_NAME: &[u8] = b"Android Accessory Interface\0";
const LANG_EN_US: u16 = 0x0409;

/// ids shown before the switch (the ones AACS uses) and google's accessory ids after
const INITIAL_ID: (u16, u16) = (0x12d1, 0x107e);
const ACCESSORY_ID: (u16, u16) = (0x18d1, 0x2d00);

const AOA_GET_PROTOCOL: u8 = 51;
const AOA_SEND_STRING: u8 = 52;
const AOA_START: u8 = 53;
const AOA_PROTOCOL_VERSION: u16 = 2;

const FFS_DESCRIPTORS_MAGIC_V2: u32 = 3;
const FFS_STRINGS_MAGIC: u32 = 2;
const FFS_HAS_FS_DESC: u32 = 1;
const FFS_HAS_HS_DESC: u32 = 2;
/// deliver device-recipient control requests to us, which the accessory requests are
const FFS_ALL_CTRL_RECIP: u32 = 64;
const FFS_EVENT_LEN: usize = 12;
/// time to wait after a disable for the host to enable the device again
/// before treating it as unplugged
const UNPLUG_GRACE_MS: i32 = 2000;
const FFS_EVENT_BATCH: usize = 4;
const EVENT_UNBIND: u8 = 1;
const EVENT_ENABLE: u8 = 2;
const EVENT_DISABLE: u8 = 3;
const EVENT_SETUP: u8 = 4;

const USB_DIR_IN: u8 = 0x80;
const DT_INTERFACE: u8 = 4;
const DT_ENDPOINT: u8 = 5;
const CLASS_VENDOR: u8 = 0xff;
const XFER_BULK: u8 = 2;
const FULL_SPEED_PACKET: u16 = 64;
/// how /sys/class/udc/*/current_speed names a full speed link
const FULL_SPEED: &str = "full-speed";
const HIGH_SPEED_PACKET: u16 = 512;
const DESCRIPTORS_PER_SPEED: u32 = 3;

fn context<T>(result: std::io::Result<T>, what: &str) -> Result<T, String> {
    result.map_err(|e| format!("{what}: {e}"))
}

fn put(file: &str, value: &str) -> Result<(), String> {
    let path = Path::new(GADGET).join(file);
    context(fs::write(&path, value), &format!("writing {}", path.display()))
}

fn make_dir(dir: &str) -> Result<(), String> {
    let path = Path::new(GADGET).join(dir);
    context(fs::create_dir_all(&path), &format!("creating {}", path.display()))
}

/// one interface with a bulk in and a bulk out endpoint, at the given packet size
fn speed_descriptors(packet: u16) -> Vec<u8> {
    let endpoint = |address: u8| [&[7, DT_ENDPOINT, address, XFER_BULK][..], &packet.to_le_bytes(), &[0]].concat();
    [vec![9, DT_INTERFACE, 0, 0, 2, CLASS_VENDOR, CLASS_VENDOR, 0, 1], endpoint(1 | USB_DIR_IN), endpoint(2)].concat()
}

fn with_header(magic: u32, words: &[u32], body: &[u8]) -> Vec<u8> {
    let length = (8 + words.len() * 4 + body.len()) as u32;
    let header: Vec<u8> = [magic, length].iter().chain(words).flat_map(|w| w.to_le_bytes()).collect();
    [&header[..], body].concat()
}

fn descriptors() -> Vec<u8> {
    let flags = FFS_HAS_FS_DESC | FFS_HAS_HS_DESC | FFS_ALL_CTRL_RECIP;
    let body = [speed_descriptors(FULL_SPEED_PACKET), speed_descriptors(HIGH_SPEED_PACKET)].concat();
    with_header(FFS_DESCRIPTORS_MAGIC_V2, &[flags, DESCRIPTORS_PER_SPEED, DESCRIPTORS_PER_SPEED], &body)
}

fn strings() -> Vec<u8> {
    with_header(FFS_STRINGS_MAGIC, &[1, 1], &[&LANG_EN_US.to_le_bytes(), INTERFACE_NAME].concat())
}

fn mount_functionfs() -> Result<(), String> {
    context(fs::create_dir_all(FFS_MOUNT), "creating functionfs mountpoint")?;
    let [source, target, kind] = [FFS_NAME, FFS_MOUNT, "functionfs"].map(|s| CString::new(s).unwrap());
    // SAFETY: all pointers are valid nul-terminated strings for the duration of the call
    let status = unsafe { libc::mount(source.as_ptr(), target.as_ptr(), kind.as_ptr(), 0, std::ptr::null()) };
    context(if status == 0 { Ok(()) } else { Err(std::io::Error::last_os_error()) }, "mounting functionfs")
}

fn find_udc() -> Result<String, String> {
    let first = context(fs::read_dir(UDC_CLASS), "no usb device controller")?.flatten().next();
    first.map(|entry| entry.file_name().to_string_lossy().into_owned()).ok_or("no usb device controller".into())
}

/// re-enumerate under the given ids by detaching from the controller and back
fn enumerate(udc: &str, (vendor, product): (u16, u16)) -> Result<(), String> {
    let _ = put("UDC", "\n");
    put("idVendor", &format!("{vendor:#06x}"))?;
    put("idProduct", &format!("{product:#06x}"))?;
    put("UDC", udc)
}

/// remove every trace of the gadget. safe to call when nothing is set up, and
/// meant to run in a process that holds no endpoint files open.
pub fn teardown() -> Result<(), String> {
    let _ = put("UDC", "\n");
    let mount = CString::new(FFS_MOUNT).unwrap();
    // SAFETY: the pointer is a valid nul-terminated string for the duration of the call
    unsafe { libc::umount(mount.as_ptr()) };
    let _ = fs::remove_file(Path::new(GADGET).join(CONFIG_LINK));
    for dir in [CONFIG_STRINGS, CONFIG, FUNCTION, STRINGS, ""] {
        let _ = fs::remove_dir(Path::new(GADGET).join(dir));
    }
    let _ = fs::remove_dir(FFS_MOUNT);
    if Path::new(GADGET).exists() || Path::new(FFS_MOUNT).exists() {
        return Err("gadget teardown incomplete".into());
    }
    Ok(())
}

/// true while a headunit has the accessory interface configured
type Live = Arc<(Mutex<bool>, Condvar)>;

pub struct Gadget {
    to_host: File,
    from_host: File,
    live: Live,
    udc: String,
}

/// the bulk in endpoint. a transfer that fills its last packet exactly is
/// followed by a zero length packet, without which a host reading into a
/// larger buffer waits for the next write before it sees this one.
pub struct ToHost {
    file: File,
    packet: usize,
}

impl Write for ToHost {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        self.file.write_all(data)?;
        if !data.is_empty() && data.len().is_multiple_of(self.packet) {
            // write, not write_all: write_all returns early on an empty buffer
            // without making the system call that queues the packet
            self.file.write(&[])?;
        }
        Ok(data.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}

/// bulk packet size for the speed the controller negotiated with the host
fn packet_size(udc: &str) -> usize {
    let speed = fs::read_to_string(Path::new(UDC_CLASS).join(udc).join("current_speed")).unwrap_or_default();
    usize::from(if speed.trim() == FULL_SPEED { FULL_SPEED_PACKET } else { HIGH_SPEED_PACKET })
}

impl Gadget {
    /// build the gadget from scratch and attach it to `udc` (or the first one found)
    pub fn create(udc: Option<&str>) -> Result<Self, String> {
        teardown()?;
        if !Path::new(CONFIGFS).exists() {
            let _ = std::process::Command::new("modprobe").arg("-a").args(MODULES).status();
        }
        let udc = udc.map_or_else(find_udc, |name| Ok(name.to_string()))?;
        for dir in [STRINGS, FUNCTION, CONFIG_STRINGS] {
            make_dir(dir)?;
        }
        for (file, value) in [("manufacturer", MANUFACTURER), ("product", PRODUCT), ("serialnumber", SERIAL)] {
            put(&format!("{STRINGS}/{file}"), value)?;
        }
        put(&format!("{CONFIG_STRINGS}/configuration"), PRODUCT)?;
        let gadget = Path::new(GADGET);
        context(std::os::unix::fs::symlink(gadget.join(FUNCTION), gadget.join(CONFIG_LINK)), "linking function")?;
        mount_functionfs()?;
        let endpoint = |name: &str| {
            let path = Path::new(FFS_MOUNT).join(name);
            context(OpenOptions::new().read(true).write(true).open(&path), &format!("opening {}", path.display()))
        };
        let mut ep0 = endpoint("ep0")?;
        context(ep0.write_all(&descriptors()), "writing usb descriptors")?;
        context(ep0.write_all(&strings()), "writing usb strings")?;
        let (to_host, from_host) = (endpoint("ep1")?, endpoint("ep2")?);
        enumerate(&udc, INITIAL_ID)?;
        let live = Live::default();
        let (control_live, control_udc) = (live.clone(), udc.clone());
        std::thread::spawn(move || {
            if let Err(e) = control_loop(ep0, &control_udc, &control_live) {
                eprintln!("usb control: {e}");
                std::process::exit(1);
            }
        });
        Ok(Self { to_host, from_host, live, udc })
    }

    /// block until a headunit has switched us to accessory mode, then hand
    /// out the read and write halves of the session stream
    pub fn accept(&self) -> Result<(File, ToHost), String> {
        let (lock, changed) = &*self.live;
        drop(changed.wait_while(lock.lock().unwrap(), |live| !*live).unwrap());
        let to_host = ToHost { file: context(self.to_host.try_clone(), "ep1")?, packet: packet_size(&self.udc) };
        Ok((context(self.from_host.try_clone(), "ep2")?, to_host))
    }
}

fn set_live(live: &Live, value: bool) {
    *live.0.lock().unwrap() = value;
    live.1.notify_all();
}

/// answer one control request. returns true when the headunit asked us to
/// start accessory mode.
fn on_setup(ep0: &mut File, setup: &[u8]) -> Result<bool, String> {
    let (direction, request) = (setup[0] & USB_DIR_IN, setup[1]);
    let length = u16::from_le_bytes([setup[6], setup[7]]) as usize;
    match (request, direction) {
        (AOA_GET_PROTOCOL, USB_DIR_IN) => {
            context(ep0.write_all(&AOA_PROTOCOL_VERSION.to_le_bytes()), "answering protocol request")?;
        }
        (AOA_SEND_STRING | AOA_START, 0) => {
            let mut data = vec![0; length];
            context(ep0.read(&mut data), "reading accessory request")?;
        }
        // functionfs stalls a request when asked for io in the wrong direction
        (_, USB_DIR_IN) => drop(ep0.read(&mut [])),
        _ => drop(ep0.write(&[])),
    }
    Ok(request == AOA_START)
}

/// true once `file` has data to read, false if `timeout_ms` passes first
fn readable_within(file: &File, timeout_ms: i32) -> bool {
    let mut poll = libc::pollfd { fd: file.as_raw_fd(), events: libc::POLLIN, revents: 0 };
    // SAFETY: one valid pollfd for the duration of the call
    unsafe { libc::poll(&mut poll, 1, timeout_ms) > 0 }
}

/// serve the control endpoint: perform the accessory switch on request and
/// fall back to the initial identity when the headunit goes away. a disable
/// ends the session at once but only counts as an unplug if no enable follows
/// within the grace period: hosts reset, reconfigure and reauthorize devices
/// they go on using.
fn control_loop(mut ep0: File, udc: &str, live: &Live) -> Result<(), String> {
    let (mut accessory, mut unplugged) = (false, false);
    let mut events = [0; FFS_EVENT_LEN * FFS_EVENT_BATCH];
    loop {
        if unplugged && !readable_within(&ep0, UNPLUG_GRACE_MS) {
            (accessory, unplugged) = (false, false);
            enumerate(udc, INITIAL_ID)?;
            continue;
        }
        let n = context(ep0.read(&mut events), "reading usb events")?;
        for event in events[..n].chunks_exact(FFS_EVENT_LEN) {
            let was_live = *live.0.lock().unwrap();
            match event[8] {
                EVENT_SETUP if on_setup(&mut ep0, event)? && !accessory => {
                    accessory = true;
                    enumerate(udc, ACCESSORY_ID)?;
                }
                EVENT_ENABLE if accessory => {
                    unplugged = false;
                    set_live(live, true);
                }
                EVENT_DISABLE | EVENT_UNBIND if was_live => {
                    unplugged = true;
                    set_live(live, false);
                }
                _ => {}
            }
        }
    }
}
