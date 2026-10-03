//! wire framing: `[channel u8][flags u8][len u16 be]`, then a `u32 be` total
//! message length on the first frame of a multi-frame message, then payload.

use alloc::vec::Vec;

pub const FLAG_FIRST: u8 = 1 << 0;
pub const FLAG_LAST: u8 = 1 << 1;
pub const FLAG_BULK: u8 = FLAG_FIRST | FLAG_LAST;
/// control-plane message (ids from the control enum) sent on a service channel
pub const FLAG_CONTROL: u8 = 1 << 2;
pub const FLAG_ENCRYPTED: u8 = 1 << 3;

/// largest plaintext chunk carried by one frame
pub const MAX_FRAME_PAYLOAD: usize = 0x4000;

const HEADER_LEN: usize = 4;
const TOTAL_LEN: usize = 4;

#[derive(Debug, PartialEq)]
pub struct Frame {
    pub channel: u8,
    pub flags: u8,
    pub payload: Vec<u8>,
}

/// true for the first frame of a multi-frame message, which carries a total length
fn has_total(flags: u8) -> bool {
    flags & FLAG_BULK == FLAG_FIRST
}

/// append one frame to `out`. `total` is the full plaintext message length.
pub fn encode_frame(channel: u8, flags: u8, total: u32, payload: &[u8], out: &mut Vec<u8>) {
    out.extend_from_slice(&[channel, flags]);
    out.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    if has_total(flags) {
        out.extend_from_slice(&total.to_be_bytes());
    }
    out.extend_from_slice(payload);
}

/// reassembles frames from a byte stream, tolerating arbitrary read boundaries
/// so the same code serves tcp streams and usb bulk transfers.
#[derive(Default)]
pub struct FrameReader {
    buf: Vec<u8>,
}

impl FrameReader {
    pub fn push(&mut self, data: &[u8]) {
        self.buf.extend_from_slice(data);
    }

    pub fn next_frame(&mut self) -> Option<Frame> {
        if self.buf.len() < HEADER_LEN {
            return None;
        }
        let (channel, flags) = (self.buf[0], self.buf[1]);
        let len = u16::from_be_bytes([self.buf[2], self.buf[3]]) as usize;
        let start = HEADER_LEN + if has_total(flags) { TOTAL_LEN } else { 0 };
        if self.buf.len() < start + len {
            return None;
        }
        let payload = self.buf[start..start + len].to_vec();
        self.buf.drain(..start + len);
        Some(Frame { channel, flags, payload })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_across_split_reads() {
        let mut wire = Vec::new();
        encode_frame(3, FLAG_FIRST | FLAG_ENCRYPTED, 9, b"hello", &mut wire);
        encode_frame(3, FLAG_LAST | FLAG_ENCRYPTED, 9, b"miss", &mut wire);
        let mut reader = FrameReader::default();
        let mut frames = Vec::new();
        for byte in wire {
            reader.push(&[byte]);
            frames.extend(reader.next_frame());
        }
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].payload, b"hello");
        assert_eq!(frames[1], Frame { channel: 3, flags: FLAG_LAST | FLAG_ENCRYPTED, payload: b"miss".to_vec() });
    }
}
