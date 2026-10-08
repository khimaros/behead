// what the firmware calls beyond the headers esp-idf-sys binds by itself:
// tinyusb's device api, and the driver for the chip's h.264 encoder
#include "tusb.h"
#include "esp_h264_enc_single.h"
#include "esp_h264_enc_single_hw.h"
#include "esp_h264_alloc.h"
