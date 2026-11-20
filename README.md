# behead

an Android Auto server in Rust: the half of Android Auto that normally runs
on a phone. it lets a Raspberry Pi, an ESP32-P4, a Linux phone, or a laptop
project its own interface onto a car headunit over USB or TCP.

status: working in real cars. see [DESIGN.md](DESIGN.md) for architecture
and protocol details.

## headunits

| headunit | status |
|---|---|
| OpenAuto | works |
| 2021 Mercedes Sprinter | works |
| Nissan Micra K14 | works |
| Google DHU 2.0 | works with a Google-signed certificate; refuses self-signed after the handshake |
| Kia EV6 | refuses self-signed after the handshake; Google-signed untried |

a headunit that refuses the self-signed certificate needs a Google-signed
pair extracted from the Android Auto app: see
[docs/certificates.md](docs/certificates.md).

## quick start

install the pinned Rust and Python toolchains and build:

    mise install
    make

then choose a target:

- **try locally**: `make vm && make demo-vm` (OpenAuto in a VM) or
  `make dhu && make demo-dhu` (Google's DHU over TCP). see [try it](#try-it)
- **Raspberry Pi 4**: `make image-rpi SESSION=launcher`, set `MOUNT_DEVICE`
  in `rpi/customize.env`, then `make flash-rpi`. see
  [Raspberry Pi 4](#raspberry-pi-4)
- **ESP32-P4**: `make build-esp32 flash-esp32` with Espressif's toolchain.
  see [ESP32-P4](#esp32-p4)
- **Linux phone or desktop**: run `target/debug/behead` directly with
  `--listen`, `--usb`, or `--functionfs`. see
  [running the server](#running-the-server)

## contents

- [try it](#try-it): OpenAuto VM, Google DHU, and browser headunit
- [sessions](#sessions): launcher, kiosk, Kodi, Phosh, and Waydroid
- [Raspberry Pi 4](#raspberry-pi-4): building, flashing, OTA updates, and media
- [ESP32-P4](#esp32-p4): standalone microcontroller firmware
- [running the server](#running-the-server): CLI options and stdout/stdin event format
- [test](#test): test tiers and headed mode
- [docs/certificates.md](docs/certificates.md): Google-signed certificates and TLS troubleshooting

## try it

### OpenAuto in a VM

    make vm        # once: creates the test VM with OpenAuto inside
    make demo-vm   # runs behead over the VM's USB gadget

`make demo-vm` opens OpenAuto's screen in `gvncviewer` when installed, or in
a browser otherwise (`OPEN=` skips opening it; VNC listens on port 5900).
keyboard controls in OpenAuto: Enter, arrow keys, Escape (back), and `h`
(home). ctrl-c stops the demo.

swap the session with `SESSION=` (`wayland`, `kodi`, `phosh`, `waydroid`,
`launcher`) or pass a custom `VIDEO_CMD=...`:

    make demo-vm SESSION=phosh

stop the demo before running `make test-vm`, as both use the VM's USB gadget.

### Google DHU

runs Google's Desktop Head Unit (DHU) directly on your machine over TCP.
requires `ANDROID_HOME` and a Google-signed certificate
([docs/certificates.md](docs/certificates.md)):

    make dhu       # once: installs the DHU from the Android SDK
    make demo-dhu

the DHU console accepts commands such as `night`, `dpad rotate left`, and
`keycode home` (`help` lists all commands). server logs and input events go
to `target/demo-dhu.log`.

### browser headunit

plug a flashed RPi4 or ESP32-P4 into your laptop over USB and use a browser
tab as the headunit (requires `tests/device/70-behead.rules`):

    make headunit

the mouse acts as a touchscreen; the mouse wheel and Page Up/Down turn the
rotary knob; arrows tilt it; Enter clicks it; Escape is back; Home is home;
other keys type into sessions using `--uinput`. audio plays after the first
click or key press in the page.

### the demo screen

`behead-demo` is the default video source. it displays:

- negotiated resolution, frame rate, and frame counter
- a sweeping bottom bar so frame drops or stutter are visible
- numbered coloured touch markers with drag trails
- touch count, active fingers, rotary knob position, and touchpad contact
- the last six input events (buttons, knob turns, touches, and any unread
  `UNKNOWN` protocol fields or messages with their hex bytes)

## sessions

| session | script | what it shows | notes |
|---|---|---|---|
| `launcher` | `sessions/launcher.sh` | built-in car UI (music, offline maps, inputs) | no compositor needed; same UI as ESP32-P4 |
| `wayland` | `sessions/kiosk.sh` | `behead-demo --wayland` in headless Sway | reference Wayland kiosk |
| `kodi` | `sessions/kodi.sh` | Kodi fullscreen in headless Sway | ALSA loopback audio, offline `.nfo` library |
| `phosh` | `sessions/phosh.sh` | Phosh mobile shell + apps | PulseAudio loopback, geoclue GPS, Flatpaks |
| `waydroid` | `sessions/waydroid.sh` | Android apps via Waydroid | F-Droid + OsmAnd~, GPS mock provider, no audio yet |

### launcher

`behead-launcher` is a lightweight car UI with three tiles: **MUSIC**,
**MAP**, and **INPUTS**. it runs on any Linux machine (`behead-launcher` on
`PATH`), as `SESSION=launcher` on the RPi4, and natively on the ESP32-P4:

    behead --cert phone.crt --key phone.key \
        --video-cmd 'sessions/launcher.sh {width} {height} {fps}' \
        --audio-cmd 'sessions/launcher.sh --audio {rate} {channels}'

`sessions/launcher.sh` plays music from `$BEHEAD_MEDIA/music` (or `~/Music`)
and loads offline maps from `$BEHEAD_MEDIA/maps` (or `~/maps`).

- **MUSIC**: lists folders and MP3, FLAC, Opus, and WAV files by name. tap a
  folder to open or a track to play/pause; tracks advance automatically into
  the next folder. car media keys (play, pause, next, previous) work from any
  screen. audio focus is claimed only while playing and released after two
  seconds of silence so the car radio resumes when paused.
- **MAP**: draws offline roads, water, and street names from `.bmap` files
  around the car's GPS position (display and pan/zoom only, no search or
  routing). build `.bmap` files from OpenStreetMap and Natural Earth data:

      tools/fetch-map.py san-francisco.bmap -122.52,37.70,-122.35,37.83
      tools/fetch-map.py world.bmap world

  `world.bmap` (~0.5 MB) draws coarse coastlines and lakes anywhere on earth
  behind regional maps (San Francisco is < 1 MB, Nashville < 3 MB).
- **INPUTS**: the interactive input test screen from `behead-demo`.
- **without a touchscreen**: the rotary knob moves across tiles/lists and
  zooms the map; d-pad arrows navigate tiles, lists, and pan the map; click
  selects; back leaves a folder or screen; home returns to the tiles.
- **text display**: names render in uppercase ASCII; accented Latin letters
  (European and Vietnamese) are folded to plain ASCII, other scripts show as
  blanks, and long names scroll horizontally.

### kiosk

`sessions/kiosk.sh` runs any single Wayland application fullscreen in a
headless Sway session with `--uinput` touch and button input (requires
`sway`, `wf-recorder`, `x264`, and root for input devices without a login
session):

    behead --usb auto --cert-dir certs --uinput \
        --video-cmd 'sessions/kiosk.sh {width} {height} {fps} behead-demo --wayland'

replace `behead-demo --wayland` with any Wayland app (for example
`kodi --windowing=wayland`).

### Kodi

`sessions/kodi.sh` runs Kodi in the kiosk and configures audio through an
ALSA loopback (`snd-aloop` and `arecord` from `alsa-utils`):

    --video-cmd 'sessions/kodi.sh {width} {height} {fps}' \
    --audio-cmd 'sessions/audio.sh {rate} {channels}'

- on first start, `kodi.sh` selects the ALSA Loopback device and enables
  packaged system add-ons (requires `python3`) so Kodi does not prompt on the
  car screen. if Kodi already has settings, select "Loopback" under
  Settings -> System -> Audio. with PipeWire, record a virtual sink monitor
  using `pw-record` instead of `sessions/audio.sh`.
- media under `$BEHEAD_MEDIA/videos` and `$BEHEAD_MEDIA/music` is added as
  sources and scanned offline using `.nfo` files generated from filenames
  (`Title (Year).mkv`; existing `.nfo` files are preserved).

### Phosh

`sessions/phosh.sh` runs the Phosh mobile shell (requires `phosh`, `phoc`,
`gnome-settings-daemon-common`, `gsettings`, `pulseaudio`, `snd-aloop`, and
`arecord`):

    --video-cmd 'sessions/phosh.sh {width} {height} {fps}' \
    --audio-cmd 'sessions/audio.sh {rate} {channels}'

- **theme and screen**: screen lock and blanking are disabled; dark mode is
  on by default. set `BEHEAD_COLOR_SCHEME` (`light` or `car`, which follows
  the car's night sensor) in `/etc/behead/session.env` on the RPi4.
- **controls**: supports touch (with `squeekboard` on-screen keyboard) and
  rotary-knob-only cars. a long click cycles the knob between focus, arrows,
  map scroll/zoom, and letter typing (shown as an on-screen notification);
  back acts as Escape.
- **bundled RPi4 Flatpaks** (`sessions/depends/phosh.flatpak`):
  - **GNOME Maps** (~1.7 GB): shows car location and pre-fetched offline map
    areas (none unless requested at build time):

        make image-rpi SESSION=phosh MAP_AREAS='Maine=-71.1,43.0,-66.9,47.5;Boston=-71.2,42.2,-70.9,42.5'

  - **CoMaps** (~1.8 GB + regions): offline search and routing. defaults to
    South Bay + Tennessee; override with `COMAPS_REGIONS`:

        make image-rpi SESSION=phosh COMAPS_REGIONS='US_Maine;France_Ile-de-France_Paris'

    list region names with
    `tools/fetch-comaps.py --flatpak ~/.cache/behead/flatpak/aarch64 --list`.
    hold a toolbar button to open its menu; tap the grey strip under the
    buttons for overflow actions including search.
  - **Gapless** (~1.2 GB): plays music from `$BEHEAD_MEDIA`.

### Waydroid

`sessions/waydroid.sh` runs Android inside the kiosk (requires `waydroid`
and the `binder` kernel module):

    --video-cmd 'sessions/waydroid.sh {width} {height} {fps}'

- runs `waydroid init` automatically if needed (reusing pre-placed images in
  `/usr/share/waydroid-extra/images` when present).
- syncs Android night mode and GPS position/accuracy from the car's sensors
  (speed, bearing, and altitude are not forwarded yet; audio is not wired
  yet).
- **apps from F-Droid**: download verified APKs with `tools/fetch-fdroid.py`:

      tools/fetch-fdroid.py net.osmand.plus --abi arm64-v8a
      tools/fetch-fdroid.py org.fdroid.fdroid --abi arm64-v8a

  the session installs APKs placed in `/usr/share/behead/apks` (or
  `$BEHEAD_APKS`), or you can bake them into `system.img` ahead of time
  (requires `e2fsprogs`, no root):

      tools/preinstall-apks.py /usr/share/waydroid-extra/images/system.img --abi arm64-v8a APK...

- to run `waydroid app` commands from outside the session, set
  `BEHEAD_RUNTIME_DIR` and point `DBUS_SESSION_BUS_ADDRESS` at
  `$BEHEAD_RUNTIME_DIR/bus`.

### encoding, GPU and location

- **video encoder**: sessions automatically use hardware encoding
  (`h264_v4l2m2m`, as on the RPi4) at 1280x720 and above when a trial encode
  succeeds, and `x264` below. override with `BEHEAD_ENCODER=x264` or
  `v4l2m2m` (hardware uses less CPU at the cost of 1-2 frames of latency).
- **GPU rendering**: set `BEHEAD_RENDERER=gles2` on machines with a GPU (the
  RPi4 `kodi`, `phosh`, and `waydroid` images set this by default).
- **GPS / location (Linux)**: install `sessions/geoclue.conf` as
  `/etc/geoclue/conf.d/90-behead.conf`, restart `geoclue`, and pass
  `--nmea-socket /run/behead/nmea.sock`. Phosh enables location
  automatically; a bare kiosk needs `allowed=true` and `system=true` for the
  app in geoclue's config.

## Raspberry Pi 4

the RPi4 image is built with [raspi-provision](../raspi-provision) checked
out beside this repository. it boots from a read-only root with a tmpfs
overlay so sudden power loss does not corrupt the SD card.

### build and flash

    make image-rpi SESSION=launcher   # demo if SESSION is omitted
    $EDITOR rpi/customize.env         # set MOUNT_DEVICE to the SD card
    make flash-rpi SESSION=launcher   # DESTRUCTIVE: writes MOUNT_DEVICE

- valid `SESSION` values: `launcher`, `wayland`, `kodi`, `phosh`, `waydroid`.
  add `SESSIONS=...` to install extra sessions alongside the active one (for
  example `make image-rpi SESSION=kodi SESSIONS=phosh`).
- images are written to `rpi/images/<hostname>-<session>.img.gz` (`-demo`
  without `SESSION`). pass the same `SESSION=` to `make flash-rpi` or
  `make ota-rpi`.
- packages are baked at build time (needs `qemu-system-aarch64` >= 11,
  `dtc`, and internet access on the build host; packages are cached for later
  builds).
- reflashing preserves the persist partition (`/home`, `/var/lib/behead`,
  saved Wi-Fi networks, and SSH keys) unless `WIPE=1` is set or the persist
  partition offset changed.

### updating a running RPi4

flash over SSH without removing the SD card (`tests/device/70-behead.rules`
required for USB cable mode):

    make ota-rpi SESSION=launcher     # asks behead to release USB, flashes over 10.55.0.1, reboots
    make release-rpi                  # only releases USB back to g_ether networking (10.55.0.1)
    make deploy-rpi                   # cross-builds, copies the binaries to RAM, restarts service

- pass `OTA_ADDRESS=10.55.1.1` (Ethernet) or `10.55.2.1` (hotspot) to leave
  USB alone.
- `make deploy-rpi` writes into the RAM overlay, so a reboot returns to the
  image's binaries (override target host with `RPI_ADDRESS=...`).
- on the Pi itself, `systemctl stop behead` hands the USB-C port back to
  `g_ether` SSH. to switch sessions on-device, edit `/etc/behead/session.env`
  and run `systemctl restart behead` (run `raspi-provision overlay-commit` to
  persist across reboots).

### media, certificates and power

- **media**: copy videos to `/home/behead/media/videos` and music to
  `/home/behead/media/music` (`BEHEAD_MEDIA`). sessions store their own state
  under `BEHEAD_HOME=/var/lib/behead` (default `~/.behead` off the Pi).
- **Waydroid on RPi4**: `SESSION=waydroid` adds Debian backports, caches
  Android images in `~/.cache/behead/waydroid`, preinstalls F-Droid and
  OsmAnd~, and sets `psi=1` on the kernel command line. if the screen stays
  black, check `waydroid status` on the Pi.
- **certificates**: `/etc/behead/certs` is tried in filename order: extracted
  Google pairs (`50-*`) first, then the generated self-signed pair
  (`90-self-signed`). see
  [docs/certificates.md](docs/certificates.md#on-the-rpi4).
- **power**: some car USB ports cannot supply a Pi 4 under load; power it
  through the GPIO header if it browns out.

## ESP32-P4

`esp32/` contains standalone firmware that turns an ESP32-P4 board into an
Android Auto phone running the launcher through the chip's hardware H.264
encoder at up to 1280x720@30, with music and maps from a FAT32 microSD card:

    make build-esp32                  # needs Espressif's Rust toolchain (espup)
    make flash-esp32                  # flashes the board over USB
    make monitor-esp32                # serial log monitor
    make test-device DEVICE=esp32     # laptop as headunit against the board

useful build variables: `ESP32_PORT=full` (use full-speed USB on GPIO 24/25),
`ESP32_CHIP=v3` (revision v3+ silicon), and `ESP32_MAPS='file1.bmap ...'`
(embed maps in firmware). see [esp32/README.md](esp32/README.md) for board
wiring, card format, and current limitations.

## running the server

    target/debug/behead \
        --cert phone.crt --key phone.key \
        --video-cmd 'ffmpeg -f lavfi -i testsrc2=size={width}x{height}:rate={fps},realtime -pix_fmt yuv420p -c:v libx264 -preset ultrafast -tune zerolatency -profile:v baseline -g {fps} -f h264 -'

### options

**certificates** (at least one required):

- `--cert FILE --key FILE`: phone certificate and private key (PEM). repeat
  to offer multiple pairs in order of preference.
- `--cert-dir DIR`: offer every `NAME.crt` with its `NAME.key` in name order;
  a refused certificate advances to the next on reconnect. see
  [docs/certificates.md](docs/certificates.md).

**media streams**:

- `--video-cmd CMD`: required. shell command writing real-time Annex-B H.264
  to stdout (`{width}`, `{height}`, `{fps}` replaced with the negotiated
  mode). receives input and sensor lines on stdin (non-blocking; unread lines
  are dropped). outlives focus changes and reconnects unless the mode
  changes; the server caches the latest SPS/PPS so hardware encoders that
  emit them once still resume cleanly. for generated ffmpeg sources, use the
  `realtime` filter rather than `-re`.
- `--max-height LINES`: cap negotiated video height to the largest mode the
  headunit offers within `LINES`.
- `--audio-cmd CMD`: shell command writing real-time signed 16-bit little
  endian PCM to stdout (`{rate}`, `{bits}`, `{channels}`, usually 48000 Hz
  stereo). an empty value counts as none. audio focus is claimed only when
  non-silent audio arrives and released after two seconds of silence so the
  car radio stays active otherwise.
- `--audio-delay MS`: delay outgoing media audio to match video encoding and
  decoding latency (on the RPi4, set `AUDIO_DELAY` in
  `/etc/behead/session.env`).
- `--mic-cmd CMD`: shell command reading headunit microphone PCM on stdin
  (usually 16000 Hz mono). opens only on the car's voice key (84) and closes
  when pressed again or when the command exits.

**transport** (defaults to `--listen 127.0.0.1:5277`):

- `--listen ADDR`: TCP address to accept a headunit on (unless `--usb` or
  `--functionfs` is given).
- `--usb UDC`: create a Linux USB gadget on the named controller from
  `/sys/class/udc` (or `auto` for the first one). needs root. removes the
  gadget on `SIGINT`/`SIGTERM`; if killed uncleanly, run `behead teardown`.
- `--functionfs DIR`: attach to an existing FunctionFS mount owned by another
  daemon, such as
  [usb-signaller](https://codeberg.org/DylanVanAssche/usb-signaller) on a
  Linux phone. the owner sets accessory IDs `18d1:2d00` and binds the UDC
  once `behead` writes descriptors; `behead` leaves the gadget alone on exit.
  implies `--usb auto` (pass `--usb UDC` if the host has multiple UDCs).

**input and sensors**:

- `--uinput`: create kernel input devices (`behead touchscreen`,
  `behead keys`, and `behead knob` when scroll mode is enabled) via
  `/dev/uinput`. map the touchscreen to the headless output in your
  compositor (for Sway:
  `input "0:0:behead_touchscreen" map_to_output HEADLESS-1`).
- `--knob KEYS`: rotary knob mapping for `--uinput`: `focus` (Tab /
  Shift+Tab, default), `arrows` (Down / Up), `scroll` (centred mouse wheel),
  `type` (letter-by-letter entry: click keeps letter or sends Enter, back
  deletes). pass a comma-separated list (e.g. `focus,arrows,scroll,type`) to
  cycle modes on long click (`knob <mode>` is emitted on stdout/stdin). set
  `KNOB` in `/etc/behead/behead.env` on the RPi4.
- `--back KEY`: back button mapping for `--uinput`: `back` (default) or
  `escape`. set `BACK` in `/etc/behead/behead.env` on the RPi4.
- `--nmea-socket PATH`: publish car GPS fixes as NMEA (`GPGGA` and `GPRMC`)
  on a Unix socket for location services such as `geoclue`.

### input and sensor lines

events from the headunit are written to stdout and to `--video-cmd`'s stdin
(when `--video-cmd` starts, it also receives the latest reading of each
sensor seen before startup):

    touch down 0 0:123,456
    touch pointer-down 1 0:123,456 1:300,200
    touch move 1 0:123,456 1:310,210
    touch pointer-up 1 0:123,456 1:310,210
    touch up 0 0:123,456
    button 3 down
    relative 65536 -1
    absolute 65536 40
    touchpad down 0 0:400,300
    unknown input 1 field 9 0102
    unknown message 1 0x80f9 abcd
    location 52.5200066 13.404954 5 34.5 12.5 90.5
    night 1
    driving 0
    speed 27.78

- **touch / touchpad**: `<touch|touchpad> <action> <action-index> <id>:<x>,<y> ...`
  listing every active pointer (`touchpad` uses the pad's own coordinates).
- **buttons and controls**: Android key codes (`button <code> <down|up>`).
  rotary knob code is `65536` (`relative 65536 <detents>`); tilts are d-pad
  `19..22`, click is `23`. `absolute <code> <value>` reports sliders.
- **unknowns**: unread input/sensor fields (`unknown <input|sensor> <ch> field <num> <hex>`)
  and unhandled messages (`unknown message <ch> <id> <hex>`, up to 32 bytes)
  are printed so new car controls can be inspected. stderr logs each input
  channel's advertised keys and surfaces on connect.
- **sensors** (missing values print as `-`):
  - `location <lat> <lon> <accuracy> <altitude> <speed> <bearing>` (degrees,
    metres, speed, degrees)
  - `compass <bearing> <pitch> <roll>` (degrees), `speed <speed>` (raw/1000:
    m/s per Google's schema, knots in OpenAuto)
  - `night <0|1>`, `parking-brake <0|1>`
  - `driving <bits>` (`0` unrestricted; `1` no video, `2` no keyboard, `4` no
    voice, `8` no setup, `16` short messages)
  - `gear <n>` (`0` neutral, `1..10` gears, `100` drive, `101` park, `102`
    reverse)
  - `rpm`, `odometer <total> <trip>`, `fuel <level> <range> <low>`,
    `environment <temp> <pressure> <rain>` (raw values; units unconfirmed)

## test

    make precommit                 # fmt, clippy, unit tests, and TCP e2e tests
    make test-e2e                  # TCP e2e tests (no root)
    make vm && make test-vm        # USB gadget, RPi4 service, OpenAuto, and desktop tests in VM
    make test-dhu                  # Google's DHU over TCP (after `make dhu`)
    make test-device               # laptop as headunit against a cabled RPi4
    make test-device DEVICE=esp32  # laptop as headunit against a cabled ESP32-P4

tests run headless by default. pass `HEADED=1` to watch live in a browser
(`OPEN=` to skip opening it) and `TESTS=` to filter:

    make test-vm HEADED=1 TESTS=test_phosh

- `make test-device` requires `tests/device/70-behead.rules` on the laptop
  and reports touch-to-picture latency. pass `DEVICE=esp32 SOAK=600` to soak
  test ESP32-P4 audio and video for 600 seconds.
- three certificate tests check against Google's Automotive Link root and
  skip unless env vars are provided: see
  [docs/certificates.md](docs/certificates.md#testing-against-googles-root).
