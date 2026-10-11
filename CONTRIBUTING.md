# contributing

## toolchain

`mise install` provides the pinned rust and python. the end-to-end tests also
need `openssl`, `ffmpeg` with libx264, and `ffprobe` on the path.

## commands

- `make`: build
- `make test`: rust unit tests
- `make test-e2e`: python end-to-end tests against the built binary
- `make precommit`: format check, clippy, unit and tcp end-to-end tests. run
  before handing work over.
- `make vm`: create and provision the incus test vm from `incant.yaml`
- `make test-vm`: usb gadget tests inside that vm. run after touching
  `gadget.rs`, the transports, or the session flow.
- `make build-rpi`, `make test-e2e-rpi`: cross-compile for the rpi4 and run
  the tcp tests against that binary under qemu user emulation. needs clang.
- `make image-rpi`: build the sdcard image. needs `../raspi-provision` and
  its prerequisites (libguestfs-tools, xz-utils). `make flash-rpi` writes
  it to the sdcard, and `make ota-rpi` to the card inside a running pi,
  over its usb cable. `make release-rpi` asks the pi for that port alone.
- `make test-device`: device tests against an rpi4 on a usb cable, or with
  `DEVICE=esp32` against an esp32-p4 running the firmware.
- `make headunit`: the browser as the headunit of the device on the cable.

a target is named for what it does, then for the device it does it to:
`build-rpi`, `flash-esp32`. one that works on any device has none in its
name, and takes `DEVICE=` where it has to know.

## layout

- `crates/aap`: protocol core. keep it `no_std` and free of io.
- `crates/link`: a headunit connection with threads and nothing else of its
  host, shared by the linux server and the firmware. keep it free of
  dependencies beyond `aap`.
- `crates/server`: linux binary
- `crates/demo`: `behead-demo`, the interactive demo video source, and the
  library with what it draws, which the firmware uses too. keep it free of
  dependencies beyond `libc`, which the wayland client needs for file
  descriptor passing and the server already uses.
- `crates/launcher`: `behead-launcher`, the home screen with its music
  player, and the library with all of it, which the firmware shows. it has
  to build for the esp32-p4, so its dependencies are plain rust and it
  reaches the host only through `std`.
- `esp32`: firmware for the esp32-p4. not a workspace member: `make
  build-esp32` builds it with espressif's toolchain, and `make precommit`
  does not cover it. code that can be tested on linux belongs in the
  crates above, since nothing in here can be.
- `sessions`: video commands that run a desktop session on the headunit.
  one that `make image-rpi SESSION=` and `make demo-vm SESSION=` can choose is
  `NAME.sh {width} {height} {fps}`, listed in `SESSION_NAMES` in the
  `Makefile`, with its packages in `sessions/depends/NAME`.
- `tools`: developer scripts, run by hand. python and the `openssl` command
  only, so they need no build step and no venv.
- `tests/e2e`: python tests. `fakehu.py` is a headunit written independently
  of the rust code; do not share constants or generated code between them,
  since the point is two readings of the protocol checking each other.
  `test_certs.py` runs `make certs-rpi` against temporary directories, and
  `test_rpi_sessions.py` does the same with `make sessions-rpi`.
  `test_launcher.py` works `behead-launcher` as a car's driver would.
  `test_session_home.py` runs `sessions/kodi.sh` with a stand-in kiosk.
- `tests/vm`: `test_usb.py` subclasses the tcp tests, so every session test
  also runs over usb. it needs pyusb, which the vm provisioning installs.
  `test_uinput.py` does the same over tcp with `--uinput`, and reads the
  resulting kernel input devices back through evdev. its demo test runs
  `behead-demo --wayland` in `sessions/kiosk.sh`, which needs the sway and
  wf-recorder the vm provisioning installs. `test_kodi.py` runs kodi there,
  playing a generated clip into an alsa loopback that `--audio-cmd` records,
  and `test_phosh.py` taps an app open in `sessions/phosh.sh`. both build on
  `desktop.py`, which runs the server with a session and waits for it.
- `tests/e2e/viewer.py`: the browser view behind `HEADED=1`. use it to watch
  a test, or fetch `/frame.jpg` to see what the headunit got.
- `tests/device`: tests against a device on a usb cable, with pyusb
  supplied by `uv run --with`: `test_device.py` for any device, which a pi
  passes as it is, and `test_firmware.py` for the esp32-p4's launcher.
- `rpi`: the raspi-provision project for the rpi4 image.

`mise.toml` stays in the plain `tool = "version"` form. richer entries make
mise require an explicit trust step, which breaks its shims until done.

## testing

behaviour that can be observed from outside the process is tested end-to-end.
unit tests are for pure functions with awkward edge cases (framing, stream
splitting). e2e tests use only the python standard library.

## references

- AACS, a c++ phone side implementation: `../../tomasz-grobelny/AACS`
- aasdk, the headunit side library under openauto: https://github.com/f1xpl/aasdk

both are GPLv3. message ids and field numbers here were taken from reading
them, which is why this project is GPL-3.0-only as well.

## dependencies

each new dependency needs a reason. current ones: `prost` (protobuf codec),
`rustls` with `ring` (tls), `libc` (mount and signal handling for the gadget),
and for the launcher's music `nanomp3-core` (mp3), `claxon` (flac) and
`opuscule` (opus), each without dependencies of its own. opuscule was
chosen over `opus-decoder` by measuring: on a laptop it decodes 300 times
faster than the music plays, the other 3 times.
