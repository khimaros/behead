# behead on the esp32-p4

firmware that makes an esp32-p4 the phone: plugged into a car's usb port,
its high speed usb port, it shows the demo on the headunit, encoded by the
chip's h.264 encoder, and reacts to the car's touch, buttons and knob.

STATUS: it runs on a board (waveshare esp32-p4-module-dev-kit, chip v1.3,
2026-10-09), over the FULL speed port, with the laptop as headunit
(`make test-rpi`, which passes): the accessory switch, tls, video at 800x480
and at 1280x720, both 30 pictures a second, a reconnect after a bus reset,
touch to picture in about 45 ms, and a test tone as media audio. a
headunit asking for more than 720 lines gets the largest it offers within
them. an earlier build has shown the demo in a van.
the high speed port builds and has had no host yet.

## building

    make build-esp32     # from the repository root
    make esp32-flash     # write it to a board on usb, and show its log

`ESP32_PORT=full` with either puts the phone on the chip's full speed
controller in place of its high speed port, on the pins of the chip's debug
port (gpio 24 and 25), which is then no debug port. for a board whose high
speed port cannot reach a host: on the waveshare kit that port is a type a
socket, set to DEVICE by the jumpers beside it, which takes a cable with a
type a plug at both ends. the log is on the uart either way.

the build needs espressif's rust toolchain, which `espup install` sets up
once, and the tools in `mise.toml` here. the first build fetches esp-idf
v5.4.1 and its compilers into `.embuild`, about 5 GB, and takes a while.
`make clean` removes that along with the build, so the next build fetches
it again.

the phone certificate is built in: `make esp32-certs` copies the pair
../dexter left in ~/.cache/behead/certs, or the one named by `PHONE_CERT`
and `PHONE_KEY`, and makes a self-signed one when there is neither. unlike
the linux server the firmware holds one certificate, not a list to try.

## what is where

- `src/usb.rs`: the usb device on tinyusb, with the android open accessory
  handshake from `aap::accessory`. the chip leaves the bus and comes back
  under google's ids when the headunit asks.
- `src/tls.rs`: the session's tls on the mbedtls that comes with esp-idf.
  rustls's crypto does not build for 32 bit risc-v.
- `src/encoder.rs`: espressif's `esp_h264` driver for the hardware encoder.
- `src/main.rs`: the connection loop, from `crates/link`, and the demo,
  from `crates/demo`, both shared with the linux server. the demo draws
  straight into the encoder's yuv (`crates/demo/src/packed.rs`): making
  that of an rgb picture took longer than drawing and encoding together,
  on the cpu and in the chip's pixel accelerator alike. one thread draws a
  picture while the encoder is at the one before: at 1280x720 drawing
  takes 10 ms and encoding 16.

## not done

- the usb controller's dma, which corrupted the stream (sdkconfig.defaults).
- the microphone and the car's sensors: the session leaves them unopened.
  of sound there is a test tone, 440 hz and quiet, as the car's media
  audio for as long as the car shows the demo. it takes the car's audio
  focus, so the radio stops for it.
- 1080 lines: the encoder takes whole macroblocks, and the stream would
  have to say what to crop.
- a certificate list, tried in turn as the linux server does.
