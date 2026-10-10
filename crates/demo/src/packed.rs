//! a surface in the form the esp32-p4's h.264 encoder reads: yuv 4:2:0, with
//! every pair of pixels as a colour byte and two of luma, the colour being
//! the blue difference on the first row of each two and the red difference
//! on the second. drawing straight into it spares the chip turning an rgb
//! picture into this, which takes it longer than drawing and encoding.

use crate::{clip, Surface};

/// bt.601 studio range, in 1/256ths: luma, and the two colour differences
const LUMA: ([i32; 3], i32) = ([66, 129, 25], 16);
const BLUE_DIFFERENCE: ([i32; 3], i32) = ([-38, -74, 112], 128);
const RED_DIFFERENCE: ([i32; 3], i32) = ([112, -94, -18], 128);
/// bytes for a pair of pixels: colour, then the luma of each
const PAIR: usize = 3;

fn component(colour: [u8; 3], (weights, offset): ([i32; 3], i32)) -> u8 {
    let sum: i32 = colour.iter().zip(weights).map(|(&value, weight)| value as i32 * weight).sum();
    (((sum + 128) >> 8) + offset) as u8
}

/// the colour byte of a row: blue difference on even rows, red on odd
fn colour_byte(row: usize, colour: [u8; 3]) -> u8 {
    component(colour, if row.is_multiple_of(2) { BLUE_DIFFERENCE } else { RED_DIFFERENCE })
}

/// a picture of even width and height in the encoder's form, top row first,
/// in memory that is someone else's
pub struct Packed<'a> {
    width: u32,
    height: u32,
    bytes: &'a mut [u8],
}

impl<'a> Packed<'a> {
    /// `bytes` holds three bytes for every two pixels, or more
    pub fn new(width: u32, height: u32, bytes: &'a mut [u8]) -> Self {
        let used = width as usize / 2 * PAIR * height as usize;
        Self { width, height, bytes: &mut bytes[..used] }
    }

    fn row_bytes(&self) -> usize {
        self.width as usize / 2 * PAIR
    }
}

impl Surface for Packed<'_> {
    fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// a row at a time, from the two kinds of row made beside the picture
    fn fill(&mut self, colour: [u8; 3]) {
        let (luma, pairs) = (component(colour, LUMA), self.width as usize / 2);
        let rows = [0, 1].map(|row| [colour_byte(row, colour), luma, luma].repeat(pairs));
        let row_bytes = self.row_bytes().max(1);
        self.bytes
            .chunks_exact_mut(row_bytes)
            .enumerate()
            .for_each(|(row, bytes)| bytes.copy_from_slice(&rows[row % 2]));
    }

    /// colour is kept for squares of four pixels, so a rectangle colours
    /// every square it touches, and an edge at an odd place carries its
    /// colour a pixel further than its luma
    fn rect(&mut self, x: i64, y: i64, width: u32, height: u32, colour: [u8; 3]) {
        let (columns, rows) = (clip(x, width, self.width), clip(y, height, self.height));
        if columns.is_empty() || rows.is_empty() {
            return;
        }
        let (luma, row_bytes) = (component(colour, LUMA), self.row_bytes());
        for row in rows.clone() {
            let start = row * row_bytes;
            columns.clone().for_each(|column| self.bytes[start + column / 2 * PAIR + 1 + column % 2] = luma);
        }
        let pairs = columns.start / 2..columns.end.div_ceil(2);
        for row in rows.start / 2 * 2..rows.end.div_ceil(2) * 2 {
            let (start, byte) = (row * row_bytes, colour_byte(row, colour));
            pairs.clone().for_each(|pair| self.bytes[start + pair * PAIR] = byte);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WHITE: [u8; 3] = [255, 255, 255];
    const RED: [u8; 3] = [255, 0, 0];

    #[test]
    fn packs_what_is_drawn_for_the_esp32_encoder() {
        let mut bytes = [0; 12];
        let mut picture = Packed::new(4, 2, &mut bytes);
        picture.fill(WHITE);
        picture.rect(2, 0, 2, 2, RED);
        // white then red: no colour then little blue on the first row, much red on the second
        assert_eq!(bytes, [128, 235, 235, 90, 82, 82, 128, 235, 235, 240, 82, 82]);
    }

    #[test]
    fn one_pixel_colours_its_square_of_four() {
        let mut bytes = [0; 12];
        let mut picture = Packed::new(4, 2, &mut bytes);
        picture.fill(WHITE);
        picture.rect(3, 1, 1, 1, RED);
        // the luma of one pixel, and both colour bytes of the square it is in
        assert_eq!(bytes, [128, 235, 235, 90, 235, 235, 128, 235, 235, 240, 235, 82]);
    }

    #[test]
    fn a_rectangle_off_the_picture_draws_nothing() {
        let mut bytes = [7; 12];
        Packed::new(4, 2, &mut bytes).rect(4, 0, 2, 2, RED);
        Packed::new(4, 2, &mut bytes).rect(-2, -2, 2, 2, RED);
        assert_eq!(bytes, [7; 12]);
    }

    #[test]
    fn leaves_room_beyond_the_picture_alone() {
        let mut bytes = [7; 14];
        Packed::new(4, 2, &mut bytes).fill(WHITE);
        assert_eq!(bytes[12..], [7, 7]);
    }
}
