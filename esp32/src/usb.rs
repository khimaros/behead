//! usb transport on tinyusb. the headunit is the usb host, so the chip shows
//! up as a phone: first as an ordinary device, then, once the headunit asks
//! through the android open accessory requests, under google's accessory
//! ids. the one vendor interface is there from the start, and carries the
//! session once the chip is an accessory.

use aap::accessory::{self, Reply};
use esp_idf_svc::sys;
use std::ffi::{c_char, c_void, CString};
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const USB_2_0: u16 = 0x0200;
const DEVICE_VERSION: u16 = 0x0100;
const DT_DEVICE: u8 = 1;
const DT_CONFIGURATION: u8 = 2;
const DT_DEVICE_QUALIFIER: u8 = 6;
const CONTROL_PACKET: u8 = 64;
const CONFIGURATION_HEADER: usize = 9;
/// bus powered, in units of 2 ma
const ATTRIBUTES: u8 = 0x80;
const MAX_POWER: u8 = 250;
const LANG_EN_US: &[u8] = &[0x09, 0x04];
/// indexes into the strings handed to tinyusb, whose first is the language
const MANUFACTURER_STRING: u8 = 1;
const PRODUCT_STRING: u8 = 2;
const SERIAL_STRING: u8 = 3;
const INTERFACE_STRING: u8 = 4;
const VENDOR_INTERFACE: u8 = 0;
/// how long the bus stays released for the headunit to notice a new identity
const REENUMERATE: Duration = Duration::from_millis(250);
const POLL: Duration = Duration::from_millis(1);
/// room for the strings a headunit sends about itself, which nothing here reads
const STRING_ROOM: usize = 256;

/// set by the control callback when the headunit asked for accessory mode
static START_REQUESTED: AtomicBool = AtomicBool::new(false);
/// whether the chip currently shows google's accessory ids
static ACCESSORY: AtomicBool = AtomicBool::new(false);

/// what tinyusb keeps pointers to for as long as the driver is installed
struct Descriptors {
    device: sys::tusb_desc_device_t,
    qualifier: sys::tusb_desc_device_qualifier_t,
    full_speed: Vec<u8>,
    high_speed: Vec<u8>,
    strings: Vec<*const c_char>,
    /// answers to control requests, and the data that follows them
    version: [u8; 2],
    received: [u8; STRING_ROOM],
}

static mut DESCRIPTORS: Option<Box<Descriptors>> = None;

/// the driver's descriptors. tinyusb reads them from its own task, and the
/// only field changed after install, the ids, changes while the bus is released
fn descriptors() -> &'static mut Descriptors {
    // SAFETY: set once in install, before tinyusb starts, and never dropped
    unsafe { (*std::ptr::addr_of_mut!(DESCRIPTORS)).as_mut().expect("usb is installed") }
}

/// one configuration holding the accessory interface, at a packet size
fn configuration(packet: u16) -> Vec<u8> {
    let interface = accessory::interface_descriptors(packet, INTERFACE_STRING);
    let total = (CONFIGURATION_HEADER + interface.len()) as u16;
    let header =
        [&[CONFIGURATION_HEADER as u8, DT_CONFIGURATION][..], &total.to_le_bytes(), &[1, 1, 0, ATTRIBUTES, MAX_POWER]];
    [header.concat(), interface].concat()
}

fn device((vendor, product): (u16, u16)) -> sys::tusb_desc_device_t {
    sys::tusb_desc_device_t {
        bLength: size_of::<sys::tusb_desc_device_t>() as u8,
        bDescriptorType: DT_DEVICE,
        bcdUSB: USB_2_0,
        bDeviceClass: 0,
        bDeviceSubClass: 0,
        bDeviceProtocol: 0,
        bMaxPacketSize0: CONTROL_PACKET,
        idVendor: vendor,
        idProduct: product,
        bcdDevice: DEVICE_VERSION,
        iManufacturer: MANUFACTURER_STRING,
        iProduct: PRODUCT_STRING,
        iSerialNumber: SERIAL_STRING,
        bNumConfigurations: 1,
    }
}

fn qualifier() -> sys::tusb_desc_device_qualifier_t {
    sys::tusb_desc_device_qualifier_t {
        bLength: size_of::<sys::tusb_desc_device_qualifier_t>() as u8,
        bDescriptorType: DT_DEVICE_QUALIFIER,
        bcdUSB: USB_2_0,
        bDeviceClass: 0,
        bDeviceSubClass: 0,
        bDeviceProtocol: 0,
        bMaxPacketSize0: CONTROL_PACKET,
        bNumConfigurations: 1,
        bReserved: 0,
    }
}

/// a string tinyusb can keep: leaked, since the driver is never uninstalled
fn leaked(text: &[u8]) -> *const c_char {
    CString::new(text).expect("no nul in a usb string").into_raw()
}

/// start the usb device, showing the ids of an ordinary phone
pub fn install() -> Result<(), String> {
    let names = [accessory::MANUFACTURER, accessory::PRODUCT, accessory::SERIAL, accessory::INTERFACE_NAME];
    let strings = std::iter::once(LANG_EN_US).chain(names.iter().map(|name| name.as_bytes())).map(leaked).collect();
    let installed = Box::new(Descriptors {
        device: device(accessory::INITIAL_ID),
        qualifier: qualifier(),
        full_speed: configuration(accessory::FULL_SPEED_PACKET),
        high_speed: configuration(accessory::HIGH_SPEED_PACKET),
        strings,
        version: [0; 2],
        received: [0; STRING_ROOM],
    });
    // SAFETY: nothing reads the descriptors before tinyusb is installed below
    unsafe { *std::ptr::addr_of_mut!(DESCRIPTORS) = Some(installed) };
    let d = descriptors();
    // SAFETY: the config is plain data, valid when zeroed, and every pointer
    // in it stays valid for the life of the program
    let status = unsafe {
        let mut config: sys::tinyusb_config_t = std::mem::zeroed();
        config.__bindgen_anon_1.device_descriptor = &d.device;
        config.string_descriptor = d.strings.as_mut_ptr();
        config.string_descriptor_count = d.strings.len() as i32;
        config.__bindgen_anon_2.__bindgen_anon_1.configuration_descriptor = d.full_speed.as_ptr();
        config.hs_configuration_descriptor = d.high_speed.as_ptr();
        config.qualifier_descriptor = &d.qualifier;
        sys::tinyusb_driver_install(&config)
    };
    if status != sys::ESP_OK {
        return Err(format!("usb: installing the driver failed ({status})"));
    }
    std::thread::spawn(switch_identities);
    Ok(())
}

/// leave the bus and come back under other ids, as if replugged
fn enumerate((vendor, product): (u16, u16)) {
    // SAFETY: tinyusb is installed, and reads the ids again once reconnected
    unsafe { sys::tud_disconnect() };
    std::thread::sleep(REENUMERATE);
    (descriptors().device.idVendor, descriptors().device.idProduct) = (vendor, product);
    // SAFETY: as above
    unsafe { sys::tud_connect() };
}

fn mounted() -> bool {
    // SAFETY: tinyusb is installed
    unsafe { sys::tud_mounted() }
}

/// become an accessory when the headunit asks, and an ordinary device again
/// once it has been gone for the grace period. a headunit lets go of a device
/// it goes on using, when it resets or reconfigures it
fn switch_identities() {
    let grace = Duration::from_millis(accessory::UNPLUG_GRACE_MS.into());
    let mut gone_since: Option<Instant> = None;
    loop {
        std::thread::sleep(POLL * 10);
        if START_REQUESTED.swap(false, Ordering::SeqCst) && !ACCESSORY.load(Ordering::SeqCst) {
            log::info!("usb: the headunit asked for accessory mode");
            enumerate(accessory::ACCESSORY_ID);
            ACCESSORY.store(true, Ordering::SeqCst);
            gone_since = Some(Instant::now());
        }
        if !ACCESSORY.load(Ordering::SeqCst) || mounted() {
            gone_since = None;
        } else if gone_since.get_or_insert_with(Instant::now).elapsed() > grace {
            log::info!("usb: the headunit is gone");
            ACCESSORY.store(false, Ordering::SeqCst);
            enumerate(accessory::INITIAL_ID);
            gone_since = None;
        }
    }
}

/// whether a headunit has the accessory interface configured
fn live() -> bool {
    ACCESSORY.load(Ordering::SeqCst) && mounted()
}

/// block until a headunit has switched the chip to accessory mode, then hand
/// out the read and write halves of the session stream
pub fn accept() -> (FromHost, ToHost) {
    while !live() {
        std::thread::sleep(POLL * 10);
    }
    (FromHost, ToHost)
}

/// answer the control requests of the accessory handshake. tinyusb calls
/// this from its own task, for every vendor request, at each stage of it
#[no_mangle]
extern "C" fn tud_vendor_control_xfer_cb(rhport: u8, stage: u8, request: *const sys::tusb_control_request_t) -> bool {
    if u32::from(stage) != sys::CONTROL_STAGE_SETUP {
        return true;
    }
    let d = descriptors();
    // SAFETY: tinyusb hands over a valid request, which is the eight bytes of a setup packet
    let setup = unsafe { std::slice::from_raw_parts(request.cast::<u8>(), size_of::<sys::tusb_control_request_t>()) };
    let (buffer, length): (*mut c_void, usize) = match accessory::reply(setup) {
        Reply::Send(version) => {
            d.version = version;
            (d.version.as_mut_ptr().cast(), version.len())
        }
        Reply::Receive(length) => (d.received.as_mut_ptr().cast(), length),
        Reply::Start(length) => {
            START_REQUESTED.store(true, Ordering::SeqCst);
            (d.received.as_mut_ptr().cast(), length)
        }
        Reply::Stall => return false,
    };
    // SAFETY: the buffers live in the descriptors, and are as long as what is transferred
    unsafe {
        match length.min(STRING_ROOM) {
            0 => sys::tud_control_status(rhport, request),
            length => sys::tud_control_xfer(rhport, request, buffer, length as u16),
        }
    }
}

/// the bulk out endpoint: what the headunit sends
pub struct FromHost;

impl Read for FromHost {
    /// blocks until the headunit sent something. reads nothing once it is gone
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        loop {
            // SAFETY: tinyusb is installed, and the buffer is writable for the length given
            let read =
                unsafe { sys::tud_vendor_n_read(VENDOR_INTERFACE, buffer.as_mut_ptr().cast(), buffer.len() as u32) };
            if read > 0 || !live() {
                return Ok(read as usize);
            }
            std::thread::sleep(POLL);
        }
    }
}

/// the bulk in endpoint: what the headunit receives
pub struct ToHost;

impl Write for ToHost {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        let mut rest = data;
        while !rest.is_empty() {
            if !live() {
                return Err(std::io::ErrorKind::BrokenPipe.into());
            }
            // SAFETY: tinyusb is installed, and `rest` is readable for its length
            let written = unsafe {
                let written = sys::tud_vendor_n_write(VENDOR_INTERFACE, rest.as_ptr().cast(), rest.len() as u32);
                sys::tud_vendor_n_write_flush(VENDOR_INTERFACE);
                written as usize
            };
            rest = &rest[written..];
            if written == 0 {
                std::thread::sleep(POLL);
            }
        }
        Ok(data.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
