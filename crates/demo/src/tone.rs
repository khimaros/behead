//! a steady test tone as signed 16 bit little endian pcm, for a demo with no
//! other sound to send to the car.

use std::f32::consts::TAU;

/// the a above middle c: small speakers play it, and it is not the hum of
/// a bad ground
const FREQUENCY: f32 = 440.0;
/// a tenth of full scale, so that it tests the car's sound and not its owner
const AMPLITUDE: f32 = i16::MAX as f32 / 10.0;

/// where the tone stands, so that one piece of it continues the last
#[derive(Default)]
pub struct Tone {
    /// how far through its cycle the next sample is, from 0 to 1
    phase: f32,
}

impl Tone {
    /// the next `frames` samples of the tone, the same on every channel
    pub fn pcm(&mut self, rate: u32, channels: u32, frames: usize) -> Vec<u8> {
        let mut pcm = Vec::with_capacity(frames * channels as usize * size_of::<i16>());
        for _ in 0..frames {
            let sample = ((self.phase * TAU).sin() * AMPLITUDE) as i16;
            (0..channels).for_each(|_| pcm.extend_from_slice(&sample.to_le_bytes()));
            self.phase = (self.phase + FREQUENCY / rate.max(1) as f32).fract();
        }
        pcm
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48000;

    fn left(pcm: &[u8], channels: usize) -> Vec<i16> {
        pcm.chunks_exact(2 * channels).map(|frame| i16::from_le_bytes([frame[0], frame[1]])).collect()
    }

    #[test]
    fn plays_its_frequency_across_pieces() {
        let mut tone = Tone::default();
        // a second of it, in pieces that do not end where a cycle does
        let pcm: Vec<u8> = (0..25).flat_map(|_| tone.pcm(RATE, 2, RATE as usize / 25)).collect();
        let samples = left(&pcm, 2);
        let rises = samples.windows(2).filter(|pair| pair[0] < 0 && pair[1] >= 0).count();
        assert!((rises as f32 - FREQUENCY).abs() <= 1.0, "{rises} cycles in a second");
    }

    #[test]
    fn is_the_same_on_every_channel_and_quiet() {
        let pcm = Tone::default().pcm(RATE, 2, 480);
        assert!(pcm.chunks_exact(4).all(|frame| frame[..2] == frame[2..]));
        let peak = left(&pcm, 2).into_iter().map(i16::unsigned_abs).max().unwrap();
        assert!((3000..=3277).contains(&peak), "peak {peak}");
    }
}
