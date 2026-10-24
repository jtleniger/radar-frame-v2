# radar-frame client

Rust service for the Raspberry Pi that drives a Waveshare 7.3-inch (F)
seven-colour e-ink panel from the `radar-frame` HTTP service.

The client replaces the former Python/`waveshare_epd` program. It uses the
`epd-waveshare` crate with its `epd7in3f` driver and `rppal` for SPI/GPIO.

## Protocol behaviour

Each cycle:

1. `GET /healthz` — a non-`200` answer skips the frame request and backs off.
2. `GET /frame` with `If-None-Match` set to the ETag of the frame currently on
   the panel.
   - `200`: decode, validate the `800x480` PNG, and refresh the panel. The ETag
     is remembered only after a successful refresh.
   - `304`: nothing is due; the panel keeps its image.
   - `503`: the server has no usable frame yet; `Retry-After` is honored.
   - `500` and other failures: exponential backoff (60 s doubling to 30 min).
3. On a `304` with no frame on the panel (for example after a client restart),
   `GET /frame?debug=true` fetches the published frame immediately instead of
   waiting for the next scheduled cycle.
4. `GET /readyz` after a `503` reports the server's readiness reason.
5. `GET /status` after every attempt is logged (mode, readiness, frame age,
   failures, radar state) and its `next_attempt` is used to schedule the next
   poll. Sleeps are clamped to 15 s–5 min for a successful cycle and 30 min for
   failures.

The client keeps the last image on the panel through server restarts and
upstream outages; it never clears the display because a request failed.

## Configuration

`config/config.ini`, in the same INI shape as the Python client:

```ini
[logging]
file = radar-frame.log
level = INFO

[server]
base_url = http://192.168.1.10:8000/
poll_seconds = 120
connect_timeout_seconds = 5
request_timeout_seconds = 30

[epaper]
use_noop = false
```

`base_url` may include a path prefix; a missing trailing slash is added. Use an
IP address, a normal DNS name, or an Avahi `.local` name. The latter needs
`libnss-mdns` and `avahi-daemon` on the Pi; see the build section.
Relative `file` paths resolve from the working directory. Logs rotate at 100 kB
with four backups, or go to stderr if the file cannot be opened.

Flags:

- `--config PATH` — configuration file (default `config/config.ini`).
- `--noop` — decode and log frames without opening SPI/GPIO.
- `--once` — run one poll and exit; useful for smoke tests.

## Build

The display Pi runs Debian 11 with glibc 2.31. Build a dynamically linked
binary against that exact glibc — not the build host's newer one. Dynamic
linking is required for Avahi: glibc loads `libnss_mdns.so` through NSS when
resolving `.local` names.

Cross-compile with `cargo-zigbuild`; the `.2.31` target suffix pins the glibc
symbol version:

```sh
cargo install cargo-zigbuild
# Install Zig first: https://ziglang.org/download/ (0.13 or newer).

# ARMv6, 32-bit (Pi 1 Model A/A+/B/B+, Pi Zero):
rustup target add arm-unknown-linux-gnueabihf
cargo zigbuild --release --manifest-path client/Cargo.toml \
  --target arm-unknown-linux-gnueabihf.2.31
```

Other Raspberry Pi targets, with the same `.2.31` suffix:

- `armv7l`, 32-bit: `armv7-unknown-linux-gnueabihf.2.31`
- `aarch64`, 64-bit: `aarch64-unknown-linux-gnu.2.31`

The artifact lands in
`client/target/arm-unknown-linux-gnueabihf/release/radar-frame-client` for the
ARMv6 build. Verify it:

```sh
file client/target/arm-unknown-linux-gnueabihf/release/radar-frame-client
# ... dynamically linked, interpreter /lib/ld-linux-armhf.so.3 ...

readelf -A client/target/arm-unknown-linux-gnueabihf/release/radar-frame-client \
  | grep Tag_CPU_arch
# Tag_CPU_arch: v6

readelf -V client/target/arm-unknown-linux-gnueabihf/release/radar-frame-client \
  | grep -o 'GLIBC_[0-9.]*' | sort -V | tail
# highest line must be GLIBC_2.31 or lower
```

Do not use the unversioned `arm-unknown-linux-gnueabihf` target: it links
against the build host's glibc. The architecture check matters in the other
direction too: an ARMv7 binary traps on an ARMv6 Pi, so confirm
`Tag_CPU_arch: v6` when deploying to a Pi 1 or Pi Zero.

### Avahi / `.local` hostnames

On the Pi, install and check NSS mDNS:

```sh
sudo apt install libnss-mdns avahi-daemon
sudo systemctl enable --now avahi-daemon
grep '^hosts:' /etc/nsswitch.conf
# hosts: files mdns4_minimal [NOTFOUND=return] dns
getent hosts radar-frame.local
```

Then set `base_url = http://radar-frame.local:8000/`. The binary still has no
OpenSSL dependency; TLS is rustls.

### Native build

A normal host build is fine for `--noop` smoke tests, but it links against the
host glibc and must not be deployed to Bullseye:

```sh
cargo build --manifest-path client/Cargo.toml --release
```

## Install on the Pi

The HAT uses SPI0/CE0 and GPIO 17 (reset), 24 (busy), 25 (DC), and 18 (power),
which are the wiring the Python driver used.

```sh
# On the Pi, once:
sudo raspi-config nonint do_spi 0
sudo usermod -aG spi,gpio pi
# Log out and back in so the group change applies.

# From the build host:
BINARY=client/target/arm-unknown-linux-gnueabihf/release/radar-frame-client

ssh pi@radar-frame-client 'mkdir -p ~/radar-frame-client/config'
scp "$BINARY" pi@radar-frame-client:/tmp/radar-frame-client
scp client/config/config.example.ini \
  pi@radar-frame-client:/home/pi/radar-frame-client/config/config.ini
scp client/radar-frame-client.service \
  pi@radar-frame-client:/tmp/radar-frame-client.service

# On the Pi:
sudo install -m 0755 /tmp/radar-frame-client \
  /home/pi/radar-frame-client/radar-frame-client
$EDITOR /home/pi/radar-frame-client/config/config.ini

sudo install -m 0644 /tmp/radar-frame-client.service \
  /etc/systemd/system/radar-frame-client.service
sudo systemctl daemon-reload
sudo systemctl enable --now radar-frame-client
journalctl -u radar-frame-client -f
```

The service expects the binary, `config/config.ini`, and `radar-frame.log` in
`/home/pi/radar-frame-client`, matching the old Python layout. The old
`venv/` and `main.py` can be removed.

Test without a panel or before enabling the service:

```sh
./radar-frame-client --config config/config.ini --noop --once
```

## Troubleshooting

- `Permission denied` opening `/dev/spidev0.0` or `/dev/gpiochip0`: the service
  user is not in the `spi`/`gpio` groups, or SPI is disabled.
- Repeated `503`/`500`: the server has not published a frame. The client honors
  `Retry-After` and will load the published frame with `?debug=true` as soon as
  one exists.
- `500` responses: the server is failing internally; the client backs off and
  keeps the current image. Check `radar-frame.log` on the server.
- HTTPS works through rustls with bundled web roots; no OpenSSL or system CA
  package is required.
