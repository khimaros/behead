E2E_DIR := tests/e2e
VM := behead-test
VM_TEST_DIR := /incant/tests/vm

# static aarch64 build for the rpi4. clang and rust-lld cross-compile it
# without an aarch64 gcc toolchain, and musl keeps it independent of the
# image's glibc.
RPI_TARGET := aarch64-unknown-linux-musl
RPI_BIN_DIR := target/$(RPI_TARGET)/release
RPI_SERVICE := behead
RPI_BINS := behead behead-demo
RPI_BIN := $(RPI_BIN_DIR)/behead
RPI_DIR := rpi
RPI_CERTS := $(RPI_DIR)/overlay/etc/behead/certs
RPI_CERT_DAYS := 3650
RPI_SELF_SIGNED := $(RPI_CERTS)/90-self-signed
RPI_PHONE := $(RPI_CERTS)/10-phone
RPI_CACHED := $(RPI_CERTS)/95-
CERT_CACHE ?= $(HOME)/.cache/behead/certs
# desktop sessions. SESSION is the one shown on the headunit, in place of the
# demo; SESSIONS names more for `make rpi-image` to install beside it
SESSION ?=
SESSIONS ?=
SESSIONS_DIR := sessions
SESSION_NAMES := wayland kodi phosh waydroid
KIOSK_SESSIONS := wayland kodi waydroid
selected_sessions = $(sort $(SESSION) $(SESSIONS))
unknown_sessions = $(filter-out $(SESSION_NAMES),$(selected_sessions))
check_sessions = $(if $(unknown_sessions),$(error unknown session $(unknown_sessions): one of $(SESSION_NAMES)))
session_depends = $(wildcard $(addprefix $(SESSIONS_DIR)/depends/,$(selected_sessions) \
	$(if $(filter $(KIOSK_SESSIONS),$(selected_sessions)),kiosk)))
RPI_SESSIONS_PATH := /usr/local/lib/behead/sessions
RPI_SESSIONS := $(RPI_DIR)/overlay$(RPI_SESSIONS_PATH)
RPI_SESSION_ENV := $(RPI_DIR)/overlay/etc/behead/session.env
RPI_DEPENDS := $(RPI_DIR)/depends
RPI_SESSION_DEPENDS = $(RPI_DEPENDS)/session-
RPI_APT_SOURCES := $(RPI_DIR)/overlay/etc/apt/sources.list.d
RPI_SESSION_SOURCES = $(RPI_APT_SOURCES)/session-
RPI_APT_PREFERENCES := $(RPI_DIR)/overlay/etc/apt/preferences.d
RPI_SESSION_PREFERENCES = $(RPI_APT_PREFERENCES)/session-
# kernel parameters the chosen sessions need, handed to raspi-provision
RPI_SESSION_CMDLINE := $(RPI_DIR)/session.cmdline
RPI_GEOCLUE := $(RPI_DIR)/overlay/etc/geoclue/conf.d/90-behead.conf
RPI_WAYDROID := $(RPI_DIR)/overlay/usr/share/waydroid-extra/images
RPI_WAYDROID_ARCH := arm64
WAYDROID_CACHE ?= $(HOME)/.cache/behead/waydroid
FETCH_WAYDROID ?= tools/fetch-waydroid.py
# apps android comes with on the pi, put into its system image: f-droid and osmand~
RPI_ANDROID_ABI := arm64-v8a
WAYDROID_APPS := org.fdroid.fdroid net.osmand.plus
APK_CACHE ?= $(HOME)/.cache/behead/apk/fdroid
FETCH_FDROID ?= tools/fetch-fdroid.py
PREINSTALL_APKS ?= tools/preinstall-apks.py
RPI_TEST_DIR := tests/rpi
PYUSB := pyusb==1.3.1
RASPI_PROVISION ?= cargo run --quiet --release --manifest-path $(abspath ../raspi-provision/Cargo.toml) --

.PHONY: build build-rpi test test-e2e test-e2e-rpi vm test-vm demo rpi-certs rpi-sessions rpi-image rpi-flash rpi-deploy \
	test-rpi demo-rpi precommit clean

build:
	cargo build

build-rpi:
	rustup target add $(RPI_TARGET)
	CC_aarch64_unknown_linux_musl=clang CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER=rust-lld \
		cargo build --release --target $(RPI_TARGET) $(RPI_BINS:%=-p %)

test:
	cargo test

# HEADED=1 (or a port) shows what the scripted headunit receives, live in a
# browser, at a url the tests print. TESTS picks modules, classes or tests to
# run. OPEN opens the viewer, or the demo's screen, once it is up; OPEN= to not.
HEADED ?=
TESTS ?=
OPEN ?= 1
DEFAULT_VIEWER_PORT := 8090
VIEWER_PORT = $(if $(filter 1,$(HEADED)),$(DEFAULT_VIEWER_PORT),$(HEADED))
LOCAL_VIEWER = http://127.0.0.1:$(VIEWER_PORT)/
VM_ADDRESS = $(shell incus list $(VM) -c 4 -f csv | sed 's/ .*//')
OPEN_WAIT := 300
# in the background: wait until a url answers, then run a command on it
open_when_up = $(if $(OPEN),(timeout $(OPEN_WAIT) sh -c 'until curl -s -o /dev/null "$$0"; do sleep 1; done' \
	"$(1)" && $(2)) > /dev/null 2>&1 &)

test-e2e: build
	$(if $(HEADED),$(call open_when_up,$(LOCAL_VIEWER),xdg-open $(LOCAL_VIEWER)))
	cd $(E2E_DIR) && BEHEAD_HEADED=$(HEADED) python3 -m unittest -v $(TESTS)

# the same tests against the rpi binary, run through qemu user emulation
test-e2e-rpi: build-rpi
	cd $(E2E_DIR) && BEHEAD_HEADED=$(HEADED) BEHEAD_BIN=$(abspath $(RPI_BIN)) python3 -m unittest -v $(TESTS)

# incus reads instance config from stdin when it is not a terminal
vm:
	incant up < /dev/null

# usb gadget tests, run as root inside the vm against the host-built binary.
# includes the rpi service rehearsal: unit file, live encoder, g_ether handover
test-vm: build
	$(if $(HEADED),$(call open_when_up,http://$(VM_ADDRESS):$(VIEWER_PORT)/,xdg-open http://$(VM_ADDRESS):$(VIEWER_PORT)/))
	incus exec $(VM) --cwd $(VM_TEST_DIR) --env BEHEAD_HEADED=$(HEADED) -- python3 -m unittest -v $(TESTS) < /dev/null

# openauto as the headunit, on a screen you can drive with a vnc viewer or a
# browser. SESSION=wayland, kodi, phosh or waydroid shows a desktop session
# instead of the demo. opens gvncviewer when installed, otherwise the browser
DEMO_VNC = $(VM_ADDRESS):0
DEMO_WEB = http://$(VM_ADDRESS):6080/vnc.html?autoconnect=1&resize=scale
demo: build
	$(check_sessions)
	$(call open_when_up,$(DEMO_WEB),$(if $(shell command -v gvncviewer),gvncviewer $(DEMO_VNC),xdg-open "$(DEMO_WEB)"))
	incus exec $(VM) -t --env VIDEO_CMD="$(VIDEO_CMD)" --env SESSION="$(SESSION)" -- bash $(VM_TEST_DIR)/demo.sh

$(RPI_DIR)/customize.env:
	cp $(RPI_DIR)/customize.env.example $@

# the certificates the pi offers the car, as NAME.crt and NAME.key pairs in
# rpi/overlay/etc/behead/certs, tried in name order until the car accepts
# one. a self-signed pair, which openauto and the 2021 sprinter accept but a
# stricter car may not (R8), is always there. PHONE_CERT and PHONE_KEY add a
# pair ahead of it, and every pair ../dexter left in CERT_CACHE
# follows it as a backup, kept in step with the cache on each build. a
# certificate there without a key, as the root is, has nothing to present.
# drop in any others by hand.
rpi-certs:
	mkdir -p $(RPI_CERTS)
	test -f $(RPI_SELF_SIGNED).key || openssl req -x509 -newkey rsa:2048 -nodes -days $(RPI_CERT_DAYS) \
		-subj /O=behead-dev -keyout $(RPI_SELF_SIGNED).key -out $(RPI_SELF_SIGNED).crt
ifneq ($(PHONE_KEY),)
	install -m 644 $(PHONE_CERT) $(RPI_PHONE).crt
	install -m 600 $(PHONE_KEY) $(RPI_PHONE).key
endif
	rm -f $(RPI_CACHED)*
	for key in $(CERT_CACHE)/*.key; do cert=$${key%.key}.crt; test -f $$key && test -f $$cert || continue; \
		name=$$(basename $$key .key); \
		install -m 644 $$cert $(RPI_CACHED)$$name.crt; install -m 600 $$key $(RPI_CACHED)$$name.key; \
	done
	for cert in $(RPI_CERTS)/*.crt; do key=$${cert%.crt}.key; \
		test "$$(openssl x509 -in $$cert -noout -pubkey)" = "$$(openssl pkey -in $$key -pubout)" \
			|| { echo "$$key does not belong to $$cert" >&2; exit 1; }; \
		echo "$$cert:"; openssl x509 -in $$cert -noout -subject -issuer -enddate; \
		openssl x509 -in $$cert -noout -checkend 0 || echo "warning: $$cert has expired" >&2; \
	done

# the desktop sessions in the pi image. every session script goes in, being
# small, but only the SESSION and SESSIONS chosen get their packages, with
# the apt source for them where debian stable has none and the kernel
# parameters they need, and waydroid the android images it starts from,
# with its apps put into the system image. SESSION becomes the video
# command, in a session.env the service reads after behead.env; without one
# the image shows the demo. whatever an earlier build chose is removed.
rpi-sessions:
	$(check_sessions)
	rm -rf $(RPI_SESSIONS) $(RPI_SESSION_ENV) $(RPI_SESSION_DEPENDS)* $(RPI_SESSION_SOURCES)* \
		$(RPI_SESSION_PREFERENCES)* $(RPI_WAYDROID)
	install -D -t $(RPI_SESSIONS) $(SESSIONS_DIR)/*.sh $(SESSIONS_DIR)/*.py
	install -D -m 644 $(SESSIONS_DIR)/geoclue.conf $(RPI_GEOCLUE)
	$(foreach list,$(session_depends),install -D -m 644 $(list) $(RPI_SESSION_DEPENDS)$(notdir $(list));)
	$(foreach list,$(wildcard $(session_depends:%=%.sources)),install -D -m 644 $(list) $(RPI_SESSION_SOURCES)$(notdir $(list));)
	$(foreach pins,$(wildcard $(session_depends:%=%.preferences)),install -D -m 644 $(pins) $(RPI_SESSION_PREFERENCES)$(basename $(notdir $(pins)));)
	cat /dev/null $(wildcard $(session_depends:%=%.cmdline)) > $(RPI_SESSION_CMDLINE)
ifneq ($(SESSION),)
	echo 'VIDEO_CMD="$(RPI_SESSIONS_PATH)/$(SESSION).sh {width} {height} {fps}"' > $(RPI_SESSION_ENV)
	cat /dev/null $(wildcard $(SESSIONS_DIR)/depends/$(SESSION).env) >> $(RPI_SESSION_ENV)
endif
ifneq ($(filter waydroid,$(SESSION) $(SESSIONS)),)
	$(FETCH_WAYDROID) --arch $(RPI_WAYDROID_ARCH) --out $(WAYDROID_CACHE)/$(RPI_WAYDROID_ARCH)
	mkdir -p $(RPI_WAYDROID)
	cp --reflink=auto $(WAYDROID_CACHE)/$(RPI_WAYDROID_ARCH)/*.img $(RPI_WAYDROID)
	set -e; apks=; for app in $(WAYDROID_APPS); do \
		apks="$$apks $$($(FETCH_FDROID) $$app --abi $(RPI_ANDROID_ABI) --out $(APK_CACHE) | sed -n 's/.*: verified //p')"; \
	done; $(PREINSTALL_APKS) $(RPI_WAYDROID)/system.img --abi $(RPI_ANDROID_ABI) $$apks
endif

# flashable sdcard image, built by raspi-provision and gzip-compressed, at
# rpi/images/<hostname>-<session>.img.gz, -demo without a SESSION. each
# session has its own, so building one leaves the others to flash later
RPI_HOSTNAME = $(shell sed -n 's/^CUSTOM_HOSTNAME=//p' $(RPI_DIR)/customize.env)
RPI_IMAGE = images/$(RPI_HOSTNAME)-$(or $(SESSION),demo).img.gz
rpi-image: build-rpi $(RPI_DIR)/customize.env rpi-certs rpi-sessions
	install -D -t $(RPI_DIR)/overlay/usr/local/bin $(RPI_BINS:%=$(RPI_BIN_DIR)/%)
	cd $(RPI_DIR) && CMDLINE_EXTRA="$$(cat $(abspath $(RPI_SESSION_CMDLINE)))" $(RASPI_PROVISION) image $(RPI_IMAGE)

# DESTRUCTIVE: write the image of SESSION, or the demo's, to the sdcard that
# MOUNT_DEVICE names in rpi/customize.env
rpi-flash:
	cd $(RPI_DIR) && $(RASPI_PROVISION) flash $(RPI_IMAGE)

# copy freshly built binaries to a running pi and restart the service. they
# land in the ram layer, so this leaves the card itself untouched: a reboot
# returns to the binaries in the image. same login as raspi-provision.
RPI_ADDRESS ?= $(shell sed -n 's/^ADDRESS=//p' $(RPI_DIR)/customize.env 2>/dev/null)
rpi-deploy: build-rpi
	scp $(RPI_BINS:%=$(RPI_BIN_DIR)/%) root@$(RPI_ADDRESS):/tmp/
	ssh root@$(RPI_ADDRESS) 'systemctl stop $(RPI_SERVICE) && cd /tmp && install -m 755 $(RPI_BINS) /usr/local/bin/ \
		&& systemctl start $(RPI_SERVICE) && systemctl is-active $(RPI_SERVICE)'

# laptop as headunit, against a flashed rpi4 connected by usb cable
test-rpi:
	$(if $(HEADED),$(call open_when_up,$(LOCAL_VIEWER),xdg-open $(LOCAL_VIEWER)))
	cd $(RPI_TEST_DIR) && BEHEAD_HEADED=$(HEADED) uv run --no-project --with $(PYUSB) python -m unittest -v $(TESTS)

# the same rpi4 with the browser as its headunit: the pi's video on a page,
# the mouse on the picture as a finger. ctrl-c stops it
demo-rpi: HEADED = 1
demo-rpi:
	$(call open_when_up,$(LOCAL_VIEWER),xdg-open $(LOCAL_VIEWER))
	cd $(RPI_TEST_DIR) && uv run --no-project --with $(PYUSB) python demo.py

precommit:
	cargo fmt --check
	cargo clippy --all-targets -- -D warnings
	$(MAKE) test test-e2e

clean:
	cargo clean
