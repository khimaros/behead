//! music files as pcm: wav read here, mp3 and flac by their crates, and opus
//! in a module of its own. a decoder hands out one piece of a file at a
//! time, so a track of any length plays from a few kilobytes of memory.

use crate::opus::Opus;
use nanomp3_core::MAX_SAMPLES_PER_FRAME;
use std::io::Read;
use std::path::Path;

/// the kinds of file that play, by the end of their names, in lower case
pub const EXTENSIONS: [&str; 4] = ["mp3", "flac", "opus", "wav"];
/// bits in a sample handed out, whatever the file has
const BITS: u32 = 16;
/// mp3 data held at once. a frame is found by the ones after it, and the
/// decoder's crate asks for this much
const MP3_BUFFER: usize = 16 * 1024;
/// with less than this left, more is read first: the longest frame, twice
const MP3_REFILL: usize = 4 * 1024;
/// what an mp3 file's tags begin with, and how long that beginning is
const ID3_MAGIC: &[u8] = b"ID3";
const ID3_HEADER: usize = 10;
/// set in a tag's flags when it ends with a copy of its header
const ID3_FOOTER: u8 = 0x10;
/// frames of a wav file handed out at once
const WAV_FRAMES: usize = 2048;
const WAV_PCM: u16 = 1;
/// stands for another format, named further on in the chunk
const WAV_EXTENSIBLE: u16 = 0xFFFE;
/// where an extensible format chunk names its real format
const WAV_SUBFORMAT: usize = 24;

/// how the samples of a piece are to be read
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Format {
    pub rate: u32,
    pub channels: usize,
}

/// a file being played
pub trait Decoder: Send {
    /// replace `samples` with the file's next piece, signed 16 bit, the
    /// channels of each frame side by side. None when the file is over
    fn next(&mut self, samples: &mut Vec<i16>) -> Option<Format>;
}

pub type Input = Box<dyn Read + Send>;

/// a decoder for a file, chosen by the end of its name
pub fn open(path: &Path) -> Result<Box<dyn Decoder>, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let extension = path.extension().and_then(|e| e.to_str()).unwrap_or_default().to_ascii_lowercase();
    decoder(&extension, Box::new(file)).map_err(|e| format!("{}: {e}", path.display()))
}

/// a decoder for a stream of one of the kinds in EXTENSIONS
pub fn decoder(extension: &str, input: Input) -> Result<Box<dyn Decoder>, String> {
    match extension {
        "mp3" => Ok(Box::new(Mp3::new(input)?)),
        "flac" => Ok(Box::new(Flac::new(input)?)),
        "opus" => Ok(Box::new(Opus::new(input)?)),
        "wav" => Ok(Box::new(Wav::new(input)?)),
        other => Err(format!("no decoder for .{other}")),
    }
}

fn read_error(e: std::io::Error) -> String {
    format!("read: {e}")
}

/// read until the buffer is full or the stream ends. returns how much came
pub fn fill(input: &mut Input, buffer: &mut [u8]) -> usize {
    let mut filled = 0;
    while filled < buffer.len() {
        match input.read(&mut buffer[filled..]) {
            Ok(0) | Err(_) => break,
            Ok(count) => filled += count,
        }
    }
    filled
}

fn skip(input: &mut Input, bytes: u64) -> Result<(), String> {
    std::io::copy(&mut input.by_ref().take(bytes), &mut std::io::sink()).map(drop).map_err(read_error)
}

struct Mp3 {
    input: Input,
    /// boxed: its 22 KB would otherwise pass over the stack of a small thread
    decoder: Box<nanomp3_core::Decoder>,
    buffer: Vec<u8>,
    /// where in the buffer the next frame is looked for
    start: usize,
    ended: bool,
}

impl Mp3 {
    /// past the tags an mp3 file begins with: a cover picture in there is
    /// megabytes to search for frames, and holds bytes that look like one
    fn new(mut input: Input) -> Result<Self, String> {
        let mut buffer = vec![0; ID3_HEADER];
        let got = fill(&mut input, &mut buffer);
        buffer.truncate(got);
        if buffer.len() == ID3_HEADER && buffer.starts_with(ID3_MAGIC) {
            // the length is four bytes of seven bits each
            let length = buffer[6..].iter().fold(0u64, |length, &byte| length << 7 | (byte & 0x7f) as u64);
            let footer = if buffer[5] & ID3_FOOTER != 0 { ID3_HEADER as u64 } else { 0 };
            skip(&mut input, length + footer)?;
            buffer.clear();
        }
        Ok(Self { input, decoder: Box::default(), buffer, start: 0, ended: false })
    }

    /// keep enough of the file in the buffer for the decoder to find a frame
    fn refill(&mut self) {
        if self.ended || self.buffer.len() - self.start >= MP3_REFILL {
            return;
        }
        self.buffer.drain(..self.start);
        let held = self.buffer.len();
        self.buffer.resize(MP3_BUFFER, 0);
        let got = fill(&mut self.input, &mut self.buffer[held..]);
        self.buffer.truncate(held + got);
        (self.start, self.ended) = (0, held + got < MP3_BUFFER);
    }
}

impl Decoder for Mp3 {
    fn next(&mut self, samples: &mut Vec<i16>) -> Option<Format> {
        samples.resize(MAX_SAMPLES_PER_FRAME, 0);
        loop {
            self.refill();
            if self.start == self.buffer.len() {
                return None;
            }
            let (used, result) = self.decoder.decode(&self.buffer[self.start..], samples.as_mut_slice());
            self.start += used;
            match result {
                Ok(frame) if frame.samples_produced > 0 => {
                    let channels = frame.channels.num() as usize;
                    samples.truncate(frame.samples_produced * channels);
                    return Some(Format { rate: frame.sample_rate, channels });
                }
                // a search that moves on nowhere would never end
                _ if used == 0 => return None,
                _ => {}
            }
        }
    }
}

struct Flac {
    reader: claxon::FlacReader<Input>,
    format: Format,
    bits: u32,
    /// the decoder's own samples, kept from one block to the next
    block: Vec<i32>,
}

impl Flac {
    fn new(input: Input) -> Result<Self, String> {
        let reader = claxon::FlacReader::new(input).map_err(|e| format!("flac: {e}"))?;
        let info = reader.streaminfo();
        let format = Format { rate: info.sample_rate, channels: info.channels as usize };
        Ok(Self { reader, format, bits: info.bits_per_sample, block: Vec::new() })
    }
}

impl Decoder for Flac {
    fn next(&mut self, samples: &mut Vec<i16>) -> Option<Format> {
        // a damaged file ends where the damage is
        let block = self.reader.blocks().read_next_or_eof(std::mem::take(&mut self.block)).ok()??;
        let to_16 = |sample: i32| match self.bits {
            bits if bits >= BITS => (sample >> (bits - BITS)) as i16,
            bits => (sample << (BITS - bits)) as i16,
        };
        samples.clear();
        for frame in 0..block.duration() {
            samples.extend((0..block.channels()).map(|channel| to_16(block.sample(channel, frame))));
        }
        self.block = block.into_buffer();
        Some(self.format)
    }
}

struct Wav {
    input: Input,
    format: Format,
    /// bytes in one sample of one channel
    width: usize,
    /// bytes of sound not yet read
    left: u64,
    bytes: Vec<u8>,
}

impl Wav {
    /// read up to the sound itself: the chunk that says how it is stored,
    /// and past any others
    fn new(mut input: Input) -> Result<Self, String> {
        let mut header = [0; 12];
        input.read_exact(&mut header).map_err(read_error)?;
        if &header[..4] != b"RIFF" || &header[8..] != b"WAVE" {
            return Err("not a wav file".into());
        }
        let mut stored = None;
        loop {
            let mut chunk = [0; 8];
            input.read_exact(&mut chunk).map_err(read_error)?;
            let length = u32::from_le_bytes(chunk[4..].try_into().unwrap()) as u64;
            match &chunk[..4] {
                b"fmt " => {
                    let mut body = vec![0; length as usize];
                    input.read_exact(&mut body).map_err(read_error)?;
                    stored = Some(wav_format(&body)?);
                }
                b"data" => {
                    let (format, width) = stored.ok_or("wav: sound before its format")?;
                    return Ok(Self { input, format, width, left: length, bytes: Vec::new() });
                }
                // chunks are padded to an even length
                _ => skip(&mut input, length + length % 2)?,
            }
        }
    }
}

/// what a wav file's format chunk says: the format, and a sample's bytes
fn wav_format(body: &[u8]) -> Result<(Format, usize), String> {
    let word = |at: usize| body.get(at..at + 2).map(|bytes| u16::from_le_bytes([bytes[0], bytes[1]]));
    let (Some(tag), Some(channels), Some(bits)) = (word(0), word(2), word(14)) else {
        return Err("wav: short format".into());
    };
    let tag = if tag == WAV_EXTENSIBLE { word(WAV_SUBFORMAT).unwrap_or(tag) } else { tag };
    let rate = u32::from_le_bytes(body[4..8].try_into().unwrap());
    if tag != WAV_PCM || channels == 0 || rate == 0 || bits == 0 || bits % 8 != 0 {
        return Err(format!("wav: format {tag} with {bits} bits is not pcm"));
    }
    Ok((Format { rate, channels: channels as usize }, bits as usize / 8))
}

impl Decoder for Wav {
    fn next(&mut self, samples: &mut Vec<i16>) -> Option<Format> {
        let frame = self.width * self.format.channels;
        let wanted = (WAV_FRAMES * frame).min((self.left - self.left % frame as u64) as usize);
        self.bytes.resize(wanted, 0);
        let got = fill(&mut self.input, &mut self.bytes);
        self.left -= got as u64;
        samples.clear();
        // the two most significant bytes of each sample. one of a single
        // byte is unsigned
        samples.extend(self.bytes[..got - got % frame].chunks_exact(self.width).map(|sample| match sample {
            [byte] => (*byte as i16 - 0x80) << 8,
            [.., low, high] => i16::from_le_bytes([*low, *high]),
            [] => 0,
        }));
        (!samples.is_empty()).then_some(self.format)
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// a wav file of these samples, with a chunk before the sound to skip
    pub fn wav(rate: u32, channels: u16, bits: u16, samples: &[u8]) -> Vec<u8> {
        let block = channels * bits / 8;
        let mut file = b"RIFF\0\0\0\0WAVELIST\x03\0\0\0abc\0fmt \x10\0\0\0".to_vec();
        file.extend([WAV_PCM.to_le_bytes(), channels.to_le_bytes()].concat());
        file.extend([rate.to_le_bytes(), (rate * block as u32).to_le_bytes()].concat());
        file.extend([block.to_le_bytes(), bits.to_le_bytes()].concat());
        file.extend(b"data");
        file.extend((samples.len() as u32).to_le_bytes());
        file.extend(samples);
        file
    }

    fn all(extension: &str, file: Vec<u8>) -> (Vec<i16>, Option<Format>) {
        let mut decoder = decoder(extension, Box::new(std::io::Cursor::new(file))).unwrap();
        let (mut every, mut piece, mut format) = (Vec::new(), Vec::new(), None);
        while let Some(found) = decoder.next(&mut piece) {
            every.extend(&piece);
            format = Some(found);
        }
        (every, format)
    }

    #[test]
    fn reads_a_wav_file_past_other_chunks() {
        let sound: Vec<u8> = [1i16, -2, 300, -400].iter().flat_map(|sample| sample.to_le_bytes()).collect();
        let (samples, format) = all("wav", wav(44100, 2, 16, &sound));
        assert_eq!(samples, [1, -2, 300, -400]);
        assert_eq!(format, Some(Format { rate: 44100, channels: 2 }));
    }

    #[test]
    fn reads_wav_samples_of_other_widths() {
        assert_eq!(all("wav", wav(8000, 1, 8, &[0x80, 0xff, 0x00])).0, [0, 0x7f00, -0x8000]);
        assert_eq!(all("wav", wav(8000, 1, 24, &[0xaa, 0x34, 0x12, 0xbb, 0xff, 0xff])).0, [0x1234, -1]);
    }

    #[test]
    fn reads_a_long_wav_file_in_pieces_and_stops_at_a_cut() {
        let mut file = wav(8000, 1, 16, &vec![1; WAV_FRAMES * 2 * 3]);
        assert_eq!(all("wav", file.clone()).0.len(), WAV_FRAMES * 3);
        file.truncate(file.len() - 101);
        assert_eq!(all("wav", file).0.len(), WAV_FRAMES * 3 - 51);
    }

    #[test]
    fn refuses_what_is_not_a_wav_file() {
        assert!(decoder("wav", Box::new(std::io::Cursor::new(b"RIFF....AVI LIST".to_vec()))).is_err());
        assert!(decoder("ogg", Box::new(std::io::empty())).is_err());
    }

    #[test]
    fn an_mp3_file_of_nothing_but_tags_is_over_at_once() {
        let mut file = b"ID3\x04\0\0\0\0\x02\x00".to_vec();
        file.extend(vec![0xff; 256]);
        assert_eq!(all("mp3", file), (vec![], None));
    }
}
