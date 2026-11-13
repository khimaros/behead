# certificates

a headunit and the phone show each other a certificate before anything else
happens. behead accepts any headunit's. the headunit may check the phone's
against Google's Automotive Link authority, and only a certificate Google
signed gets past such a headunit.

| headunit | self-signed certificate |
|---|---|
| openauto | accepted |
| 2021 Mercedes Sprinter | accepted |
| Google DHU 2.0 | refused, after the handshake; Google's accepted |
| Kia EV6 | refused, after the handshake |

## the certificates on offer

`--cert` and `--key`, repeated, or `--cert-dir` with `NAME.crt` and
`NAME.key` pairs, give the server an ordered list. a headunit says nothing
about which authority it trusts before it refuses, so when it refuses a
certificate its next connection gets the next one, starting over after the
last. the one it accepts stays in use.

the RPi4 image offers Google's certificate first when it has been
extracted, and always carries a self-signed one behind it: see
[on the RPi4](#on-the-rpi4). the ESP32-P4 firmware is built with one
certificate: `PHONE_CERT` and `PHONE_KEY`, else the extracted `carservice`
pair, else a self-signed one.

## extracting Google's certificate

the Android Auto app carries a phone certificate Google signed,
`carservice.crt`, with its key, and the root that signs it. they are taken
out of the app by [dexter](https://github.com/khimaros/dexter), checked out
next to this repo:

1. download the app, verified against GrapheneOS's signed metadata, to
   `~/.cache/behead/apk`:

        tools/fetch-apk.py

2. in dexter, create `configs/com.google.android.projection.gearhead.toml`,
   as described in
   [AACS issue 15](https://github.com/tomasz-grobelny/AACS/issues/15#issuecomment-5983404974)
3. run dexter on the app. it writes `ca.crt`, `carservice.crt` and
   `carservice.key` to `~/.cache/behead/certs`:

        cd ../dexter && python3 dexter.py com.google.android.projection.gearhead

4. rebuild and flash the image, or the firmware:

        make image-rpi SESSION=launcher && make flash-rpi
        make build-esp32 flash-esp32

the key is not in the app in the clear. it sits AES encrypted in the app's
code, keyed by a stream the app mixes from its own certificates, and dexter
replays that to read it. the certificate expires: the current one on
2026-12-23, after which a newer app has to be extracted again.

## on the RPi4

the service offers the car every pair in `/etc/behead/certs`, in name order.
`make image-rpi` fills it:

- `50-NAME` for every pair in `~/.cache/behead/certs` (`CERT_CACHE=...` for
  another directory), such as `50-carservice`. a pair gone from the cache
  leaves the image again
- `90-self-signed`, always, for a headunit that checks nothing
- anything dropped into `rpi/overlay/etc/behead/certs` before building,
  named so it sorts where it should be tried

Google's pairs go ahead of the self-signed one because the RPi4 forgets the
accepted certificate at every boot. a strict car offered the self-signed
one first would refuse the first connection after each, and show "phone not
responding" until the cable is plugged in again. a car that checks nothing
takes Google's as readily. the build checks that each key belongs to its
certificate and warns about expired ones: an expired Google certificate is
refused by a strict car, and the self-signed one behind it is too.

## reading the TLS log

`journalctl -u behead -f` on the RPi4 shows each handshake:

    tls: presenting certificate subject O=..., issuer O=..., valid FROM to UNTIL (FILE)
    tls: headunit offers TLSv1_2 with cipher suites TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256 ...
    tls: handshake complete: TLSv1_2 TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384, headunit certificate subject ...

the first line names the certificate offered on this connection; check
whether it has expired. the second lists everything the headunit can speak;
rustls only implements the ECDHE suites with AES-GCM or ChaCha20, so a list
without any of those cannot work. if the headunit refuses the certificate
during the handshake, the session ends with

    tls: the headunit rejected the phone certificate (alert UnknownCA)

where the alert names its reason (UnknownCA: not signed by an authority it
trusts; CertificateExpired; BadCertificate). any other TLS failure is logged
as `tls: ` followed by the error.

some headunits finish the handshake with any certificate and refuse it only
afterwards, in the message that ends authentication. the Kia EV6 and the
DHU do this to the self-signed certificate:

    session ended: auth: the headunit rejected the phone certificate after the TLS handshake (status -3, authentication failure)

either way, with more than one certificate the next connection gets the
next one:

    tls: the headunit refused the phone certificate, the next connection gets certificate N of M

and with only one there is nothing else to try: extract Google's, above.

## testing against Google's root

three end-to-end tests check the phone certificate the way a strict car
would, against the extracted root, and skip without it. one builds the
RPi4 image's certificates and checks that Google's, first, is accepted on
the first connection. two pass only with a current Google signed
certificate (R8):

    BEHEAD_GOOGLE_CA=~/.cache/behead/certs/ca.crt \
        BEHEAD_GOOGLE_CERT=~/.cache/behead/certs/carservice.crt \
        BEHEAD_GOOGLE_KEY=~/.cache/behead/certs/carservice.key make test-e2e

`make test-dhu` checks the same against the DHU, which verifies like a car:
see [test](../README.md#test).
