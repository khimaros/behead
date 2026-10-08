//! the android open accessory handshake, as the phone sees it. the headunit
//! is the usb host: it finds an ordinary device, asks it over vendor control
//! requests whether it speaks the accessory protocol, and tells it to start.
//! the device then comes back under google's accessory ids, with one vendor
//! interface of two bulk endpoints that carries the session. this module is
//! the part every usb stack shares; the stack itself belongs to the host.

use alloc::vec::Vec;

/// ids shown before the switch (the ones AACS uses) and google's accessory ids after
pub const INITIAL_ID: (u16, u16) = (0x12d1, 0x107e);
pub const ACCESSORY_ID: (u16, u16) = (0x18d1, 0x2d00);
pub const MANUFACTURER: &str = "khimaros";
pub const PRODUCT: &str = "behead";
pub const SERIAL: &str = "0001";
pub const INTERFACE_NAME: &str = "Android Accessory Interface";
pub const PROTOCOL_VERSION: u16 = 2;
pub const FULL_SPEED_PACKET: u16 = 64;
pub const HIGH_SPEED_PACKET: u16 = 512;
/// the bulk endpoints: to the headunit, and from it
pub const ENDPOINT_IN: u8 = 1 | USB_DIR_IN;
pub const ENDPOINT_OUT: u8 = 2;
/// time to wait after the headunit lets go of the device for it to take it
/// up again before treating it as unplugged: hosts reset, reconfigure and
/// reauthorize devices they go on using
pub const UNPLUG_GRACE_MS: u32 = 2000;

const GET_PROTOCOL: u8 = 51;
const SEND_STRING: u8 = 52;
const START: u8 = 53;
const USB_DIR_IN: u8 = 0x80;
const DT_INTERFACE: u8 = 4;
const DT_ENDPOINT: u8 = 5;
const CLASS_VENDOR: u8 = 0xff;
const XFER_BULK: u8 = 2;
const SETUP_LENGTH: usize = 8;
/// index of the interface's name among the device's strings
const INTERFACE_STRING: u8 = 1;

/// one interface with a bulk in and a bulk out endpoint, at the given packet size
pub fn interface_descriptors(packet: u16, name: u8) -> Vec<u8> {
    let endpoint = |address: u8| [&[7, DT_ENDPOINT, address, XFER_BULK][..], &packet.to_le_bytes(), &[0]].concat();
    let interface = [9, DT_INTERFACE, 0, 0, 2, CLASS_VENDOR, CLASS_VENDOR, 0, name].to_vec();
    [interface, endpoint(ENDPOINT_IN), endpoint(ENDPOINT_OUT)].concat()
}

/// the interface as functionfs wants it, which keeps the strings of a
/// function in a table of its own
pub fn function_descriptors(packet: u16) -> Vec<u8> {
    interface_descriptors(packet, INTERFACE_STRING)
}

/// what to do with one control request from the headunit
#[derive(Debug, PartialEq, Eq)]
pub enum Reply {
    /// answer with these bytes
    Send([u8; 2]),
    /// take the data that follows, this many bytes, and acknowledge
    Receive(usize),
    /// as Receive, then come back as an accessory
    Start(usize),
    /// not an accessory request: refuse it
    Stall,
}

/// decide on a control request from its eight byte setup packet
pub fn reply(setup: &[u8]) -> Reply {
    if setup.len() < SETUP_LENGTH {
        return Reply::Stall;
    }
    let (to_host, request) = (setup[0] & USB_DIR_IN != 0, setup[1]);
    let length = u16::from_le_bytes([setup[6], setup[7]]) as usize;
    match (request, to_host) {
        (GET_PROTOCOL, true) => Reply::Send(PROTOCOL_VERSION.to_le_bytes()),
        (SEND_STRING, false) => Reply::Receive(length),
        (START, false) => Reply::Start(length),
        _ => Reply::Stall,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VENDOR_IN: u8 = 0xc0;
    const VENDOR_OUT: u8 = 0x40;

    #[test]
    fn answers_the_requests_of_the_accessory_handshake() {
        assert_eq!(reply(&[VENDOR_IN, 51, 0, 0, 0, 0, 2, 0]), Reply::Send([2, 0]));
        assert_eq!(reply(&[VENDOR_OUT, 52, 0, 0, 1, 0, 8, 0]), Reply::Receive(8));
        assert_eq!(reply(&[VENDOR_OUT, 53, 0, 0, 0, 0, 0, 0]), Reply::Start(0));
    }

    #[test]
    fn refuses_what_is_not_part_of_it() {
        assert_eq!(reply(&[VENDOR_IN, 53, 0, 0, 0, 0, 0, 0]), Reply::Stall, "start goes to the device");
        assert_eq!(reply(&[VENDOR_OUT, 51, 0, 0, 0, 0, 2, 0]), Reply::Stall, "the version comes from the device");
        assert_eq!(reply(&[VENDOR_IN, 6, 0, 1, 0, 0, 18, 0]), Reply::Stall);
        assert_eq!(reply(&[VENDOR_OUT, 53]), Reply::Stall);
    }

    #[test]
    fn describes_one_vendor_interface_with_two_bulk_endpoints() {
        let descriptors = interface_descriptors(HIGH_SPEED_PACKET, 4);
        assert_eq!(descriptors.len(), 9 + 7 + 7);
        assert_eq!(descriptors[..9], [9, 4, 0, 0, 2, 0xff, 0xff, 0, 4]);
        assert_eq!(descriptors[9..16], [7, 5, 0x81, 2, 0, 2, 0]);
        assert_eq!(descriptors[16..], [7, 5, 0x02, 2, 0, 2, 0]);
    }
}
