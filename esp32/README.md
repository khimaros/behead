# behead on the esp32-p4

firmware that makes an esp32-p4 the phone: plugged into a car's usb port,
its high speed usb port, it shows the launcher on the headunit, encoded by
the chip's h.264 encoder, plays the music on its microsd card as the car's
sound, and is worked by the car's touch, buttons and knob.

STATUS: it runs on a board (waveshare esp32-p4-module-dev-kit, chip v1.3,
2026-10-10), over the FULL speed port, with the laptop as headunit
(`make test-device DEVICE=esp32`, which passes): the accessory switch, tls, video at
800x480 and at 1280x720, both 30 pictures a second, a reconnect after a bus
reset, and touch to picture in about 40 ms. a headunit asking for more than
720 lines gets the largest it offers within them. an earlier build has
shown the demo in a van.
a card of opus files mounts and plays, at the speed it is heard beside the
pictures, as the laptop measures it. NOT yet heard by anyone, and mp3,
flac and wav have played on linux only. with the slot empty the firmware
says NO CARD and goes on.
the high speed port builds and has had no host yet.

## the card

fat32, which a card over 32 GB has to be formatted as: esp-idf reads no
exfat. mp3, flac, opus and wav files, in folders up to eight deep, listed and
played in the order of their paths. the card is read once, when the board
starts, so it has to be in by then. it is on the chip's first sd slot, four
data lines wide, on gpio 39 to 44, with the pins fed by the chip's fourth
regulator.

## building

    make build-esp32     # from the repository root
    make flash-esp32     # write it to a board on usb
    make monitor-esp32   # show what the board logs

`ESP32_PORT=full` with the first two puts the phone on the chip's full speed
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

the phone certificate is built in: `make certs-esp32` copies the pair
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
- `src/card.rs`: the microsd card, mounted as files under `/sdcard`.
- `src/main.rs`: the connection loop, from `crates/link`, and the
  launcher, from `crates/launcher`, both shared with linux. the launcher
  and what plays outlive a connection. it draws straight into the
  encoder's yuv (`crates/demo/src/packed.rs`): making that of an rgb
  picture took longer than drawing and encoding together, on the cpu and
  in the chip's pixel accelerator alike. one thread draws a picture while
  the encoder is at the one before (the demo at 1280x720: 10 ms drawing,
  16 encoding), and another decodes the music a packet ahead of the car.

## not done

- the usb controller's dma, which corrupted the stream (sdkconfig.defaults).
  untried since everything that starts a usb transfer was put on one core,
  which may have been its trouble too.
- the microphone and the car's sensors: the session leaves them unopened.
- leaving the car its own sound while nothing plays: the firmware takes
  the car's audio focus as soon as it connects, so the radio stops for a
  launcher that is silent.
- a card put in, or changed, while the board runs.
- m4a files, which a card may hold beside the kinds that play.
- room to spare for opus: it decodes at 1.6 times the speed it is heard
  while the screen is still, and falls behind beside the input test,
  whose picture is drawn anew every time (see WORKING.md).
- tags, covers, shuffle, and moving within a track.
- 1080 lines: the encoder takes whole macroblocks, and the stream would
  have to say what to crop.
- a certificate list, tried in turn as the linux server does.
