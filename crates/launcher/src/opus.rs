//! opus files: the ogg pages they come in are read here, and the packets
//! inside them decoded by the opuscule crate.

use crate::decode::{fill, Decoder, Format, Input};
use opuscule::{sample_to_i16, Channels, SampleRate};
use std::collections::VecDeque;

const PAGE_MAGIC: &[u8] = b"OggS";
/// a page's header, up to the lengths of its segments, the count of which
/// is its last byte
const PAGE_HEADER: usize = 27;
/// a segment this long goes on in the next one
const FULL_SEGMENT: usize = 255;
/// larger than any packet of sound: a damaged file is given up there
const MAX_PACKET: usize = 1 << 20;
const HEAD_MAGIC: &[u8] = b"OpusHead";
/// what the first packet says, by where in it: the channels, the frames to
/// drop at the start, a gain in 256ths of a decibel, and how the channels
/// are laid out, which is 0 for one or two of them
const HEAD_LENGTH: usize = 19;
const HEAD_CHANNELS: usize = 9;
const HEAD_SKIP: usize = 10;
const HEAD_GAIN: usize = 16;
const HEAD_LAYOUT: usize = 18;
const GAIN_STEPS_PER_BEL: f32 = 20.0 * 256.0;
/// opus is always decoded at this rate, whatever was recorded
const RATE: u32 = 48000;
/// the longest packet, 120 ms
const MAX_FRAMES: usize = 5760;

/// the packets of an ogg stream, in order
struct Packets {
    input: Input,
    /// the lengths of what is left of the page being read
    segments: VecDeque<u8>,
}

impl Packets {
    /// read the next page's header. false where the stream ends, or stops
    /// being ogg
    fn page(&mut self) -> bool {
        let mut header = [0; PAGE_HEADER];
        if fill(&mut self.input, &mut header) < PAGE_HEADER || !header.starts_with(PAGE_MAGIC) {
            return false;
        }
        let mut lengths = vec![0; header[PAGE_HEADER - 1] as usize];
        let whole = fill(&mut self.input, &mut lengths) == lengths.len();
        self.segments.extend(lengths);
        whole
    }

    /// replace `packet` with the stream's next one, which may lie across
    /// pages. false when there is none
    fn next(&mut self, packet: &mut Vec<u8>) -> bool {
        packet.clear();
        while packet.len() <= MAX_PACKET {
            let Some(length) = self.segments.pop_front().map(usize::from) else {
                if !self.page() {
                    break;
                }
                continue;
            };
            let held = packet.len();
            packet.resize(held + length, 0);
            if fill(&mut self.input, &mut packet[held..]) < length {
                break;
            }
            if length < FULL_SEGMENT {
                return true;
            }
        }
        false
    }
}

pub struct Opus {
    packets: Packets,
    /// boxed: it holds the working memory of both of opus's codecs
    decoder: Box<opuscule::Decoder>,
    channels: usize,
    /// frames still to drop, which the encoder put in to warm up
    skip: usize,
    /// what the file asks every sample to be multiplied by
    gain: f32,
    packet: Vec<u8>,
    /// a packet decoded, in whatever the decoder was built to count in
    pcm: Vec<opuscule::Val>,
}

impl Opus {
    /// read the two packets a file begins with: what it holds, and its tags
    pub fn new(input: Input) -> Result<Self, String> {
        let mut packets = Packets { input, segments: VecDeque::new() };
        let mut head = Vec::new();
        if !packets.next(&mut head) || head.len() < HEAD_LENGTH || !head.starts_with(HEAD_MAGIC) {
            return Err("not an opus file".into());
        }
        let channels = Channels::from_count(head[HEAD_CHANNELS] as usize).filter(|_| head[HEAD_LAYOUT] == 0);
        let channels = channels.ok_or("opus: more than two channels")?;
        let word = |at: usize| [head[at], head[at + 1]];
        let gain = 10f32.powf(i16::from_le_bytes(word(HEAD_GAIN)) as f32 / GAIN_STEPS_PER_BEL);
        let mut tags = Vec::new();
        packets.next(&mut tags);
        Ok(Self {
            packets,
            decoder: Box::new(opuscule::Decoder::new(SampleRate::Hz48000, channels)),
            channels: channels.count(),
            skip: u16::from_le_bytes(word(HEAD_SKIP)) as usize,
            gain,
            packet: Vec::new(),
            pcm: vec![Default::default(); MAX_FRAMES * channels.count()],
        })
    }
}

impl Decoder for Opus {
    fn next(&mut self, samples: &mut Vec<i16>) -> Option<Format> {
        samples.clear();
        while samples.is_empty() {
            if !self.packets.next(&mut self.packet) {
                return None;
            }
            // a damaged packet is left out
            let frames = self.decoder.decode(Some(&self.packet), &mut self.pcm, false).unwrap_or(0);
            let dropped = self.skip.min(frames);
            self.skip -= dropped;
            let kept = &self.pcm[dropped * self.channels..frames * self.channels];
            samples.extend(kept.iter().map(|&sample| sample_to_i16(sample)));
            // few files ask for a gain, and the rest are spared the sums
            if self.gain != 1.0 {
                samples.iter_mut().for_each(|sample| *sample = (*sample as f32 * self.gain) as i16);
            }
        }
        Some(Format { rate: RATE, channels: self.channels })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// a packet that says nothing but what it is: 20 ms of stereo, which
    /// decodes to silence
    const QUIET: &[u8] = &[0xfc];
    const QUIET_FRAMES: usize = 960;

    /// an ogg page of these segments. nothing here reads its other fields
    fn page(segments: &[&[u8]]) -> Vec<u8> {
        let mut page = PAGE_MAGIC.to_vec();
        page.resize(PAGE_HEADER - 1, 0);
        page.push(segments.len() as u8);
        page.extend(segments.iter().map(|segment| segment.len() as u8));
        page.extend(segments.concat());
        page
    }

    fn head(channels: u8, skip: u16, layout: u8) -> Vec<u8> {
        let mut head = [HEAD_MAGIC, &[1, channels], &skip.to_le_bytes()[..], &RATE.to_le_bytes()[..]].concat();
        head.extend([0, 0, layout]);
        head
    }

    fn packets(file: Vec<u8>) -> Vec<Vec<u8>> {
        let mut packets = Packets { input: Box::new(std::io::Cursor::new(file)), segments: VecDeque::new() };
        let (mut found, mut packet) = (Vec::new(), Vec::new());
        while packets.next(&mut packet) {
            found.push(packet.clone());
        }
        found
    }

    #[test]
    fn puts_packets_together_from_segments_and_pages() {
        let long = vec![7; FULL_SEGMENT + 3];
        let file = [page(&[b"one", &long[..FULL_SEGMENT], &long[FULL_SEGMENT..], &[9; FULL_SEGMENT]]), page(&[b"end"])]
            .concat();
        let found = packets(file);
        assert_eq!(found[..2], [b"one".to_vec(), long]);
        assert_eq!((found.len(), found[2].len()), (3, FULL_SEGMENT + 3), "the last began on the page before");
    }

    #[test]
    fn a_stream_ends_where_it_is_cut_or_stops_being_ogg() {
        let mut cut = page(&[b"whole", b"cut short"]);
        cut.truncate(cut.len() - 1);
        assert_eq!(packets(cut), [b"whole".to_vec()]);
        assert_eq!(packets([page(&[b"one"]), b"not a page of anything at all".to_vec()].concat()), [b"one".to_vec()]);
    }

    #[test]
    fn decodes_packets_and_drops_the_encoders_start() {
        let file = [page(&[&head(2, 1000, 0)]), page(&[b"OpusTags"]), page(&[QUIET, QUIET, QUIET])].concat();
        let mut opus = Opus::new(Box::new(std::io::Cursor::new(file))).unwrap();
        let (mut samples, mut frames) = (Vec::new(), Vec::new());
        while let Some(format) = opus.next(&mut samples) {
            assert_eq!(format, Format { rate: RATE, channels: 2 });
            frames.push(samples.len() / 2);
        }
        // the first packet is all dropped, and 40 frames of the second
        assert_eq!(frames, [2 * QUIET_FRAMES - 1000, QUIET_FRAMES]);
    }

    #[test]
    fn refuses_what_it_cannot_play() {
        let open = |file: Vec<u8>| Opus::new(Box::new(std::io::Cursor::new(file))).err();
        assert!(open(page(&[b"OpusHead"])).is_some(), "a head too short to read");
        assert!(open(page(&[&head(6, 0, 1)])).is_some(), "surround");
        assert!(open(b"RIFF".to_vec()).is_some());
    }
}
