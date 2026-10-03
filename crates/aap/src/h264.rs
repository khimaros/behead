//! annex-b h.264 stream helpers: the headunit wants one access unit per
//! media message, with parameter sets delivered apart from picture data.

use alloc::vec::Vec;

const START_CODE: [u8; 3] = [0, 0, 1];
const NAL_TYPE_MASK: u8 = 0x1f;
const NAL_SPS: u8 = 7;
const NAL_PPS: u8 = 8;
/// high bit of the first slice header byte is set when first_mb_in_slice == 0
const FIRST_MB_ZERO: u8 = 0x80;

fn is_vcl(nal_type: u8) -> bool {
    (1..=5).contains(&nal_type)
}

/// sei, sps, pps and access unit delimiters can only precede a picture
fn is_unit_prefix(nal_type: u8) -> bool {
    (6..=9).contains(&nal_type)
}

fn find_start_code(buf: &[u8], from: usize) -> Option<usize> {
    buf.get(from..)?.windows(START_CODE.len()).position(|w| w == START_CODE).map(|i| i + from)
}

/// splits an annex-b byte stream into access units. a unit is released once
/// the first nal of the following unit is seen, which costs one frame of latency.
#[derive(Default)]
pub struct AccessUnitSplitter {
    buf: Vec<u8>,
    scan: usize,
    has_vcl: bool,
}

impl AccessUnitSplitter {
    pub fn push(&mut self, data: &[u8]) -> Vec<Vec<u8>> {
        self.buf.extend_from_slice(data);
        let mut units = Vec::new();
        loop {
            let Some(mut code) = find_start_code(&self.buf, self.scan) else {
                self.scan = self.scan.max(self.buf.len().saturating_sub(START_CODE.len() - 1));
                break;
            };
            let Some(&[header, slice]) = self.buf.get(code + 3..code + 5) else {
                self.scan = code;
                break;
            };
            let nal_type = header & NAL_TYPE_MASK;
            let new_picture = is_vcl(nal_type) && slice & FIRST_MB_ZERO != 0;
            if self.has_vcl && (is_unit_prefix(nal_type) || new_picture) {
                let start = if code > 0 && self.buf[code - 1] == 0 { code - 1 } else { code };
                units.push(self.buf.drain(..start).collect());
                code -= start;
                self.has_vcl = false;
            }
            self.has_vcl |= is_vcl(nal_type);
            self.scan = code + START_CODE.len();
        }
        units
    }

    /// release whatever is buffered, for use at end of stream
    pub fn finish(&mut self) -> Option<Vec<u8>> {
        self.take()
    }

    /// release a buffered picture because the stream went quiet. encoders
    /// write each picture in one go, so a pause means it is complete.
    pub fn has_picture(&self) -> bool {
        self.has_vcl
    }

    pub fn idle(&mut self) -> Option<Vec<u8>> {
        self.has_vcl.then(|| self.take()).flatten()
    }

    fn take(&mut self) -> Option<Vec<u8>> {
        self.scan = 0;
        self.has_vcl = false;
        Some(core::mem::take(&mut self.buf)).filter(|unit| !unit.is_empty())
    }
}

/// split an access unit into its leading sps/pps nals and the remainder
pub fn split_config(unit: &[u8]) -> (&[u8], &[u8]) {
    let mut from = 0;
    while let Some(code) = find_start_code(unit, from) {
        let nal_type = unit.get(code + 3).map_or(0, |b| b & NAL_TYPE_MASK);
        if nal_type != NAL_SPS && nal_type != NAL_PPS {
            let start = if code > 0 && unit[code - 1] == 0 { code - 1 } else { code };
            return unit.split_at(start);
        }
        from = code + START_CODE.len();
    }
    (unit, &[])
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    const SPS: &[u8] = &[0, 0, 0, 1, 0x67, 0x42];
    const PPS: &[u8] = &[0, 0, 0, 1, 0x68, 0xce];
    const IDR: &[u8] = &[0, 0, 0, 1, 0x65, 0x88, 0x84];
    const P: &[u8] = &[0, 0, 0, 1, 0x41, 0x9a, 0x21];
    /// second slice of the same picture: first_mb_in_slice != 0
    const P_SLICE2: &[u8] = &[0, 0, 1, 0x41, 0x1a, 0x21];

    #[test]
    fn splits_units_at_any_read_boundary() {
        let stream = [SPS, PPS, IDR, P, P_SLICE2, P, SPS, PPS, IDR].concat();
        let expect = [[SPS, PPS, IDR].concat(), [P, P_SLICE2].concat(), P.to_vec(), [SPS, PPS, IDR].concat()];
        for chunk in [1, 2, 3, 7, stream.len()] {
            let mut splitter = AccessUnitSplitter::default();
            let mut units: Vec<Vec<u8>> = stream.chunks(chunk).flat_map(|c| splitter.push(c)).collect();
            units.extend(splitter.finish());
            assert_eq!(units, expect, "chunk size {chunk}");
        }
    }

    #[test]
    fn releases_a_picture_when_the_stream_goes_quiet() {
        let mut splitter = AccessUnitSplitter::default();
        assert_eq!(splitter.push(&[SPS, PPS].concat()), Vec::<Vec<u8>>::new());
        assert_eq!(splitter.idle(), None, "parameter sets alone wait for their picture");
        splitter.push(IDR);
        assert_eq!(splitter.idle(), Some([SPS, PPS, IDR].concat()));
        assert_eq!(splitter.push(&[P, P].concat()), vec![P.to_vec()]);
        assert_eq!(splitter.idle(), Some(P.to_vec()));
        assert_eq!(splitter.idle(), None);
    }

    #[test]
    fn separates_parameter_sets() {
        let unit = [SPS, PPS, IDR].concat();
        assert_eq!(split_config(&unit), (&[SPS, PPS].concat()[..], IDR));
        assert_eq!(split_config(P), (&[][..], P));
    }
}
