# android auto phone-side protocol: documentation survey and AACS disagreements

date: 2026-10-02. all quotes were pulled with curl and read as plain text unless
noted. raw copies of every fetched page are in /tmp/ref/web/.

sources compared:

- AACS: github.com/tomasz-grobelny/AACS (HEAD 4a18a05, local "checkpoint" on top of upstream faa1cf2)
- aasdk: /tmp/ref/aasdk (f1xpl/aasdk 046b3b3, 2018-07-17)

terms: MD = mobile device (phone), HU = head unit, AOA/AOAP = android open
accessory protocol, AAP/GAL = the projection protocol above AOA.

## summary of the main findings

1. the only officially PUBLISHED spec is AOA 1.0/2.0. it covers the USB mode
   switch and nothing above it.
2. no public spec for the projection layer was found. the belief that it is
   partner-only is supported by evidence (a guide marked "Google Confidential &
   Proprietary", and developers.google.com/cars pointing partners to "your
   Google point of contact"), but no official page states the licensing terms
   outright.
3. a leaked official document exists (Head Unit Integration Guide 1.3.0, 2016).
   it confirms: HU is TLS client, MD is TLS server, TLS 1.2 with client auth,
   and the HU VERIFIES the MD certificate against the google CA.
4. the AACS phone certificate (AAServer/ssl/android_auto.crt) EXPIRED on
   2026-02-18. aasdk/openauto does not verify it, so the rig still works. a real
   HU that follows the guide should reject it. this is the highest risk item.
5. AACS and aasdk agree on the wire for framing, message ids and the handshake
   order. most disagreements are AACS omissions: no receive-side reassembly, no
   video focus request, hardcoded video config index, media sent before start,
   exit on any unknown control message.

## 1. official documentation

### 1.1 AOA 1.0 (published, authoritative)

URL: https://source.android.com/docs/core/interaction/accessories/aoa

VID/PID in accessory mode:

> The vendor ID should match Google's ID (0x18D1). If the device is already in
> accessory mode, the product ID should be 0x2D00 or 0x2D01 and the accessory
> can establish communication with the device through bulk transfer endpoints
> using its own communication protocol

> Note: 0x2D00 is reserved for Android-powered devices that support accessory
> mode. 0x2D01 is reserved for devices that support accessory mode as well as
> the Android Debug Bridge (ADB) protocol, which exposes a second interface
> with two bulk endpoints for ADB.

VID/PID before the switch:

> devices that support accessory mode (but are not in accessory mode) initially
> report the device manufacturer vendor and product IDs instead of the AOA
> vendor and product IDs.

request 51:

> Send a 51 control request ("Get Protocol") to determine if the device
> supports the Android accessory protocol. If the device supports the protocol,
> it returns a non-zero number that represents the supported protocol version.

    requestType:    USB_DIR_IN | USB_TYPE_VENDOR
    request:        51
    value:          0
    index:          0
    data:           protocol version number (16 bits little endian sent from the
                    device to the accessory)

request 52:

    requestType:    USB_DIR_OUT | USB_TYPE_VENDOR
    request:        52
    value:          0
    index:          string ID
    data            zero terminated UTF8 string sent from accessory to device

> The following string IDs are supported, with a maximum size of 256 bytes for
> each string (must be zero-terminated with \0).

    manufacturer name:  0
    model name:         1
    description:        2
    version:            3
    URI:                4
    serial number:      5

request 53:

    requestType:    USB_DIR_OUT | USB_TYPE_VENDOR
    request:        53
    value:          0
    index:          0
    data:           none

> After completing these steps, the accessory should wait for the connected USB
> device to re-introduce itself on the bus in accessory mode, then re-enumerate
> connected devices.

endpoint layout:

> 0x2D00 has one interface with two bulk endpoints for input and output
> communication.
> 0x2D01 has two interfaces with two bulk endpoints each for input and output
> communication. The first interface handles standard communication and the
> second interface handles ADB communication. To use an interface, locate the
> first bulk input and output endpoints, set the device configuration to a
> value of 1 with a SET_CONFIGURATION (0x09) device request, then communicate
> using the endpoints.

the page does NOT specify interface class/subclass/protocol, endpoint
addresses, or wMaxPacketSize.

### 1.2 AOA 2.0 (published, authoritative)

URL: https://source.android.com/docs/core/interaction/accessories/aoa2

> Android devices that support only the features in AOAv1 must return 1 as the
> protocol version; devices that support the additional feautres in AOAv2 must
> return 2 as the protocol version. AOAv2 is backward-compatible with AOAv1

product IDs: 0x2D00 accessory, 0x2D01 accessory + adb, 0x2D02 audio,
0x2D03 audio + adb, 0x2D04 accessory + audio, 0x2D05 accessory + audio + adb.

> Product IDs used in AOAv1 (0x2D00 and 0x2D01) continue to be supported in
> AOAv2.

new requests: ACCESSORY_REGISTER_HID 54, ACCESSORY_UNREGISTER_HID 55,
ACCESSORY_SET_HID_REPORT_DESC 56, ACCESSORY_SEND_HID_EVENT 57,
SET_AUDIO_MODE 58 (value 0 or 1, index 0, no data).

> This command must be sent before sending the ACCESSORY_START command for
> entering accessory mode.

> Caution: AOAv2 audio support has been deprecated in Android 8.0.

overview page: https://source.android.com/docs/core/interaction/accessories/protocol

> AOAv1. Supports generic accessory communication and adb debugging.
> AOAv2. Supports human interface device (HID) capabilities. Available in
> Android 4.1 (API Level 16) and higher.

### 1.3 android kernel f_accessory.c (official google source code, not prose docs)

this is the gadget driver real phones use. it fills the gaps in the AOA pages.

URL: https://android.googlesource.com/kernel/common/+/refs/heads/android12-5.10/drivers/usb/gadget/function/f_accessory.c
(fetched with ?format=TEXT. the same path on android-4.14-stable and
android-mainline returned an error page, so only android12-5.10 was read.)
header: .../android12-5.10/include/uapi/linux/usb/f_accessory.h

- line 51: `#define PROTOCOL_VERSION    2`
- line 48: `#define BULK_BUFFER_SIZE    16384`
- line 49: `#define ACC_STRING_SIZE     256`
- lines 142-150: interface is `bNumEndpoints = 2`,
  `bInterfaceClass = USB_CLASS_VENDOR_SPEC`,
  `bInterfaceSubClass = USB_SUBCLASS_VENDOR_SPEC`, `bInterfaceProtocol = 0`
- lines 202-216: high speed bulk IN and OUT `wMaxPacketSize = cpu_to_le16(512)`
- lines 218-230: full speed bulk IN and OUT have NO wMaxPacketSize set (left to
  usb_ep_autoconfig, which caps full speed bulk at 64)
- lines 152-200: super speed and super speed plus `wMaxPacketSize = 1024`,
  `bMaxBurst = 6`
- line 265: interface string "Android Accessory Interface"
- lines 996-1064: request 53 and 52 are only accepted with
  `bRequestType == (USB_DIR_OUT | USB_TYPE_VENDOR)`, request 51 only with
  `(USB_DIR_IN | USB_TYPE_VENDOR)`. request 51 writes
  `*((u16 *)cdev->req->buf) = PROTOCOL_VERSION` and clears all stored strings.
  requests 54-58 are handled. strings longer than 255 are truncated
  (lines 455-456).
- uapi header lines 22-29: `USB_ACCESSORY_VENDOR_ID 0x18D1`,
  `USB_ACCESSORY_PRODUCT_ID 0x2D00`, `USB_ACCESSORY_ADB_PRODUCT_ID 0x2D01`

### 1.4 desktop head unit (published, does not document the wire protocol)

URL: https://developer.android.com/training/cars/testing/dhu

> Android Auto supports connecting to DHU version 2.x with the Android Open
> Accessory (AOA) protocol, using the following command:
> ./desktop-head-unit --usb

> On the development machine, run the following adb command to forward socket
> connections from the development machine's port 5277 to the same port number
> on the mobile device. This configuration lets the DHU connect to the head
> unit server running on the mobile device over a TCP socket.
> adb forward tcp:5277 tcp:5277

> By default, the head unit server connects over port 5277. To override the
> host or port, use the --adb=<[localhost:]port> flag

what it gives us: TCP port 5277 for the phone's developer "head unit server",
and confirmation that the DHU reaches the phone over plain AOA. it has no
framing, TLS, version or channel details. a grep of the page text for
"wireless" found no transport description.

### 1.5 projection protocol itself: nothing published

searched source.android.com, developer.android.com, developers.google.com.
found only high level descriptions:

URL: https://source.android.com/docs/automotive/start/what_automotive

> Android Auto is a platform running on the user's phone, projecting the
> Android Auto user experience to a compatible in-vehicle infotainment system
> over a USB connection.

URL: https://developers.google.com/cars

> For information about different types of Android for Cars partners and the
> early access available to them, talk to your Google point of contact.

no page with framing, TLS, version negotiation, service discovery or channel
definitions was found. no official statement of the licensing terms was found
either; "OEM-only under license" remains an inference from the two quotes above
plus the confidentiality marking in 1.6. partner portals were not fetched (they
need a login).

### 1.6 Head Unit Integration Guide 1.3.0 (official origin, NOT officially published)

this is a google-authored document that leaked through a search engine cache.
treat it as accurate for 2016 and as not redistributable.

mirror: https://milek7.pl/.stuff/galdocs/huig13_cache.html
the cache header names the origin as
http://docs.ulmt.com/Android/Android%20Auto%20Protocol%201.3.pdf (not fetched).

front matter:

> Head Unit Integration Guide Version 1.3.0 Last Updated: 2016-08-10
> Google Confidential & Proprietary

AOA strings the HU sends (section "Configuring AOAP"):

> An Android MD identifies a vehicle as a AAP receiver based on accessory
> identifiers: Manufacturer Name Android / Model Name Android Auto /
> Description Android Auto / AAP Protocol Version 1.0 / URI <Empty> /
> Serial Number <Empty>

USB:

> HU implementations MUST support USB 2.0 and AOAP 1.0 or 2.0. The HU operates
> as a USB Host and AOA accessory.

> the HU MUST attempt AOAP initiation regardless of connected MD USB Vendor ID
> (VID) and Product ID (PID).

> After the device is in accessory mode, the vendor ID is 0x18D1 (Google) and
> the product ID is 0x2D00 or 0x2D01.

> AOAP 2.0 adds extra product IDs for an OPTIONAL USB audio interface. If audio
> support is also enabled by the head unit, then product IDs 0x2D04 and 0x2D05
> should also be supported. Android Auto requires only the accessory
> functionality.

connection start:

> After establishing an AOAP connection with the MD, the HU initiates
> negotiation of the AAP connection by sending a Version Request to the MD [...]
> The HU MUST NOT send additional Version Requests or ping requests [...] while
> awaiting a response from the MD. Encrypted packets MUST NOT be sent from HU
> to MD until authentication is complete.

> If the MD supports AAP and authentication succeeds, the HU receives a Service
> Discovery Request [...] If the MD does not support AAP, then a ByeBye message
> MAY be received by the HU.

TLS:

> The AAP protocol uses industry standard TLS 1.2 with Client Authentication to
> secure communications between the head unit (TLS Client) and mobile device
> (TLS Server).

> If the receiver library cannot verify the sender certificate, the HU
> terminates the connection and disconnects AOAP

> The HU and MD establish mutual trust using two (2) certificates signed by the
> Google CA: HU certificate sent to MD, which verifies it to ensure the HU is a
> valid AAP receiver. MD certificate sent to HU, which verifies it to ensure
> the MD is a valid AAP device.

> Public/Private Key Format RSA with 2048 bit keys / Digital Certificate Format
> X.509

> SSL authentication might fail with error "Certificate not yet valid" or
> "Certificate expired" if the internal clock on the HU is incorrect

video focus:

> During connection setup video focus MUST NOT be granted to the MD by the HU
> (VIDEO_FOCUS_PROJECTED) until VideoSink::setupCallback() has been called.

> After establishing an AAP connection, including the MD announcing the video
> encoder is ready [...], the HU remains in native mode but the MD can request
> video focus immediately. The HU MUST notify the MD immediately when video
> focus changes.

> In response to a video focus request, the HU sends a video focus notification
> to the MD, indicating the new video focus mode.

video:

> H.264 video sent from MD contains only I frames and P frames (buffering set
> to minimum).

the guide names 800x480 at 30 FPS as the required baseline ("the reason for the
required resolution of 800x480 at 30 FPS").

the guide (2016) has no wireless projection section. it does not give frame
header bytes, message ids or proto field numbers; those live in the receiver
library, which is not public.

### 1.7 wireless projection

no official documentation found. see section 2 for unofficial details.

## 2. unofficial sources (secondary, reverse engineered)

- **milek7 "Android Auto protocol research"**
  https://milek7.pl/.stuff/galdocs/readme.md
  protos.proto and common.proto extracted from google's DHU binary with pbtk,
  plus a wireshark dissector (androidauto.lua). this is the closest thing to
  google's own schema, because the names come from descriptors embedded in the
  binary. it reports `PROTOCOL_MAJOR_VERSION = 1`, `PROTOCOL_MINOR_VERSION = 6`,
  `WIFI_PORT = 30515` (enum GalConstants). quote: "Their official emulator
  binary works only over TCP [...] Protocol is the same however, and stream
  data is just pushed through bulk endpoints."
  caveat: the dissector treats bytes 2..5 of a FIRST frame as one 4-byte
  length, which contradicts aasdk and AACS (2-byte frame length + 4-byte total
  length). aasdk and AACS agree with each other and interoperate.
- **aa-proxy-rs** https://github.com/aa-proxy/aa-proxy-rs (GPL-2.0, rust,
  pushed 2026-10-01). src/protos/protos.proto matches the milek7 dump on the
  points checked (sint32 priority, VIDEO_FPS_60 = 1, WIFI_PORT = 30515,
  protocol 1.6). also has WifiStartRequest {ip_address = 1, port = 2} and
  WifiInfoResponse {ssid, key, bssid, security_mode, access_point_type}, and
  the wireless RFCOMM profile UUID 4de17a00-52cb-11e6-bdf4-0800200c9a66
  (src/bluetooth.rs:381). its README says MITM mode needs hu_key/hu_cert/
  md_key/md_cert/galroot_cert and points at AACS for the key files. its
  frame flag constants: FIRST 1<<0, LAST 1<<1, CONTROL 1<<2, ENCRYPTED 1<<3
  (src/mitm.rs:142-146).
- **opencardev/aasdk** https://github.com/opencardev/aasdk (branch newdev).
  has protobuf/aap_protobuf with subdirs aaw, channel, service, shared. only
  the directory listing was fetched; the proto contents were NOT read.
- **f1xpl/aasdk** (the checkout in /tmp/ref/aasdk). HU side only. its proto
  field NAMES are guesses and several are wrong against the DHU dump (see
  table).
- TCP port 5288 for wireless came up in a web search summary of third-party
  projects. it was not found in the aa-proxy-rs sources grepped, and the DHU
  dump says 30515. the port is carried in WifiStartRequest, so treat it as
  negotiated. unverified.

## 3. disagreements

status column: V = verified by reading both sides, I = inference (one side
read, the other reasoned or taken from memory).

handling column: AACS = copy AACS, DOC = follow the doc or kernel driver,
aasdk = follow aasdk, QUIRK = make it configurable.

### 3.1 AOA handshake and USB descriptors

| # | area | AACS behaviour (file:line) | official doc / aasdk | impact on a real HU | handling | status |
|---|------|----------------------------|----------------------|---------------------|----------|--------|
| U1 | request 51 reply | writes `"\002\000"`, i.e. version 2 little endian. does not check bRequestType or wLength. ModeSwitcher.cpp:27-29 | AOA2: v2 devices "must return 2". f_accessory.c:1049-1054 only answers when type is IN+VENDOR. aasdk accepts 1 or 2 (AccessoryModeProtocolVersionQuery.cpp:66) | none for the reply value | DOC (return 2, check bRequestType 0xC0) | V |
| U2 | claims AOA v2 but implements none of 54-58 | no branch for 54-58. ModeSwitcher.cpp:27-38 | AOA2 page defines 54-58; f_accessory.c:1009-1048 handles them. HUIG: "Android Auto requires only the accessory functionality" | low. see U4 for the side effect | DOC: ack 58 with value 0, stall or ack 54-57 | V |
| U3 | request 52 strings | never parsed or checked. the setup branch only prints wIndex (ModeSwitcher.cpp:30-34). the data stage is consumed by the NEXT read() in the loop (line 79), then the string bytes are passed to handleSwitchMessage and reinterpreted as usb_functionfs_event structs (line 23-24); they only get printed because byte 8 is never FUNCTIONFS_SETUP for ASCII | AOA1: index 0-5, zero terminated UTF8, max 256. HUIG: manufacturer "Android", model "Android Auto". aasdk sends Android / Android Auto / Android Auto / 2.0.1 / https://f1xstudio.com / HU-AAAAAA001 (AccessoryModeQueryFactory.cpp:47-69) | none (phone side may ignore them), but the parsing is accidental | DOC: read the data stage explicitly, keyed on the pending setup; log strings; optionally require manufacturer "Android" and model "Android Auto" | V |
| U4 | end of switch loop | request 53 returns 0 from handleSwitchMessage, but the return value is discarded (ModeSwitcher.cpp:86-89 tests the stale `length`). the loop really ends when the next read() returns 0, which FunctionFS does for ANY pending OUT setup with wLength 0 (f_fs.c ffs_ep0_read, FFS_SETUP_PENDING branch), and checkError maps 0 to -1 (src/utils.cpp:17-18) | AOA1: only request 53 starts accessory mode. AOA2: request 58 "must be sent before sending the ACCESSORY_START" and has no data | an HU that sends 58 (or 54/55) before 53 makes AACS tear down the gadget early, before 53 arrives. unknown how many HUs do this | DOC: switch only on request 53, and ack other zero-length requests without leaving the loop | V (mechanism), I (which HUs send 58) |
| U5 | VID/PID before the switch | 0x12d1:0x107e (huawei IDs), strings "TAG"/"AAServer". ModeSwitcher.cpp:47-48 | AOA1: device reports "the device manufacturer vendor and product IDs". HUIG: HU MUST attempt AOAP "regardless of connected MD USB Vendor ID (VID) and Product ID (PID)". aasdk tries every device (USBHub.cpp:116-139) | low | QUIRK (configurable VID/PID/strings, default to AACS values) | V |
| U6 | VID/PID after the switch | 0x18d1:0x2d00. AaCommunicator.cpp:490 | AOA1 and HUIG: 0x18D1, 0x2D00 or 0x2D01. aasdk accepts both (USBHub.cpp:89-93) | none | AACS | V |
| U7 | extra mass storage function before the switch | 4 MiB empty backing file exposed as removable CD-ROM LUN, added before the FFS function. ModeSwitcher.cpp:50-56, 64-66; MassStorageFunction.cpp:16-27 | not in any doc. real phones expose MTP/ADB/charging before the switch. HUIG lists mass storage as one of several possible MD modes | unknown. an HU could try to mount it or show a media source before or instead of trying AOA | QUIRK, default OFF, enable only if an HU refuses a vendor-only device | I |
| U8 | pre-switch interface | one vendor interface, 0 endpoints, string "Android Accessory Interface", flag FUNCTIONFS_ALL_CTRL_RECIP. descriptors.cpp:99-136 | AOA does not define a pre-switch interface. ALL_CTRL_RECIP is required for FunctionFS to see device-recipient vendor requests (f_fs.c:3393-3415) | none | AACS (keep ALL_CTRL_RECIP; the string is cosmetic) | V |
| U9 | accessory interface class | class 0xFF, subclass 0xFF, protocol 0, 2 bulk endpoints, iInterface "Android Accessory Interface". descriptors.cpp:30-39, 138 | identical to f_accessory.c:142-150, 265. aasdk only checks bNumEndpoints >= 2 on interface 0 and picks endpoints by direction (AOAPDevice.cpp:36-45, 71) | none | AACS | V |
| U10 | full speed wMaxPacketSize | 512 for both bulk endpoints in the FULL SPEED descriptors. descriptors.cpp:46, 55 | f_accessory.c:218-230 leaves full speed unset (autoconfig caps at 64, epautoconf.c:163-171). USB 2.0 full speed bulk max is 64. f_fs.c:2910-2946 restores the user value after autoconfig, so the invalid 512 reaches the host | only when the link runs at full speed (bad cable, FS-only port). host may reject the config or babble | DOC: 64 for full speed | V |
| U11 | high speed wMaxPacketSize | 512. descriptors.cpp:77, 86 | f_accessory.c:207, 215: 512 | none | AACS | V |
| U12 | super speed descriptors | none (flags are FS + HS only). descriptors.cpp:23-24 | f_accessory.c:152-200 has SS and SS+ (1024, burst 6). HUIG only requires USB 2.0 | none on USB 2.0 UDCs (pi, most laptops with dwc2/dummy_hcd). on an SS UDC the gadget is limited to high speed | AACS; add SS later if a target UDC needs it | V (descriptors), I (effect) |
| U13 | request 51 after the switch | accessory-mode descriptors omit FUNCTIONFS_ALL_CTRL_RECIP, so vendor device requests are not delivered and ep0 stalls. descriptors.cpp:23-24; handleEp0Message ignores setup events (AaCommunicator.cpp:453-463) | f_accessory.c answers 51 in any mode. AOA1 says an accessory that sees 0x18D1:0x2D00 need not send 51 again; aasdk does not (USBHub.cpp:116-119) | only if an HU re-queries the protocol on the accessory device | DOC: answer 51 in accessory mode too | V (AACS), I (HU behaviour) |
| U14 | USB suspend | FUNCTIONFS_SUSPEND on ep0 throws and the process exits. AaCommunicator.cpp:458-460 | HUIG: "the USB AOAP session is maintained even if the HU is not actively displaying projection content" | exits on any bus suspend | QUIRK: treat suspend as a soft event | V (AACS), I (impact) |
| U15 | configuration attributes | bmAttributes 0xc0 (self powered), bMaxPower 0x30. Configuration.cpp:13-14 | AOA overview: accessories "must provide 500mA at 5V for charging power". phones are bus powered | none expected | QUIRK (configurable, low priority) | I |

### 3.2 framing

| # | area | AACS behaviour (file:line) | official doc / aasdk | impact on a real HU | handling | status |
|---|------|----------------------------|----------------------|---------------------|----------|--------|
| F1 | flag bit 2 NAME | `MessageTypeFlags { Control = 0, Specific = 1 << 2 }`. include/enums.h:16-19 | aasdk: `MessageType { SPECIFIC = 0, CONTROL = 1 << 2 }` (MessageType.hpp:32-36). milek7 dissector and aa-proxy-rs also call 0x04 "Control" | none on the wire: AACS sets the bit on ChannelOpenRequest sent on a service channel (ChannelHandler.cpp:42-45), aasdk sets it on ChannelOpenResponse (VideoServiceChannel.cpp:57). AACS's names are inverted | aasdk naming: bit 2 set = control-type message (id < 0x8000) on a non-zero channel | V |
| F2 | encryption and frame type flags | Plain 0, Encrypted 1<<3; First 1, Last 2, Bulk 3, middle is 0 (unnamed). include/enums.h:5-14 | aasdk identical (EncryptionType.hpp:28-32, FrameType.hpp:30-36, MIDDLE = 0). DHU dump: FRAG_CONTINUATION 0, FRAG_FIRST 1, FRAG_LAST 2, FRAG_UNFRAGMENTED 3 | none | AACS | V |
| F3 | header layout | byte 0 channel, byte 1 flags, bytes 2-3 big endian frame length; FIRST frames add a 4-byte big endian total length (send side). AaCommunicator.cpp:429-438 | aasdk identical: 2-byte header, then 2 or 6 bytes of size (FrameHeader.hpp:46, FrameSize.cpp:60-86) | none | AACS | V |
| F4 | meaning of the two lengths | frame length = ciphertext bytes in this frame; total = plaintext size of the whole message. AaCommunicator.cpp:376, 425-437 | aasdk identical (MessageOutStream.cpp:126-146). aasdk as receiver ignores the total | none | AACS | V |
| F5 | max frame payload on send | 2000 plaintext bytes per frame, comment "it should work up to about 16k, but we might get some weird hardware issues". AaCommunicator.cpp:372-373 | aasdk: `cMaxFramePayloadSize = 0x4000` (MessageOutStream.hpp:60), splits when size >= 0x4000 (MessageOutStream.cpp:52). f_accessory BULK_BUFFER_SIZE 16384 | works, but about 8x more frames and TLS records per video frame | QUIRK: default 16384 plaintext bytes as aasdk, allow lowering to 2000 | V |
| F6 | multi-frame SEND | one TLS record per frame, frames of one message are never interleaved with another channel (single FIFO, message stays at the front). only encrypted messages are split; plain messages are sent whole with a 16-bit length. AaCommunicator.cpp:375-450 | aasdk at this commit rejects interleaving (MESSENGER_INTERTWINED_CHANNELS, MessageInStream.cpp:71-77) | none | AACS: do not interleave | V |
| F7 | multi-frame RECEIVE | not implemented. every frame is decrypted and dispatched as a full message. for a FIRST frame the 4-byte total length is not skipped, so it is fed to TLS as ciphertext. AaCommunicator.cpp:345-363 | aasdk reassembles FIRST/MIDDLE/LAST and uses the 6-byte size on FIRST (MessageInStream.cpp:80, 134-153). aasdk as HU sends split messages for payloads >= 16384 | breaks on any HU message >= 16 KiB (large service discovery response, long mic audio buffers). rare but fatal | aasdk: full reassembly per channel | V |
| F8 | frame vs USB transfer boundaries | requires each read() to start on a frame boundary and contain whole frames, else throws "nbytes<4+length". AaCommunicator.cpp:354-355 | aasdk treats the transport as a byte stream with 16384-byte chunks (Transport.cpp:33-78, DataSink.hpp:46) | fails if an HU splits a frame across bulk transfers | aasdk: stream parser with a carry buffer | V |
| F9 | decrypt | one SSL_read per frame into a 100 KiB buffer. AaCommunicator.cpp:204-218 | aasdk loops while SSL_pending (Cryptor.cpp:193-206) | data left behind if a frame carries more than one TLS record | aasdk: drain until no pending data | V |
| F10 | unknown channel | indexes channelHandlers[] without a null check for non-zero channels. AaCommunicator.cpp:157-160 | n/a | null dereference if the HU uses a channel id it did not advertise | reject with a logged error | V |

### 3.3 version exchange

| # | area | AACS behaviour (file:line) | official doc / aasdk | impact on a real HU | handling | status |
|---|------|----------------------------|----------------------|---------------------|----------|--------|
| V1 | request layout | reads u16 major, u16 minor big endian after the 2-byte id; ignores trailing bytes. AaCommunicator.cpp:86-89 | aasdk sends exactly that (ControlServiceChannel.cpp:47-50). DHU dump: an optional VersionRequestOptions protobuf may follow the 4 bytes | none | AACS, tolerate trailing bytes | V |
| V2 | response layout | id 2, u16 major, u16 minor, u16 status (0), sent Plain + Bulk on channel 0. AaCommunicator.cpp:76-84 | aasdk parses the same three u16 (ControlServiceChannel.cpp:176-181; status MATCH 0, MISMATCH 0xFFFF). DHU dump: STATUS_NO_COMPATIBLE_VERSION = -1, optional VersionResponseOptions may follow | none | AACS | V |
| V3 | version numbers | accepts only major 1, always answers 1.5 whatever minor was asked; throws (process exits) on other majors instead of answering a mismatch. AaCommunicator.cpp:90-93 | aasdk asks for 1.1 (Version.hpp:23-24) and does not act on the returned numbers. DHU dump constants: 1.6. HUIG AOA string "1.0" is the accessory version string, not this field | unknown whether any HU rejects 1.5 | QUIRK: configurable answer, default 1.5 as AACS (known to work on the author's car); answer 0xFFFF on unsupported major | V (code), I (HU reaction) |

### 3.4 TLS

| # | area | AACS behaviour (file:line) | official doc / aasdk | impact on a real HU | handling | status |
|---|------|----------------------------|----------------------|---------------------|----------|--------|
| T1 | role | TLS SERVER (SSLv23_server_method, SSL_set_accept_state, SSL_accept). AaCommunicator.cpp:275, 299, 306 | HUIG: "head unit (TLS Client) and mobile device (TLS Server)". aasdk is the client (SSLWrapper.cpp:72-79, 120-124) | none | AACS | V |
| T2 | protocol versions | any version the library allows except TLS 1.3 (SSL_OP_NO_TLSv1_3). AaCommunicator.cpp:306, 337 | HUIG: "industry standard TLS 1.2". aasdk: TLSv1_2_client_method on old OpenSSL, TLS_client_method otherwise | none. TLS 1.3 must stay off: its post-handshake messages do not fit the message-3 handshake transport | DOC: TLS 1.2 only | V |
| T3 | client certificate | requested (SSL_VERIFY_PEER) but never checked: the verify callback returns 1 and a missing cert is tolerated. AaCommunicator.cpp:336, 340-343 | HUIG: "TLS 1.2 with Client Authentication"; the MD verifies the HU cert against the google CA. aasdk always presents its cert (Cryptor.cpp:75-83) | none for us (we are lenient) | AACS: request the cert, do not verify by default; QUIRK to verify against a supplied root | V |
| T4 | phone (server) certificate | AAServer/ssl/android_auto.crt: subject O=CarService, issuer O=Google Automotive Link, RSA 2048, notAfter Feb 18 22:22:18 2026 GMT. EXPIRED as of 2026-10-02 (checked with openssl x509) | HUIG: "MD certificate sent to HU, which verifies it"; "If the receiver library cannot verify the sender certificate, the HU terminates the connection"; names the "Certificate expired" failure. aasdk does NOT verify (SSL_VERIFY_NONE, SSLWrapper.cpp:123) | HIGH. works against openauto, expected to fail on an HU that checks validity dates. a self-signed cert would fail the chain check too | no clean option: needs a current google-signed MD cert and key (AACS history shows they were refreshed in 2020, 2021, 2022). make cert and key paths configurable and surface TLS alerts clearly | V (expiry, aasdk), I (real HU enforcement, from the 2016 guide) |
| T5 | DH parameters | requires dhparam.pem (2048-bit, generated at build time) and throws without it; also SSL_CTX_set_ecdh_auto. AaCommunicator.cpp:313, 322-335; AAServer/CMakeLists.txt:60 | no doc. aasdk sets no cipher list. HUIG: RSA 2048 keys | DH params only matter if the HU offers DHE suites and no ECDHE. unknown which HUs do | QUIRK. note for rust: rustls has no DHE and no static RSA key exchange, so an HU that offers only those needs an OpenSSL-backed TLS stack | V (code), I (HU cipher lists) |
| T6 | handshake transport | each flight is sent as message id 3, Plain + Bulk, on channel 0, unfragmented. AaCommunicator.cpp:282-290 | aasdk same (ControlServiceChannel.cpp:55-62). HUIG: no encrypted packets before authentication completes | none | AACS | V |
| T7 | after the handshake | waits for AuthComplete (id 4) from the HU, ignores its status, then sends ServiceDiscoveryRequest encrypted. AaCommunicator.cpp:238-240 | aasdk sends AuthComplete PLAIN (ControlServiceChannel.cpp:64-71). DHU dump: AuthResponse {status = 1} | none; should check status == 0 | AACS plus a status check | V |

### 3.5 message fields

| # | area | AACS behaviour (file:line) | official doc / aasdk | impact on a real HU | handling | status |
|---|------|----------------------------|----------------------|---------------------|----------|--------|
| M1 | ChannelOpenRequest | `unknown_field = 1` (int32, sent as 0), `channel_id = 2`. proto/ChannelOpenRequest.proto:9-10; ChannelHandler.cpp:36-37 | aasdk: `int32 priority = 1; int32 channel_id = 2`. DHU dump: `required sint32 priority = 1; required int32 service_id = 2` | none while priority is 0 (same encoding). a non-zero int32 would be misread as zigzag | DOC (unofficial dump): sint32 priority, service_id | V |
| M2 | ChannelOpenResponse | status int32 = 1, value never checked. proto/ChannelOpenResponse.proto:9; ChannelHandler.cpp:25-27 | aasdk Status {OK 0, FAIL 1}. DHU dump MessageStatus: SUCCESS 0, negative error codes | a refused channel is treated as open | check status == 0 | V |
| M3 | media setup request raw bytes | `08 03`: field 1 varint 3. VideoChannelHandler.cpp:140-142 | aasdk: `AVChannelSetupRequest { uint32 config_index = 1 }`. DHU dump: `Setup { required MediaCodecType type = 1 }` with MEDIA_CODEC_VIDEO_H264_BP = 3 | none. the value is the codec type (H.264 baseline), not a config index; aasdk's name is wrong | DOC (unofficial dump) | V |
| M4 | MediaChannelSetupResponse | `unknown_field_1/2/3`, all required uint32; content never read by AAServer (only the message id is checked). proto/MediaChannelSetupResponse.proto:9-11; VideoChannelHandler.cpp:178-180 | aasdk: media_status = 1 (NONE 0, FAIL 1, OK 2), max_unacked = 2, repeated configs = 3. DHU dump: `Config { Status status = 1 (STATUS_WAIT 1, STATUS_READY 2); optional uint32 max_unacked = 2; repeated uint32 configuration_indices = 3 }` | AACS never learns whether the sink is ready, the ack window, or which config the HU picked. note aasdk's "FAIL = 1" is really WAIT | DOC: parse it, wait for READY, keep max_unacked and the indices | V |
| M5 | start indication raw bytes | `08 00 10 00`: field 1 = 0, field 2 = 0. VideoChannelHandler.cpp:154-158 | aasdk: `int32 session = 1; uint32 config = 2`. DHU dump: `Start { required int32 session_id = 1; required uint32 configuration_index = 2 }` | config index 0 is hardcoded while the encoder is hardcoded to 800x480@30 (VideoChannelHandler.cpp:74-78). wrong picture or decoder error on any HU whose video_configs[0] is not 480p | DOC: pick the index of a config the encoder matches, taken from service discovery and M4 | V (bytes), I (HU effect) |
| M6 | ServiceDiscoveryRequest | `model = 4` ("AAServer"), `manufacturer = 5` ("TAG"). proto/ServiceDiscoveryRequest.proto:9-10; AaCommunicator.cpp:98-99 | aasdk: device_name = 4, device_brand = 5. DHU dump: small/medium/large_icon = 1-3, label_text = 4, device_name = 5, phone_info = 6 | cosmetic (name shown on the HU) | DOC (unofficial dump) | V |
| M7 | service descriptor | fields 10 and 13 are "unknown_channel_1/2". proto/Channel.proto:27-29 | aasdk lacks them. DHU dump: 7 radio, 9 media_playback, 10 phone_status, 11 media_browser, 13 generic_notification, 14 wifi_projection | none | DOC (unofficial dump) | V |
| M8 | media sink descriptor | `media_type` enum {Audio 1, Video 3}; every sink with value 3 overwrites the video channel number (last one wins) and starts its own gstreamer pipeline. proto/MediaStreamType.proto; AaCommunicator.cpp:113-119 | aasdk AVStreamType same. DHU dump: field 1 is MediaCodecType (PCM 1, AAC_LC 2, H264_BP 3, AAC_LC_ADTS 4, VP9 5, AV1 6, H265 7); sinks also carry display_id = 6 and display_type = 7 (MAIN 0, CLUSTER 1, AUXILIARY 2) | on an HU with a cluster display there are several H.264 sinks; AACS keeps the last one seen | DOC: select display_type MAIN (or absent) | V (protos), I (HU effect) |
| M9 | closed proto2 enums with `required` | VideoConfig.video_resolution and video_fps are required closed enums (H480 1, H720 2, H1080 3). proto/VideoConfig.proto:12-13 | DHU dump adds 1440p 4, 2160p 5 and portrait modes 6-9 | an unknown value makes the required field "missing"; AACS ignores the parse result so it limps on. in rust this must not be a hard error | use open enums or integers for everything received from the HU | V |
| M10 | frame rate enum | F30 = 1, F60 = 2. proto/VideoFps.proto:11-13 | aasdk: _30 = 1, _60 = 2. DHU dump and aa-proxy-rs: VIDEO_FPS_60 = 1, VIDEO_FPS_30 = 2 | AACS ignores the value (always 30 fps). the three sources disagree; the DHU dump is the stronger one | DOC (unofficial dump), verify on a real HU | V |
| M11 | PingRequest | timestamp = 1, `unknown_1 = 2` (int32). proto/PingRequest.proto:9-10 | aasdk: timestamp only. DHU dump: timestamp = 1, bool bug_report = 2, bytes data = 3; PingResponse adds bytes data = 2 | none | DOC (unofficial dump); echo data if present | V |
| M12 | input binding request | repeated ButtonCode enum, proto2, so UNPACKED on the wire; unknown keycodes advertised by the HU are dropped by the closed enum. proto/InputChannel.proto:15-18; InputChannelHandler.cpp:29-30 | aasdk: `repeated int32 scan_codes = 1`. DHU dump: `repeated int32 keycodes = 1 [packed = true]` | protobuf parsers accept both encodings, so this should interoperate | DOC: packed int32 | V (protos), I (interop) |

### 3.6 touch

| # | area | AACS behaviour (file:line) | official doc / aasdk | impact on a real HU | handling | status |
|---|------|----------------------------|----------------------|---------------------|----------|--------|
| I1 | touch action values | Press 0, Release 1, Drag 2, Down 5, Up 6. proto/TouchAction.proto:9-13 | aasdk: PRESS 0, RELEASE 1, DRAG 2 only (TouchActionEnum.proto:27-29). DHU dump PointerAction: ACTION_DOWN 0, ACTION_UP 1, ACTION_MOVED 2, ACTION_POINTER_DOWN 5, ACTION_POINTER_UP 6 | AACS matches the DHU dump. aasdk is incomplete (no multi-touch pointer down/up). AACS's NAMES for 5 and 6 are misleading: they are secondary pointer down/up | AACS values, DHU names | V |
| I2 | touch event fields | `touch_location = 1`, `touch_action = 3` (required closed enum); no field 2. proto/TouchEvent.proto:12-13 | aasdk and DHU dump: `action_index = 2` (which pointer the action applies to), action optional | multi-touch cannot be decoded without action_index; an unknown action value fails the required check | DOC: add action_index, make action optional and open | V |
| I3 | input event fields | timestamp = 1, touch_event = 3, buttons_event = 4. proto/InputEvent.proto:12-14 | aasdk adds disp_channel = 2, absolute = 5, relative = 6. DHU dump adds touchpad_event = 7 | rotary and touchpad input are lost | DOC | V |
| I4 | when the input channel is opened | only when a unix socket client writes to it; then ChannelOpenRequest + binding request, both blocking. InputChannelHandler.cpp:67-77 | a phone opens its channels right after service discovery. aasdk only sends touch after the channel is open | no touch until a client attaches (this is the open item in AACS WORKING.md) | open input (and the other advertised channels) right after service discovery | V (AACS), I (phone behaviour) |

### 3.7 media acks, video focus, ping, other control messages

| # | area | AACS behaviour (file:line) | official doc / aasdk | impact on a real HU | handling | status |
|---|------|----------------------------|----------------------|---------------------|----------|--------|
| P1 | media ack | MediaAckIndication (0x8004) is accepted and discarded; no window, no back-pressure. VideoChannelHandler.cpp:184-185 | aasdk: `AVMediaAckIndication { session = 1, value = 2 }` and max_unacked in the setup response. DHU dump: `Ack { session_id = 1; ack = 2; repeated receive_timestamp_ns = 3 }` | video is pushed regardless of decoder progress. tolerable at 480p30, risky on slow HUs | DOC: stop sending when unacked frames reach max_unacked | V (code), I (HU effect) |
| P2 | video focus | never sends a focus request (0x8007). sends StartIndication on EVERY VideoFocusIndication (0x8008) without reading the mode. VideoChannelHandler.cpp:181-183 | HUIG: "the HU remains in native mode but the MD can request video focus immediately"; focus MUST NOT be granted before setup. aasdk/DHU dump: request {disp_channel 1, mode 2, reason 3}, notification {focus 1, unsolicited 2}; modes PROJECTED 1, NATIVE 2, NATIVE_TRANSIENT 3, PROJECTED_NO_INPUT_FOCUS 4 | on an HU that waits for a request, the screen stays native until the user picks android auto. when the HU takes focus back (NATIVE), AACS sends Start again instead of stopping | DOC: send a focus request after setup; Start on PROJECTED, Stop (0x8002) on NATIVE | V (code, guide), I (HU effect) |
| P3 | media before start | video frames are sent as soon as the channel is open and setup is answered, independent of StartIndication. VideoChannelHandler.cpp:30-46, 127-133 | aasdk expects START_INDICATION before media (VideoServiceChannel.cpp:101-112 dispatches them separately; openauto was not checked out, so its handling was not read) | an HU may drop or fault on media that precedes Start | gate media on Start | V (AACS), I (HU effect) |
| P4 | media message ids | 0x0000 "MediaWithTimestampIndication" with 8-byte big endian microseconds when the gstreamer buffer has a pts, else 0x0001 "MediaIndication". SPS/PPS are never sent separately. VideoChannelHandler.cpp:33-39 | aasdk uses the same two ids and treats both as media. DHU dump: 0 MEDIA_MESSAGE_DATA, 1 MEDIA_MESSAGE_CODEC_CONFIG | id 1 is the codec config slot. an HU decoder may want SPS/PPS there first | DOC (unofficial dump): send SPS/PPS as id 1 once, then data as id 0 with timestamp | V (ids), I (HU effect) |
| P5 | ping | answers PingRequest (0x0b) with PingResponse (0x0c) echoing the timestamp, always Encrypted. never originates pings and has no timeout. AaCommunicator.cpp:244-246, 253-269 | aasdk sends PingRequest PLAIN (ControlServiceChannel.cpp:118-125) and at this commit does not handle an incoming PING_REQUEST at all (lines 141-171). HUIG mentions HU-originated pings only. DHU dump has STATUS_PING_TIMEOUT = -25 | fine as a responder. aasdk accepts the encrypted reply because decryption follows the per-frame flag | AACS for replies; optional phone-originated ping as a QUIRK, default off | V |
| P6 | unknown control messages | any id on channel 0 outside {1, 3, 4, 6, 0x0b, 0x0e, 0x13} throws and the process exits. AaCommunicator.cpp:247-250 | aasdk as HU can send SHUTDOWN_REQUEST 0x0f / RESPONSE 0x10 (ControlServiceChannel.cpp:91-107). DHU dump lists 9, 15-17, 20-26. HUIG describes ByeBye | an HU-initiated ByeBye or any newer notification kills AAServer instead of a clean reply | DOC: answer ByeBye (0x0f) with 0x10, log and ignore unknown ids | V |
| P7 | channels other than video and input | never opened by AAServer itself; messages are relayed to socket clients (AAClient proxy mode). AaCommunicator.cpp:126-128; DefaultChannelHandler.cpp:14-19 | a phone opens sensor, audio and other sinks, and requests sensors | no night mode or driving status, no audio. acceptable for a video-only first cut | open at least the sensor channel later | V (AACS), I (need) |

## 4. open questions that only a real HU can answer

- whether current HUs enforce the MD certificate dates and chain (T4).
- whether any HU sends request 58 or 54-57 before 53 (U4), or re-sends 51 in
  accessory mode (U13).
- which version reply HUs accept (V3) and which frame rate numbering is right
  (M10).
- whether HUs offer ECDHE suites (T5).
- wireless: the TCP port and RFCOMM flow have no official source.

## 5. fetch log

fetched and read: the three AOA pages, the DHU page, what_automotive,
developers.google.com/cars, f_accessory.c / f_accessory.h / f_fs.c /
epautoconf.c on android12-5.10, milek7 readme + protos.proto + common.proto +
androidauto.lua + huig13_cache.html, aa-proxy-rs README + protos +
bluetooth.rs + mitm.rs (grep only).

failed or skipped: f_accessory.c on android-4.14-stable and android-mainline
(error page), aa-proxy-rs src/usb_gadget.rs (404), opencardev aap_protobuf
file contents (directory listing only), the original docs.ulmt.com PDF, any
partner portal.
