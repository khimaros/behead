# behead

an android auto server in rust: the half of android auto that normally runs
on a phone. it lets a raspberry pi or laptop put its own interface on a car
headunit.

status: early. sessions work over tcp and over a usb gadget, against a
scripted headunit and against openauto, which shows the video and sends
touch. it runs on a raspberry pi 4, tested from a laptop over usb, and a
2021 sprinter headunit accepts its self-signed certificate. see `DESIGN.md`.

## build

    mise install
    make

## run

    target/debug/behead \
        --cert phone.crt --key phone.key \
        --video-cmd 'ffmpeg -f lavfi -i testsrc2=size={width}x{height}:rate={fps},realtime -pix_fmt yuv420p -c:v libx264 -preset ultrafast -tune zerolatency -profile:v baseline -g {fps} -f h264 -'

- `--cert`, `--key`: certificate and private key presented to the headunit.
  repeat the pair to offer several, in order of preference
- `--cert-dir`: offer every `NAME.crt` in a directory with its `NAME.key`,
  in name order. when the headunit refuses a certificate (the handshake
  starts but never finishes), its next connection gets the next certificate,
  starting over after the last. the one it accepts stays in use
- `--video-cmd`: shell command writing annex-b h.264 to stdout. `{width}`,
  `{height}` and `{fps}` are replaced with the mode the headunit negotiated.
  the input lines shown below also arrive on the command's stdin, so it can
  react to them; lines it does not read in time are dropped.
  the command keeps running while the car shows its own screen and across
  reconnects, so the applications it shows keep their state; a new mode
  restarts it. the car picks the stream up again at a keyframe, with the
  parameter sets the command wrote last, so a hardware encoder that writes
  them only once works as well as x264, which repeats them. it must produce
  frames in real time: frames are sent as they
  arrive. for a generated source use ffmpeg's `realtime` filter, not `-re`,
  which lets the first half second out in one burst
- `--audio-cmd`: shell command writing signed 16 bit little endian pcm to
  stdout, in real time, played as the headunit's media audio. `{rate}`,
  `{bits}` and `{channels}` are replaced with the headunit's format, usually
  48000 hz stereo. without it the car keeps its own audio focus
- `--audio-delay`: milliseconds to hold that audio back. a picture takes
  longer to reach the car's screen than sound its speakers, so without a
  delay the sound of a film runs ahead of it. on the pi it is `AUDIO_DELAY`
  in `/etc/behead/session.env`; change it and `systemctl restart behead`
- `--mic-cmd`: shell command reading the headunit microphone as pcm on
  stdin, with the same replacements, usually 16000 hz mono
- `--listen`: tcp address to accept the headunit on, default `127.0.0.1:5277`
- `--usb`: appear to the headunit as a usb device instead. takes the name of
  a usb device controller from `/sys/class/udc`, or `auto` for the first
  one. needs root and a machine with a device controller, such as the usb-c
  port of an rpi4
- `--uinput`: also turn touch and buttons into kernel input devices named
  `behead touchscreen` and `behead keys`, so applications on a wayland
  compositor receive them directly. needs write access to `/dev/uinput`.
  map the touchscreen to the output shown on the headunit, for example in
  sway: `input "0:0:behead_touchscreen" map_to_output HEADLESS-1`
- `--knob`: what a turn of the car's rotary knob types on `behead keys`, one
  key press per detent. `focus`, the default, is tab one way and shift+tab
  the other, which moves between the things on screen in most applications.
  `arrows` is down and up, for applications such as kodi where tab does
  something else. the pi image's kodi session uses `arrows`; `KNOB` in
  `/etc/behead/behead.env` sets it for the others
- `--nmea-socket`: offer the car's location on a unix socket at this path,
  as the nmea sentences of a gps receiver (GGA and RMC), for a location
  service to read. a reader gets the latest fix when it connects and every
  one after. anyone who can reach the socket can read it; its directory,
  created if missing, decides who can

in usb mode the server creates the kernel gadget on start and removes it on
SIGINT or SIGTERM. if it was killed before it could, `behead teardown`
removes what was left.

touch, button and control events from the headunit are printed to stdout:

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

a touch line is `touch <action> <action index> <pointer id>:<x>,<y> ...`,
listing every finger on the screen. the action index says which of them the
action applies to. buttons and controls are named by their android key code.
a rotary knob is 65536: a turn is `relative 65536 <detents>`, negative one
way and positive the other, a tilt is a dpad button (19 to 22) and a click
the dpad's centre (23). `absolute <code> <value>` is a control moved to a
position. a `touchpad` line is a touch line for a car's touchpad, such as
the pads on a steering wheel, in the pad's own coordinates.

what a car sends that the server has no name for is printed too, to look
into: `unknown input <channel> field <number> <hex>` for a part of an input
event, `unknown sensor ...` likewise, and `unknown message <channel> <id>
<hex>` for a whole message, each with up to 32 bytes of it. the server's log
lists what each of the car's input channels offers when it connects:

    input channel 1: keys 3 4 19 20 21 22 23, touchscreen 800x480

readings from the car's sensors follow the same way, one line each, for
every sensor the car offers:

    location 52.5200066 13.404954 5 34.5 12.5 90.5
    night 1
    driving 0
    speed 27.78

- `location <latitude> <longitude> <accuracy> <altitude> <speed> <bearing>`:
  degrees, then metres, metres, speed and degrees
- `compass <bearing> <pitch> <roll>` in degrees, `speed <speed>`
- `night <0|1>`, `parking-brake <0|1>`
- `driving <restrictions>`: 0 when nothing is restricted, otherwise bits: 1 no
  video, 2 no keyboard, 4 no voice input, 8 no setup, 16 short messages
- `gear <gear>`: 0 neutral, 1 to 10, 100 drive, 101 park, 102 reverse
- `rpm`, `odometer <total> <trip>`, `fuel <level> <range> <low>` and
  `environment <temperature> <pressure> <rain>` as the car sends them: their
  units are not confirmed

a value the car leaves out is `-`. speeds are the car's number divided by
1000, metres per second by google's naming, though openauto sends knots.
the video command gets these lines on stdin too, and when it starts, the
latest of each kind the car sent before.

## try it

    make vm      # once: the test vm, with openauto inside
    make demo

this runs the server on the vm's usb gadget with openauto as the headunit,
and opens openauto's screen in gvncviewer when it is installed, otherwise
in the browser, to drive with the mouse and keyboard. `OPEN=` leaves that
to you: the url is printed, and vnc listens on port 5900. touch and button
events reaching the server are printed in the terminal. ctrl-c stops it.

`make demo SESSION=phosh` shows the phosh shell instead, `SESSION=kodi`
kodi, and `SESSION=wayland` the demo as a wayland client.

the screen is drawn by `behead-demo`, a small program that shows what the
car negotiated and reacts to its input:

- the resolution, frame rate and a frame counter
- a bar sweeping along the bottom at a fixed speed, so stutter is visible
- a numbered, coloured marker under every finger, and a trail behind drags
- how many touches so far, how many fingers are down, and where the rotary
  knob stands
- where a finger rests on the car's touchpad, if it has one
- the last six input events: every button with its name, each turn of the
  knob, each finger going down or up, on the screen or the touchpad, and
  anything the server could not read, as `UNKNOWN` with its bytes

openauto's keys: enter, the arrow keys, escape (back) and h (home) send the
matching android auto buttons. `VIDEO_CMD=... make demo` swaps the demo for
another video command. stop the demo before `make test-vm`; both need the
vm's usb gadget.

## wayland applications

`sessions/kiosk.sh` puts one wayland application on the headunit, full
screen in a headless sway, with the car's touch and buttons reaching it
through `--uinput`:

    behead --usb auto --cert-dir certs --uinput \
        --video-cmd 'sessions/kiosk.sh {width} {height} {fps} behead-demo --wayland'

it needs sway, wf-recorder and x264, renders on the cpu, and runs as root
because it opens input devices without a login session. every session
encodes with the machine's hardware encoder (`h264_v4l2m2m`, as on the
rpi4) when the picture is 1280x720 or larger and a trial encode with ffmpeg
works, and with x264 otherwise. `BEHEAD_ENCODER=x264` or `v4l2m2m` in the
server's environment chooses one regardless; the hardware costs less cpu
and adds a frame or two of delay. on a machine with a
gpu, such as the rpi4, set `BEHEAD_RENDERER=gles2` in the server's
environment and every session renders on it instead; on the pi android then
opens apps in about half the time. `behead-demo
--wayland` is the same demo screen as a wayland client; any other
application works in its place, for example kodi:

    --video-cmd 'sessions/kiosk.sh {width} {height} {fps} kodi --windowing=wayland'

taps and the car's back, dpad and enter buttons are tested with kodi.
`sessions/kodi.sh` runs it that way and adds sound: a kodi without settings
is set to play into one end of an alsa loopback, and `sessions/audio.sh`
records the other end for the car:

    --video-cmd 'sessions/kodi.sh {width} {height} {fps}' \
    --audio-cmd 'sessions/audio.sh {rate} {channels}'

this needs the snd-aloop kernel module and arecord, from alsa-utils. a kodi
that already has settings keeps its audio device; choose "Loopback" under
settings, system, audio. an empty `--audio-cmd` counts as none. with
pipewire, record a virtual sink's monitor with `pw-record` instead.

kodi asks on its first start whether to enable each add-on that came as a
system package of its own, such as the spectrum visualization. `kodi.sh`
switches those on before a kodi's first start, so nothing is asked on the
car's screen. needs python3.

`sessions/phosh.sh` shows the phosh mobile shell, as on mobian and
droidian, with its overview and apps on the headunit:

    --video-cmd 'sessions/phosh.sh {width} {height} {fps}'

it needs phosh, phoc, gnome-settings-daemon-common and gsettings, and like
the kiosk it renders on the cpu and runs as root. the screen does not lock
or blank, and applications are asked for their dark style while the car
reports night. a car without a touchscreen drives it with its knob: a turn
moves between the overview's icons, a click opens one, and the home button
goes to the overview and back.

the session runs a pulseaudio of its own, so its applications have sound.
it plays into the same alsa loopback as kodi, and the same command records
it for the car:

    --audio-cmd 'sessions/audio.sh {rate} {channels}'

this needs pulseaudio, the snd-aloop kernel module and arecord.

linux applications learn where the car is from geoclue, which reads the
server's nmea socket. install `sessions/geoclue.conf` as
`/etc/geoclue/conf.d/90-behead.conf`, restart geoclue, and add the flag:

    behead ... --nmea-socket /run/behead/nmea.sock

geoclue answers an application only when the user's shell agrees. phosh
does, and the session switches its location setting on. a bare kiosk has no
shell to ask, so there the application needs an entry of its own in
geoclue's configuration (`allowed=true`, `system=true`).

`sessions/waydroid.sh` shows android, through waydroid: its launcher and
apps, with the car's touch and its home and back buttons:

    --video-cmd 'sessions/waydroid.sh {width} {height} {fps}'

it needs waydroid and the binder kernel module. where waydroid has not been
set up, the session does it: `waydroid init`, which downloads android's
images, about 1 GB, unless they are in `/usr/share/waydroid-extra/images`,
and android set to render on the cpu, as the session's compositor does.
android's night mode follows the car's, and the car's position becomes
android's gps location, for apps such as osmand~. android gets the position
and its accuracy; speed, bearing and altitude do not reach it yet.

apps come from f-droid. `tools/fetch-fdroid.py` downloads one, checking
f-droid's signature on its index and the app's hash, and the session
installs every apk it finds in `/usr/share/behead/apks`, or the directory
BEHEAD_APKS names, that android does not have yet:

    tools/fetch-fdroid.py net.osmand.plus --abi arm64-v8a      # osmand~
    tools/fetch-fdroid.py org.fdroid.fdroid --abi arm64-v8a    # the f-droid client

installing takes android minutes on a small machine. to have the apps there
from android's first start, put them in its system image ahead of time:

    tools/preinstall-apks.py /usr/share/waydroid-extra/images/system.img --abi arm64-v8a APK...

this writes each apk, and its native libraries for that abi, to
`/system/app` in the image, growing it to fit. it needs e2fsprogs, no root,
and no android running. apps added this way cannot be removed from the
car's screen, only updated.

`waydroid app install` and `waydroid app launch` talk to the session over
its own bus. to use them from outside, name the session's runtime directory
and point at the bus in it:

    --video-cmd 'env BEHEAD_RUNTIME_DIR=/run/behead-session sessions/waydroid.sh {width} {height} {fps}'
    DBUS_SESSION_BUS_ADDRESS=unix:path=/run/behead-session/bus waydroid app launch net.osmand.plus

android has no sound on the headunit yet.

## reading the tls log

`journalctl -u behead -f` on the pi shows each handshake:

    tls: presenting certificate subject O=..., issuer O=..., valid FROM to UNTIL (FILE)
    tls: headunit offers TLSv1_2 with cipher suites TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256 ...
    tls: handshake complete: TLSv1_2 TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384, headunit certificate subject ...

the first line names the certificate offered on this connection; check
whether it has expired. the second lists everything the headunit can speak;
rustls only implements the ecdhe suites with aes-gcm or chacha20, so a list
without any of those cannot work. if the headunit refuses our certificate
the session ends with

    tls: the headunit rejected the phone certificate (alert UnknownCA)

where the alert names its reason (UnknownCA: not signed by an authority it
trusts; CertificateExpired; BadCertificate). any other tls failure is logged
as `tls: ` followed by the error. with more than one certificate, an
unfinished handshake is followed by

    tls: handshake not completed, the next connection gets certificate N

## raspberry pi 4

the image is built by [raspi-provision](../raspi-provision), checked out next
to this repo. it runs from a read-only root, so cutting power does not
corrupt the card.

    make rpi-image                       # builds rpi/images/behead-demo.img.gz
    $EDITOR rpi/customize.env            # set MOUNT_DEVICE to the sdcard
    make rpi-flash                       # DESTRUCTIVE: writes MOUNT_DEVICE

settings live in `rpi/customize.env`, created from `customize.env.example` on
the first build. both targets run raspi-provision straight from its
checkout; the `raspi-provision` commands shown further down need it
installed (`make install` in that repo). the image is named after the
hostname and the session it shows, `-demo` without one, so images of several
sessions can sit side by side. give `make rpi-flash` the same `SESSION` to
write that one.

the packages in `rpi/depends`, ffmpeg for the default video command, are
installed into the image when it is built. that needs qemu-system-aarch64
11 or newer, dtc and internet access on the build host, and makes the first
build slow; later builds reuse the downloaded packages.

to try a change on a running pi without reflashing:

    make rpi-deploy      # cross-builds, copies both binaries, restarts the service

the copies live in the ram layer, so a reboot returns to the image's
binaries. it logs in as raspi-provision does (root at `ADDRESS` from
`rpi/customize.env`; override with `RPI_ADDRESS=...`).

the `behead` service starts on boot and takes the usb-c port for android
auto. `systemctl stop behead` on the pi hands the port back to ssh over
usb. the video command is set in `/etc/behead/behead.env`, and shows the
demo.

to put a desktop session on the headunit instead, name it when building:

    make rpi-image SESSION=phosh                 # wayland, kodi, phosh or waydroid
    make rpi-image SESSION=kodi SESSIONS=phosh   # kodi shown, phosh installed beside it

`SESSION` is the session shown, and `SESSIONS` installs more to switch to
on the device. each brings the packages listed for it in `sessions/depends`.
the session scripts are in `/usr/local/lib/behead/sessions` on every image,
and the choice is the `VIDEO_CMD` in `/etc/behead/session.env`, which the
service reads after `behead.env`. to switch on the device, edit or remove
that file and `systemctl restart behead`; on the read-only root that lasts
until the next boot, or for good after `raspi-provision overlay-commit`.
sessions give their applications a home of their own, `~/.behead` of
whoever runs behead unless `BEHEAD_HOME` names another, so kodi under
behead keeps its settings apart from your own. on the pi the service sets
`BEHEAD_HOME=/var/lib/behead`, which the image lists in `/etc/persist.d` so
it survives reboots as `/home` does.

kodi plays what is in `/home/behead/media`: copy films and shows to
`/home/behead/media/videos` and music to `/home/behead/media/music`, for
example with `scp` as root. `kodi.sh` creates both directories and gives a
kodi without sources the two as its sources, so they are under Videos >
Files and Music > Files from the first start. they are in the library as
well, under Movies and Music, a moment after each start of the session:
`kodi-library.py` names every video after its file, "Title (Year).mkv"
being the form kodi asks for, in a `.nfo` it writes beside the file, since
the pi has no internet to look films up with. a `.nfo` of your own is kept.
files copied while kodi runs appear at the next start.
the directory is `BEHEAD_MEDIA` in `behead.service`; sources added or
changed in kodi are kept.

`SESSION=waydroid` also adds debian's backports as an apt source, since
stable has no waydroid package, and puts android's system and vendor images
in the image, 2.4 GB, where `waydroid init` finds them and downloads
nothing. `tools/fetch-waydroid.py` fetches them once, about 1 GB, into
`~/.cache/behead/waydroid`, checked against the hashes waydroid's update
channel lists. the newest f-droid and osmand~ apks, from
`tools/fetch-fdroid.py`, are written into the image's copy of the system
image by `tools/preinstall-apks.py`, so android starts with both apps.
`waydroid.sh` sets waydroid up from the images at the first session after
each boot; android keeps its data under `/var/lib/behead`. the
image boots with `psi=1`, because android does not start on a kernel with
pressure stall information off, as the pi's is by default. if android stays
black, `waydroid status` on the pi says whether its container is running.

the service offers the car every `NAME.crt` and `NAME.key` pair in
`/etc/behead/certs`, in name order, until one is accepted. the image always
carries a self-signed pair, `90-self-signed`. supply your own as `10-phone`:

    make rpi-image PHONE_CERT=phone.crt PHONE_KEY=phone.key

every pair [dexter](../dexter) left in `~/.cache/behead/certs` (see
[test](#test); `CERT_CACHE=...` for another directory) is added behind the
self-signed one as a backup, such as `95-carservice`, and leaves the image
again once it is gone from the cache. or drop more pairs into
`rpi/overlay/etc/behead/certs` before building, named so they sort where
they should be tried. the build checks that each key belongs to its
certificate and warns about expired ones. the self-signed certificate is
accepted by openauto and by the 2021 sprinter headunit. a headunit that
verifies the phone certificate against google's authority refuses it and
gets the next pair on its following connection.

the usb-c port also powers the pi. a headunit port may not supply what a pi 4
needs under load; power it through the gpio header if it browns out.

## test

    make test-e2e    # tcp, no root
    make vm          # create the test vm, once
    make test-vm     # usb gadget, pi service, openauto and desktop tests in the vm
    make test-rpi    # laptop as headunit, against a pi on a usb cable

the tests run headless. `HEADED=1` shows what the scripted headunit
receives, live in the browser, which opens by itself (`OPEN=` to not), and
`TESTS` picks what to run:

    make test-vm HEADED=1 TESTS=test_phosh

`make test-rpi` needs the udev rule in `tests/rpi/70-behead.rules`
installed on the laptop, and the pi's usb-c port cabled to it. it also
prints the pi's touch to picture latency.

`make demo-rpi` makes the browser the pi's headunit: its video on a page,
which opens by itself, and the mouse on the picture as a finger. the mouse
wheel turns a rotary knob, the arrow keys tilt it and enter clicks it;
escape is back and h is home. ctrl-c stops it.

three tests check the phone certificate the way a strict car would, against
google's automotive link root, and skip unless given it. one walks the pi
image's order: the self-signed pair refused, then the google signed one
accepted and kept. the root and a phone
pair come from the android auto app, into ~/.cache/behead/certs, taken out
by [dexter](../dexter), checked out next to this repo, whose config for the
app reads the download and writes there:

    tools/fetch-apk.py          # verified download, to ~/.cache/behead/apk
    python3 ../dexter/dexter.py com.google.android.projection.gearhead   # the app's certs and key
    BEHEAD_GOOGLE_CA=~/.cache/behead/certs/ca.crt \
        BEHEAD_GOOGLE_CERT=~/.cache/behead/certs/carservice.crt \
        BEHEAD_GOOGLE_KEY=~/.cache/behead/certs/carservice.key make test-e2e

the extraction also recovers the pair the app itself presents, `carservice.crt`
and `carservice.key`: a car holding the root accepts it until it expires on
2026-12-23. the key is not in the app in the clear. it sits aes encrypted in
the app's code, keyed by a stream the app mixes from its own certificates, and
the tool replays that to read it.

two pass only with a current google signed phone certificate (R8).
