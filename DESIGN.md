# design

## roles

android auto has two ends. the car headunit is the usb host, the tls client,
and the display. the phone is the usb device, the tls server, and the source
of video. this project implements the phone end.

    application -> h.264 encoder -> behead (this project) -> transport -> car
    application <- input events  <- behead (this project) <- transport <- car

## crates

`crates/aap` is the protocol core. it is `no_std` and does no io: bytes from
the transport go into `Session::receive`, bytes for the transport come out of
`Session::take_output`. tls sits behind the `Tls` trait in memory-bio style.
this is what lets one core serve linux hosts and microcontrollers.

- `frame.rs`: wire framing and reassembly from arbitrary read boundaries
- `proto.rs`: message ids and protobuf messages, written by hand with prost
  derive so there is no build script and no protoc dependency
- `session.rs`: the phone side state machine
- `h264.rs`: annex-b access unit splitting

`crates/server` is the linux binary, `behead`. it owns the transport, the
rustls endpoint, the encoder child process, and input delivery.

## session flow

1. headunit sends a version request, we answer with a version response
2. tls 1.2 handshake carried inside plain frames on channel 0. we request the
   headunit certificate and accept any. the phone certificate comes from an
   ordered list: a tls 1.2 ClientHello carries nothing that says which
   authority the headunit trusts, so a handshake that starts and never
   finishes moves the next connection on to the next certificate, wrapping
   around. an accepted certificate stays in use
3. headunit sends auth complete, we send a service discovery request
4. from the service discovery response we open the video and input channels,
   and the media audio and microphone channels when there is a command for
   them
5. video: setup request (h.264 baseline), wait for setup response and video
   focus, send start, then one access unit per media message
6. input: binding request for the advertised keycodes, then input events arrive
7. media audio: setup request (pcm), then an audio focus request on the
   control channel. start once the headunit grants focus, stop when it takes
   focus back, which it may do unprompted
8. microphone: setup request (pcm), then a microphone open request. the
   headunit then sends timestamped pcm, each packet acked

every outgoing protobuf scalar is written even when zero, because headunits
parse these messages as proto2 with required fields.

## video

the server runs a configurable shell command that writes annex-b h.264 to
stdout, with `{width}`, `{height}` and `{fps}` replaced by the mode the
headunit negotiated. this keeps capture and encoding outside the binary:
x264 on a laptop, the v4l2 hardware encoder on rpi4, a file in tests.

parameter sets (sps/pps) are sent once per stream as an untimestamped media
message, matching how a phone delivers codec config. pictures follow with a
microsecond timestamp.

flow control follows the headunit ack window: no more than `max_unacked`
pictures are in flight. when the window is full the encoder pipe backs up.
that trades latency for an unbroken stream; dropping frames instead needs a
way to ask the encoder for a keyframe, which the command interface lacks.

an access unit is released when the first nal of the next one arrives, or
when the encoder has been quiet for 10 ms. encoders write a picture in one
go, so a pause means it is complete, and without it the last picture before
a still screen would wait forever for the next. an encoder that stalls for
longer in the middle of a picture would have it sent in two parts.

the command (`video.rs`) outlives video focus and connections: the driver
switching to the car's own screen and back, or a reconnect, must not
restart what the phone shows. while no headunit is watching, its pictures
are read and dropped, so it never blocks. a headunit that missed part of
the stream picks it up at the next keyframe, which it can only decode from;
a keyframe is an idr picture, or a unit that carries parameter sets. x264
puts the parameter sets in front of every keyframe of a raw stream; a
hardware encoder may write them only once, at the start, so `video.rs`
keeps the last ones it saw and puts them in front of the keyframe a
headunit resumes at. a new mode, or
the command exiting, starts it afresh.

the command runs in a process group of its own, and stopping it sends the
whole group SIGTERM, then SIGKILL after two seconds. that reaches
everything a session starts, such as a compositor, not only the shell. the
shell also gets SIGTERM from the kernel if the server dies without stopping
it (PR_SET_PDEATHSIG).

### desktop sessions

the session scripts in `sessions/` are video commands that run a headless
wlroots compositor at the negotiated mode, rendering on the cpu (pixman),
captured by wf-recorder into x264. input comes from the `--uinput` devices
through the compositor's libinput backend. `lib.sh` holds what they share.

the cpu is the default because it works everywhere, the test vm included,
whose render node has no 3d behind it. BEHEAD_RENDERER names another
wlroots renderer; with `gles2` the compositor uses the gpu's render node,
and waydroid.sh leaves android on the gpu `waydroid init` found instead of
setting swiftshader. measured on the rpi4 at 800x480 (2026-10-03, cpu then
gpu): osmand~ cold start 7.2 to 8.8 s against 3.4 to 4.3 s, settings 2.0 to
2.4 s against 0.7 to 1.2 s, the launcher's median frame 31 ms against 12 ms,
its 95th percentile 117 ms against 36 ms, and surfaceflinger idle at 44% of
a core against 5%. what stays is the capture: wf-recorder with x264 takes
45% of a core whatever is on screen.

the sessions choose their encoder in `sessions/encoder.sh`: the hardware
one (`h264_v4l2m2m`, the pi 4's) for pictures of 500000 pixels and more
where a one frame trial encode with ffmpeg succeeds, x264 otherwise, and
x264 again if the hardware recorder ends while the compositor lives.
`BEHEAD_ENCODER=x264` or `v4l2m2m` names one outright. the size limit comes
from the pi (2026-10-04, wf-recorder's share of a core and touch to picture
through the kiosk, x264 then hardware): at 800x480, 48% against 37% and
103 ms against 170 ms; at 1280x720, 95% against 63%. the hardware saves
little on a small picture, since reading the screen back and converting it
stay in software, and it holds pictures back a frame or two. at 720p x264
is at the limit of its one thread, so there the hardware is worth its
delay. it is given yuv420p: with nv12, which it also accepts, the picture
comes out green from halfway down, from ffmpeg as from wf-recorder. it
takes a bit rate in place of a quality, 0.35 bits per pixel, has its
profile set to baseline, and reports level 4 whatever it is asked.

the pipe demo keeps x264 through ffmpeg: with the hardware encoder its
touch to picture time on the pi goes from 34 ms to 67 ms, past the 50 ms
it is held to.

- `kiosk.sh`: sway, with one application full screen.
- `phosh.sh`: phoc with the phosh shell, on a session bus of its own and
  without gnome-session. settings come from a keyfile in the session's
  runtime directory, so the screen neither locks nor blanks. phosh locks at
  startup unless GDMSESSION says a display manager already let the user in,
  so the script sets it. a pulseaudio of the session's own gives its
  applications sound (see audio). the overview opens by a swipe, which a
  car without a touchscreen cannot make, so the keyfile binds the car's
  home button (XF86HomePage) to phosh's toggle-overview, which has no key
  by default.
- `waydroid.sh`: android's full ui in the kiosk. it loads binder with the
  three legacy devices, since debian's kernel has no binderfs, starts the
  container, and stops the android session when the car session ends,
  because android outlives its window. each android app is a wayland
  window of its own. without a gpu android renders with swiftshader.
  apps reach android two ways. the session installs apks it finds, which
  needs android running and takes minutes on a small machine.
  `tools/preinstall-apks.py` instead writes them into the system image at
  build time, under `/system/app`, where android registers what it finds on
  every start. it edits the ext4 image with debugfs, so it needs neither
  root nor an android of the target's architecture, which an image build on
  another architecture could not run. android does not unpack the native
  libraries of apps in its image, so the tool stores them beside the apk. an
  app android already holds under `/data` from an earlier install stays in
  use there, as an update of the one in the image.

every session has a session bus of its own, in its runtime directory, which
BEHEAD_RUNTIME_DIR can name. phosh needs one, and so does waydroid, whose
`waydroid app` commands find the session through it: from outside the
session they report it stopped unless pointed at that bus. waydroid also
binds a pulseaudio socket from the runtime directory into android whether
one exists or not, and android does not start without it, so `lib.sh` puts
an empty stand-in there. android's sound goes nowhere until a session runs a
sound server.

notes:
- wf-recorder normally captures only when the screen changes. a still
  screen then sends nothing, and openauto shows a picture only once more
  follow it, so a still phosh overview stayed black. the scripts capture
  every frame at the negotiated rate instead; repeats of a still picture
  are tens of bytes each.
- wf-recorder gives x264 a microsecond timebase, which x264 reads as the
  frame rate, so the scripts set the h.264 level from the mode instead.
- the scripts follow their parent process and their compositor, and end
  when either goes: a server that dies without stopping them still takes
  the compositor with it, and a compositor that dies does not leave
  wf-recorder hanging. wf-recorder ignores SIGTERM once its compositor is
  gone, so it gets SIGKILL.
- like any video command, a session survives focus changes and reconnects,
  so the applications in it keep running.

## audio

`--audio-cmd` writes pcm at the format of the headunit's media channel, and
the server sends it in 40 ms packets, within the ack window like video.
packets are timestamped by the samples sent before them rather than by the
clock, so the timestamps stay exact whatever the pipe does. the command must
produce audio in real time; the server does not pace it. video and audio
share `Flow` in `session.rs`: setup, focus, ack window and start or stop.

the two streams carry clocks of their own, and a car plays each as it
arrives, so nothing lines them up. the picture arrives later: it is
captured, encoded, and decoded by the car, while sound is only recorded.
`--audio-delay` holds each audio packet that long after it was read
(`audio.rs`), in 40 ms steps. the right value depends on the host and the
car, so it is a setting, `AUDIO_DELAY` in a session's `.env`, and kodi's
150 ms is a guess by ear still to be measured.

media audio is opened only with `--audio-cmd`, because asking for audio focus
silences the car's own sound. guidance (speech) and system audio channels are
not opened yet.

`--mic-cmd` reads the headunit microphone. the microphone is opened as soon
as the channel is set up and stays open for the session; a phone opens it
only while listening, which matters once a car shows a microphone indicator.
writes to the command never block the session: they go in pieces no larger
than PIPE_BUF, which the kernel writes whole or not at all, so audio the
command has no room for is dropped without splitting a sample.

behead does not talk to a sound server. for kodi an alsa loopback stands in
for one: kodi plays into one end and `sessions/audio.sh` records the other
with `arecord`. kodi does not follow alsa's default device: it picks a card
of its own at its first start and stores that, so pointing the default at
the loopback (an `.asoundrc`) was tried and did not work. `kodi.sh` instead
writes the device into the settings of a kodi that has none, after loading
the module, since kodi replaces a stored device it cannot find. the pi's
unit always passes `--audio-cmd ${AUDIO_CMD}`, which a session's `.env` file
sets, and the server takes an empty command for none.

applications that play through a sound server get one in the session:
`start_sound` in `lib.sh` runs a pulseaudio in the session's runtime
directory whose only sink is the loopback's playback end, so `audio.sh`
serves it unchanged. phosh.sh starts it; android's sound would go the same
way and is not wired yet. the sink is fixed at 48000 hz stereo, the format
of the car's media channel, because the loopback makes both ends use one
format and `arecord` asks for the car's. pipewire was tried first, as one
process with its pulseaudio module and a static alsa sink: a client connects
but nothing links its stream to the sink, which is a session manager's job
(wireplumber), and the loopback's sink took no port configuration by itself
either. one pulseaudio does both.

the sources disagree on three audio details, recorded in WORKING.md as quirk
candidates (R9): audio type 4, audio focus type 3, and the field order of
the microphone response. none of them affects what behead sends or reads.

## input

touch and button events become text lines, printed to stdout and written to
the video command's stdin. so do the two kinds of event a car's other
controls send, fields 5 and 6 of the input event, which AACS leaves out and
aasdk has: relative, a rotary knob's turns in detents under key code 65536,
and absolute, a control's position. a knob's tilt and click are ordinary
dpad buttons. `--uinput` types a knob's turn as keys, described below, and
does not pass absolute events on, so only a video command reading the lines
sees those.

a headunit may offer more than one input service, a touchpad or a set of
buttons beside the touchscreen, each on a channel of its own. every one is
opened and sent a binding request for its keys, since a headunit sends
nothing from an input service it was not asked to bind. a touchpad's
touches are field 7 of the input event, the touchscreen's message in the
pad's coordinates, and come out as `touchpad` lines. the server logs what
each input service offers at service discovery.

what arrives unread is kept visible, because a car is the only source for
what its controls send: `proto::unknown_fields` walks the top level of an
input or sensor event for field numbers prost has no member for, and a
message with an id no channel handles is passed on whole. both become
`unknown` lines, on stdout and to the video command, so the demo shows
them on the car's own screen. the stdin pipe is non-blocking: a command that
does not read its input loses lines instead of stalling the session.

with `--uinput`, `uinput.rs` also creates two kernel input devices once
service discovery arrives, and removes them when the session ends. that way
any compositor reading input through libinput (sway, phoc, mutter) sees the
car as local hardware, without a per-compositor injection path.
- `behead touchscreen`: multitouch protocol b, one slot per android pointer
  id, plus single touch emulation, which libinput requires. its range is the
  touch area from discovery, not the video mode: libinput scales a
  touchscreen's range onto the output it is mapped to, so the compositor
  handles any difference between the two.
- `behead keys`: the advertised android key codes that have a linux
  equivalent (home to KEY_HOMEPAGE, back to KEY_BACK, dpad, media, volume).
  a car that lists the rotary knob also gets the keys its turn types, one
  press and release per detent. keys were chosen over a wheel axis because
  a wheel scrolls whatever lies under a pointer the car has no way to move,
  while keys go to whatever has focus. no pair of keys suits every
  application, so `--knob` picks: tab and shift+tab, which most toolkits
  read as next and previous, or down and up for kodi, where tab switches
  to the player.

## sensors

the headunit's sensor channel carries what the car knows: location, night
mode, driving status, speed and more. a sensor sends nothing until asked, so
the session asks for every one the headunit advertises that `SensorEvent`
can carry, eleven of aasdk's types. asking changes nothing on the car,
unlike audio focus, so this needs no flag.

`sensors.rs` turns readings into text lines, delivered like input lines.
values with a known scale come out in plain units (degrees, metres); the
rest as sent, because nothing seen so far gives their units. openauto sends
location speed in knots where google's field name suggests metres per
second: a quirk candidate (R9).

a headunit reports a state such as night mode once, when asked, which is
before it shows the phone and so before the video command runs. the server
keeps the latest line of each kind and writes them to a video command when
it attaches.

the sessions act on what they can. `lib.sh`'s `on_line` calls a function
with each line from stdin. phosh.sh sets the dark style preference for
night, waydroid.sh android's night mode.

location takes a way of its own, since a location service is system wide
and outlives sessions. `--nmea-socket` has the server listen on a unix
socket and write each fix there as a GGA and an RMC sentence (`nmea.rs`),
which is how geoclue shares a gps receiver (its `nmea-socket` option, shipped
as `sessions/geoclue.conf`). geoclue connects while an application wants a
location and retries every five seconds, so the server keeps the latest fix
for a reader that connects after it. choices in the sentences, from reading
geoclue's parser:

- the time of day is the server's clock at arrival. geoclue combines it
  with its own date and drops a fix older than the one before, so the car's
  clock, which may differ, stays out of it.
- accuracy becomes horizontal dilution at five metres per unit, which
  geoclue maps back to metres in coarse steps.
- speed is taken as metres per second and written in knots, so openauto's
  knots come out wrong by that factor (the quirk above).
- the checksum is correct, though geoclue does not check it.

geoclue hands a location to an application once the user's agent agrees.
phosh is one, with its location setting on, which phosh.sh sets.

android has no gps in waydroid, so waydroid.sh adds a test provider named
gps and feeds it each `location` line with `cmd location`. android's
location service drops commands from root without a word, which is what
`waydroid shell` runs as, so the session runs them as android's shell user
(`waydroid shell --uid 2000`), after allowing that user mock locations. the
command takes a position and an accuracy; speed, bearing and altitude would
need an app that holds the mock location permission and builds the fix
itself. every fix costs a `waydroid shell`, fine at a car's one per second.

## demo program

`crates/demo` builds `behead-demo`, a video source whose only dependency is
`libc`. by default it renders raw rgb frames at the negotiated mode and rate
and reacts to the input lines on stdin. each frame is due one period after the
last; when it falls behind (the encoder starting up blocks its first writes) it
skips ahead rather than sending the frames it missed in a burst. ffmpeg encodes
its output. it is the default video command on the rpi image and in `make demo`,
and the end-to-end tests use it to check the whole loop: a touch from the
headunit has to come back as a marker at the same place in the video.

with `--wayland` it is a fullscreen wayland client instead (`wayland.rs`),
taking its size and touch and key input from the compositor. it speaks the
wire protocol directly, covering wl_shm, xdg-shell, wl_touch and wl_keyboard,
because the alternatives are a large client library or a dependency in a
crate meant to stay small. `libc` is needed only to pass the shared memory
file descriptor (sendmsg with SCM_RIGHTS), which stable rust does not expose.
the uinput tests run it in the kiosk session, so the same marker check covers
uinput, libinput, the compositor and screen capture.

## transports

output is written by a thread of its own, fed from a queue. a usb gadget
write blocks until the host collects the data, so writing from the read loop
deadlocks against a headunit that is waiting for us to read.

- tcp (`--listen`): used by the test suite, and by headunit emulators that
  connect over the network.
- usb gadget (`--usb`): the headunit is the usb host, so the machine running
  this needs a usb device controller. rpi4 has one on its usb-c port.

### usb gadget lifecycle

`gadget.rs` builds one configfs gadget with one functionfs function: a vendor
interface with a bulk in and a bulk out endpoint, 64 byte packets at full
speed and 512 at high speed, as the android kernel's accessory driver does.

1. the gadget enumerates under an ordinary phone identity
2. the headunit sends the android open accessory requests: 51 (we answer
   protocol version 2), 52 (identification strings), 53 (start)
3. on 53 only, the gadget detaches and re-enumerates as 18d1:2d00
4. once the headunit configures the accessory, a session runs on the bulk
   endpoints
5. when the headunit goes away the gadget returns to step 1. a disable ends
   the session at once, but only counts as going away if the host does not
   enable the device again within two seconds: hosts reset, reconfigure and
   reauthorize devices they keep using (usbguard does it to every new
   device), and treating that as an unplug made the first test on a real
   rpi fail intermittently

a write that fills its last usb packet exactly is followed by a zero length
packet, sized for the speed the link negotiated (64 bytes full speed, 512
high speed). without it a host reading into a larger buffer, as headunits
do, waits for the next write: the first run on a real rpi showed this as
one frame in a few hundred arriving a frame late.

teardown has one code path: `behead teardown`. on SIGINT or SIGTERM the
server execs itself with that command. the kernel refuses to remove a gadget
while endpoint files are open, and exec closes all of them at once. a server
that was killed outright leaves the gadget behind; the next start, or a
manual `behead teardown`, removes it.

## test tiers

0. `make test-e2e`: real binary over tcp against `tests/e2e/fakehu.py`, a
   scripted headunit written independently in python. no root.
1. `make test-vm`: the same session tests over usb inside an incus vm
   (`incant.yaml`). the kernel `dummy_hcd` controller loops the gadget back
   to a host port in the same vm, and the test plays the headunit through
   pyusb, including the accessory switch, unplug, teardown and crash recovery.
2. `make test-rpi`: the laptop plays the headunit, through the same scripted
   headunit and pyusb, against a flashed rpi4 on a usb cable. checks that
   video decodes and arrives at a steady 30 frames per second, at 480p and
   720p, and that a bus reset is survived, and measures touch to picture:
   33 ms on the pi, one frame, as in the vm. passes on an rpi4.
   `tests/vm/test_service.py` runs the same device tests inside the vm
   against the systemd unit from `rpi/overlay`, including the hand-over of
   the usb controller with `g_ether`.
3. `tests/vm/test_openauto.py` (part of `make test-vm`): openauto, an
   independent headunit implementation, runs in a container inside the vm
   on a virtual display and drives the server over the usb gadget. checks
   that it negotiates video, that the picture it draws is the moving test
   pattern, and that clicks and drags on its screen arrive as touch events
   at the right coordinates.
   `tests/vm/test_uinput.py` and `tests/vm/test_kodi.py` (also part of
   `make test-vm`) run desktop sessions: the scripted headunit over tcp,
   `--uinput`, and `sessions/kiosk.sh`. the kodi tests read kodi's open
   window over json-rpc, to check that a tap on the settings gear opens
   settings and the car's back button closes it. kodi ignores input during
   window animations and taps with no time between down and up, so the
   tests wait for animations to finish and hold each touch and button.
   `tests/vm/test_latency.py` measures touch to picture: from the scripted
   headunit sending a touch to it receiving the first picture with the
   demo's marker under the finger. in the vm at 800x480@30: the pipe demo
   (x264 through ffmpeg) takes 33 ms, one frame, over tcp and over the usb
   gadget alike; the wayland kiosk 100 to 166 ms, three to five frames,
   spent in the client waiting for sway's next frame, sway compositing on
   the one after, and wf-recorder's frame rate filter holding a frame. a
   real car adds its own decoding and display. the scripted headunit sets
   TCP_NODELAY: without it, its own acks held each touch back a frame. the
   rpi device tests measure the same on hardware.
4. the sprinter headunit.

openauto found one bug no scripted test could: it presents an x.509 v1
certificate, which webpki refuses to parse. the scripted headunit now uses
a v1 certificate too, and the server no longer parses the headunit's
certificate at all.

## rpi4 image

`rpi/` is a raspi-provision project: it holds only what differs from that
framework's base image. `make rpi-image` cross-compiles a static aarch64
binary (musl, built with clang and rust-lld, so no arm gcc toolchain and no
dependence on the image's glibc), installs it into `rpi/overlay`, and runs
`raspi-provision image` with a path of the session's own,
`rpi/images/<hostname>-<session>.img.gz`, so each session's image stays
until that session is built again. `make rpi-flash SESSION=` hands the same
path to `raspi-provision flash`, which decompresses it onto the card.

- `overlay/etc/systemd/system/behead.service` runs the server on boot
- `overlay/etc/behead/behead.env` holds the default video command, the demo
- `depends/behead` lists packages, which raspi-provision installs into the
  image in a one-off qemu boot
- `OVERLAY=tmpfs`: the root is read-only with a ram upper layer

`make rpi-sessions`, part of `make rpi-image`, generates the rest from
`SESSION` and `SESSIONS`, and removes what an earlier build chose:

- every session script goes to `/usr/local/lib/behead/sessions`. every
  session is run the same way, `NAME.sh {width} {height} {fps}`, which is
  why kodi and the wayland demo have scripts of their own that only call
  `kiosk.sh`. `make demo SESSION=` relies on the same form
- `sessions/depends/NAME` becomes `depends/session-NAME` for each session
  chosen, plus the kiosk's list when one of them runs in the kiosk. the
  scripts are small enough to always ship; the packages are not
- `SESSION` becomes `VIDEO_CMD` in `overlay/etc/behead/session.env`. the
  unit reads it after `behead.env`, so the tracked default stays as it is
  and a build without `SESSION` shows the demo. `sessions/depends/NAME.env`
  follows it there for the session shown. waydroid's, kodi's and phosh's
  set `BEHEAD_RENDERER=gles2`; waydroid has been measured on the pi's gpu
  and phosh seen running on it, and the wayland demo stays on the cpu. kodi's and phosh's also name
  the audio command and its delay
- `sessions/depends/NAME.preferences` is an apt pin, installed in
  `/etc/apt/preferences.d`, which raspi-provision's bake resolves against.
  phosh has one: raspberry pi os ships a wlroots built by raspberry pi, apt
  prefers it as the newer version, and debian's phoc dies on it at startup
  with "stack smashing detected", whatever the renderer. the pin takes
  wlroots, and sway with it, from debian
- `sessions/depends/NAME.sources` is an apt source the packages need, and
  `NAME.cmdline` kernel parameters the session needs, collected in
  `rpi/session.cmdline` and handed to raspi-provision as `CMDLINE_EXTRA`.
  waydroid has both: its package is only in debian's backports, and
  android's low memory killer needs `psi=1` on the pi kernel, where android
  otherwise gives up some 19 seconds into its boot
- for waydroid, the android images go to `/usr/share/waydroid-extra/images`,
  the place `waydroid init` looks before it downloads any. the build's copy
  of the system image gets the f-droid and osmand~ apks from
  `tools/preinstall-apks.py`; the cached download stays as fetched
- `sessions/geoclue.conf` goes to `/etc/geoclue/conf.d`, and the unit passes
  `--nmea-socket` with the path it names, under a `RuntimeDirectory`.
  geoclue itself comes with phosh's packages

the unit always passes `--uinput`, which the demo ignores and sessions
need. `sessions/home.sh`, which every session script reads, makes
`BEHEAD_HOME` the home of the applications, `~/.behead` by default. the
unit sets `BEHEAD_HOME=/var/lib/behead`, since root's own home is in the ram
layer. `overlay/etc/persist.d/behead` lists that directory, and
raspi-provision binds every directory listed there from the persist
partition, as it does `/home`. so what sessions write under their home
survives: kodi's library, and android's data, which waydroid keeps in
`~/.local/share/waydroid`. the unit starts after `overlay-firstboot`, which
makes the bind on the first boot.

the unit also sets `BEHEAD_MEDIA=/home/behead/media`, on the same partition.
`kodi.sh` creates `videos` and `music` under it and, where kodi has no
`sources.xml` yet, writes one naming the two. the path comes from the unit
and not the script, so off the pi nothing is created outside the session's
home.

kodi enables by itself only the add-ons in its own manifest. one from a
package of its own (debian's kodi recommends kodi-visualization-spectrum)
goes into kodi's add-on database switched off, and kodi asks about each at
its first start (`ConfigureAndEnableAddons` in kodi's Application.cpp).
kodi adds only the add-ons its database lacks and keeps the rows it finds,
and it decides before the screen is up, so nothing can answer in time from
outside. `sessions/kodi-addons.py`, run by `kodi.sh` before kodi, therefore
makes the database where there is none, with every packaged add-on on. it
carries the six tables of kodi 20 and 21's `Addons33.db`, which is the
fragile part: a kodi with a newer schema upgrades a database it finds, one
with an older schema would ignore this one and ask again.

a source is not yet the library. kodi lists a film only when its folder has
a content type and a scraper has described the file, both normally set on
screen, and its film scrapers need the internet. `sessions/kodi-library.py`,
started by `kodi.sh` beside kodi, does it without either:

- it writes a `.nfo` with the title and year from the file name beside every
  video that has none. `metadata.local`, the one scraper that works offline,
  reads those and skips a file without one
- once kodi has made its video database, it adds the row "set content"
  would, the videos folder as movies with that scraper. kodi has no json-rpc
  call for this, so the row goes in with sqlite
- it asks kodi over json-rpc, on the tcp port kodi offers to localhost by
  default, to scan the videos and the music folder. music needs no content
  type: kodi reads the files' tags

it stops with the session, which it knows by its parent process, and writes
to stderr, since stdout is the video.

the pi has one usb device controller, and the base image gives it to
`g_ether` for ssh over usb. the service unloads `g_ether` before starting and
loads it again after stopping, so the two uses take turns and raspi-provision
needs no change.

## protocol sources and quirks

google publishes only the usb layer: android open accessory 1.0 and 2.0 on
source.android.com, and the accessory driver in the android kernel. the
projection protocol above it has no public specification.

for that layer the sources are, strongest first: protobuf definitions
extracted from google's desktop head unit binary (the "galdocs" dump),
aa-proxy-rs, aasdk, and AACS. where they disagree this implementation
follows the first, and the differences are candidates for R9 quirks:

- frame rate enum: google has 60 = 1 and 30 = 2. aasdk and AACS have them
  reversed, so openauto advertising "30" is read here as 60.
- video focus: the headunit stays on its native screen until the phone sends
  a video focus request. AACS never sends one. we send it after video setup
  and also accept an unsolicited grant.
- touch: actions 5 and 6 are a further finger going down or up. aasdk lacks
  them, and AACS lacks the action index needed to tell which finger moved.
- accessory switch: AACS leaves its loop on any zero length control request.
  we switch on request 53 only.
- initial usb identity: AACS also exposes a mass storage function before the
  switch. we do not. a headunit that ignores a vendor-only device would need
  it.
- protocol version: we answer 1.5 as AACS does. google's constants say 1.6.

none of these has been settled against a real headunit.

## known risks

- the sprinter headunit is the only real one tested against. openauto, the
  only independent implementation in the tests, does not verify the phone
  certificate at all: aasdk sets SSL_VERIFY_NONE (SSLWrapper.cpp:123), with
  no ca store and no way to add one.
- phone certificate: the 2021 sprinter headunit accepts a self-signed phone
  certificate, as openauto does. a headunit following google's guide checks
  it against one root, "Google Automotive Link" (C=US, ST=California,
  L=Mountain View, valid 2014 to 2044); every headunit and phone
  certificate seen in AACS and aasdk is issued by it, and nothing points to
  vendor specific roots. `../dexter`, with its config for the app
  (`configs/com.google.android.projection.gearhead.toml`), takes the root from the
  android auto app, which carries it to check headunits, and the app's own
  phone pair: the key is not stored in the clear there, but aes encrypted in
  the app's code and keyed by a stream the app mixes from the two certificates
  it carries, which the tool replays. the google signed phone certificate AACS
  publishes, with its key, chains to it but expired on 2026-02-18.
  the ordered certificate list exists for these headunits; see
  R8. `make rpi-image` puts the extracted pairs in the image behind the
  self-signed one, so the certificate proven on the sprinter is still
  offered first. the scripted headunit can verify like them: the session tests check
  that an expired certificate is reported, and, given the root
  (BEHEAD_GOOGLE_CA) and a google signed pair (BEHEAD_GOOGLE_CERT and
  BEHEAD_GOOGLE_KEY), that the self-signed one is refused and the google
  signed one accepted, separately and as the fallback from one to the
  other. without them those three skip.
- rustls offers only modern tls 1.2 suites (ecdhe with aes-gcm or chacha).
  an older headunit that needs cbc or dhe suites would require another
  backend behind the `Tls` trait.
- laptops rarely have a usb device controller, so a laptop can only reach a
  wired-only headunit through extra hardware.
