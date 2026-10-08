//! the chip's h.264 encoder, through espressif's esp_h264 component: one
//! picture in, as packed yuv 4:2:0, one baseline access unit out.

use aap::VideoMode;
use behead_demo::Canvas;
use esp_idf_svc::sys;

/// the encoder works in macroblocks of this many pixels a side
const MACROBLOCK: u32 = 16;
/// what the encoder's dma wants its buffers aligned to
const ALIGNMENT: u32 = 64;
/// bits per pixel and second, in hundredths, as for the pi's encoder
const CENTIBITS: u32 = 35;
const QP_MIN: u8 = 20;
const QP_MAX: u8 = 40;
/// room for one access unit, as a share of the raw picture. a first picture
/// of flat colour and text is far smaller
const OUTPUT_SHARE: u32 = 2;

fn check(what: &str, status: sys::esp_h264_err_t) -> Result<(), String> {
    (status == sys::esp_h264_err_t_ESP_H264_ERR_OK).then_some(()).ok_or(format!("encoder: {what} failed ({status})"))
}

/// a buffer the encoder's dma can reach, in the psram
struct Buffer {
    data: *mut u8,
    length: u32,
}

impl Buffer {
    fn new(length: u32) -> Result<Self, String> {
        let mut actual = 0;
        // SAFETY: plain allocation; the result is checked below
        let data = unsafe { sys::esp_h264_aligned_calloc(ALIGNMENT, 1, length, &mut actual, sys::ESP_H264_MEM_SPIRAM) };
        (!data.is_null())
            .then_some(Self { data: data.cast(), length: actual })
            .ok_or(format!("encoder: no room for {length} bytes"))
    }

    fn bytes(&mut self) -> &mut [u8] {
        // SAFETY: the allocation is `length` bytes, and owned by this buffer
        unsafe { std::slice::from_raw_parts_mut(self.data, self.length as usize) }
    }
}

impl Drop for Buffer {
    fn drop(&mut self) {
        // SAFETY: allocated by the heap in new, and freed once
        unsafe { sys::heap_caps_free(self.data.cast()) };
    }
}

pub struct Encoder {
    handle: sys::esp_h264_enc_handle_t,
    input: Buffer,
    output: Buffer,
}

// SAFETY: the handle is used by whichever one thread owns the encoder
unsafe impl Send for Encoder {}

impl Encoder {
    /// an encoder for one video mode, with a keyframe every second. the
    /// encoder takes whole macroblocks, so 1080 lines, which are not, would
    /// need the stream to say what to crop
    pub fn new(mode: VideoMode) -> Result<Self, String> {
        if mode.width % MACROBLOCK != 0 || mode.height % MACROBLOCK != 0 {
            return Err(format!("encoder: {}x{} is not whole macroblocks", mode.width, mode.height));
        }
        let picture = mode.width * mode.height * 3 / 2;
        let (input, output) = (Buffer::new(picture)?, Buffer::new(picture / OUTPUT_SHARE)?);
        let config = sys::esp_h264_enc_cfg_hw_t {
            pic_type: sys::esp_h264_raw_format_t_ESP_H264_RAW_FMT_O_UYY_E_VYY,
            gop: mode.fps as u8,
            fps: mode.fps as u8,
            res: sys::esp_h264_resolution_t { width: mode.width as u16, height: mode.height as u16 },
            rc: sys::esp_h264_enc_rc_t {
                bitrate: mode.width * mode.height * mode.fps * CENTIBITS / 100,
                qp_min: QP_MIN,
                qp_max: QP_MAX,
            },
        };
        let mut handle = std::ptr::null_mut();
        // SAFETY: the config and the handle outlive the calls, and the handle is checked before use
        unsafe {
            check("creating", sys::esp_h264_enc_hw_new(&config, &mut handle))?;
            check("opening", sys::esp_h264_enc_open(handle))?;
        }
        Ok(Self { handle, input, output })
    }

    /// compress one picture, which has the mode's size. returns the access
    /// unit, in annex-b, which lasts until the next call
    pub fn encode(&mut self, canvas: &Canvas, timestamp_ms: u32) -> Result<&[u8], String> {
        canvas.pack_yuv420(self.input.bytes());
        let mut input = sys::esp_h264_enc_in_frame_t {
            raw_data: sys::esp_h264_pkt_t { buffer: self.input.data, len: self.input.length },
            pts: timestamp_ms,
        };
        // SAFETY: plain data, valid when zeroed
        let mut output: sys::esp_h264_enc_out_frame_t = unsafe { std::mem::zeroed() };
        output.raw_data = sys::esp_h264_pkt_t { buffer: self.output.data, len: self.output.length };
        // SAFETY: the encoder is open, and both frames point at buffers it can reach
        check("encoding", unsafe { sys::esp_h264_enc_process(self.handle, &mut input, &mut output) })?;
        Ok(&self.output.bytes()[..output.length as usize])
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        // SAFETY: the handle was opened in new, and is closed once
        unsafe {
            sys::esp_h264_enc_close(self.handle);
            sys::esp_h264_enc_del(self.handle);
        }
    }
}
