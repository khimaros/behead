# behead on the esp32-p4

firmware that makes an esp32-p4 the phone: plugged into a car's usb port,
its high speed usb port, it shows the demo on the headunit, encoded by the
chip's h.264 encoder, and reacts to the car's touch, buttons and knob.

STATUS: it builds and links. it has NEVER run on a board. everything below
the session core is written against the vendor's headers and sources
without hardware to try it on.

## building

    make build-esp32     # from the repository root
    make esp32-flash     # write it to a board on usb, and show its log

the build needs espressif's rust toolchain, which `espup install` sets up
once, and the tools in `mise.toml` here. the first build fetches esp-idf
v5.4.1 and its compilers into `.embuild`, about 5 GB, and takes a while.

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
  from `crates/demo`, both shared with the linux server.

## not done

- sound, the microphone and the car's sensors: the session leaves them
  unopened.
- 1080 lines: the encoder takes whole macroblocks, and the stream would
  have to say what to crop.
- a certificate list, tried in turn as the linux server does.
- reads and writes poll tinyusb each millisecond instead of waiting on it.
