# requirements

DRAFT: transcribed from the product owner's requests on 2026-10-02.
awaiting review; edit freely.

## product

R1. implement the phone side of android auto (the "server") in rust, so a
    general purpose computer can present a custom interface on a car headunit.

R2. target host: raspberry pi 4 running linux. laptop and esp32 are
    DEFERRED: the rpi4 comes first.

R3. on rpi4 and laptop, any application (kodi for example) can be shown on the
    headunit as HD video at a stable framerate.

R4. DEFERRED with esp32: a simple touch interface is enough there.

R5. touch input from the headunit reaches the application being shown.

R6. production use is an rpi4 or laptop plugged into a 2021 sprinter van
    headunit. that headunit is usb only, so the wired usb-c connection is
    the priority.

R13. mobile linux phones (droidian, mobian on pinephone / pinephone pro) are
     a target, after the rpi4.

R14. the headunit can show a desktop: phosh, gnome-shell, or a single app
     full screen (kiosk).

R15. android apps run through waydroid, including f-droid apps such as
     osmand~.

R16. audio plays through the headunit, and the headunit microphone reaches
     applications.

## operations

R7. the software owns all kernel usb gadget configuration and teardown.
    no manual configfs steps, and no state left behind after exit.

R8. an automated process obtains the android auto certificates from a
    production device or its apks (grapheneos apks are a candidate source).

R9. where real headunits or reference implementations disagree with published
    protocol documentation, behaviour is selectable (a quirks mode).

## testing

R10. the whole system is testable end-to-end in a virtual machine on a laptop.

R11. an rpi4 attached to the laptop can be flashed and tested with the laptop
     acting as the headunit.

R12. rpi4 images are built with `../raspi-provision`, which keeps the system
     safe against sudden power loss.
