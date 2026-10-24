# radar-frame

One Rust package, one binary. `radar-frame` serves the e-ink frame over HTTP,
builds the setup artifacts, renders radar and e-ink frames for inspection, and
writes the palette. There is no Python, gunicorn, GDAL, PROJ, or subprocess in
the server: the HTTP service, radar rasterizer, e-ink composition, and Archive
II parser are all linked into `target/release/radar-frame`.

The client (`client/`) is a Rust service for the Raspberry Pi. It polls this
server, uses the same conditional `/frame` contract, and drives the Waveshare
7.3-inch panel through `epd-waveshare`. See `client/README.md` for the glibc
2.31 build used on Debian 11 and the Avahi `.local` setup.

## Layout

```text
server/
  Cargo.toml               the only package manifest; one bin target
  Cargo.lock
  LICENSE.md
  README.md
  radar-frame.example.service
  config/config.example.ini
  assets/
    gdal-colors.txt        embedded color table
    spleen-16x32.otf
    spleen-32x64.otf
    icons/*.svg
  templates/               askama templates
  static/style.css
  src/
    main.rs                thin: radar_frame::cli::main()
    lib.rs                 module tree
    error.rs               typed errors and HTTP mapping
    cli/                   clap structs, dispatch, demo/serve/render/view/setup
    config/                config settings paths constants
    http/                  router handlers pages status updater
    service/               startup clock renderer frame modes setup
    state/                 schedule, mode selection, persistence
    telemetry/             logging metrics
    util/                  backoff fsutil
    sources/               mock nexrad nws online open_meteo osm
    nexrad/                archive2 bins elevations error geo
    render/                archive colors osm pngio raster render streets
    view/                  codes colors components error font forecast
                           icons image info palette views
  tests/
    api.rs cli.rs modes.rs render.rs view.rs
    fixtures/
      api/, nexrad/, frame/, view/
```

Runtime paths stay relative to the working directory: `config/config.ini`,
`data/`, `photos/`, and `radar-frame.log`. `config/config.ini`, `data/`,
`photos/`, and the log are gitignored. The only build directory is `target/`.

## Build

Requires Rust 1.90 or newer.

```sh
cd server
cargo build --release
```

The binary is `target/release/radar-frame`.

## Commands

```text
radar-frame serve [--bind 0.0.0.0:8000] [--mock]
radar-frame setup [--force-streets]

radar-frame demo clear [--mock]
radar-frame demo radar [--mock]
radar-frame demo warnings [--mock]

radar-frame render volume --file <volume.ar2> [--minimum 10] [--force-streets]
radar-frame render archive --archive <volume.ar2> [-e 1-3] [--minimum <n>] [--maximum <n>]
    --min-lon <f> --min-lat <f> --max-lon <f> --max-lat <f>
    --width <n> --height <n> [--colors <table>] --streets <mask.png> --output <png>
radar-frame render streets --input <osm.xml> --min-lon ... --min-lat ...
    --max-lon ... --max-lat ... --width <n> --height <n> --output <png>

radar-frame view storm --radar <png> --palette <png> --output <png>
    --timezone <IANA> [--now <rfc3339>]              (alerts JSON on stdin)
radar-frame view clear --palette <png> --output <png>
    --timezone <IANA> [--now <rfc3339>]              (forecast JSON on stdin)
radar-frame view palette --output <png>
```

`demo` exercises one data path end to end and prints what it found plus the
frame it wrote. `clear` fetches the forecast and alerts and renders the
clear-sky frame; `radar` downloads the latest NEXRAD volume and renders the
storm frame; `warnings` fetches the alerts and prints them without rendering.
`--mock` swaps in the built-in forecast, three active sample alerts, and an
embedded KTLX volume, so `demo clear --mock`, `demo radar --mock`, and
`demo warnings --mock` run without network access. The mocked radar cycle
still parses, rasterizes, and composes the frame from real Archive II bytes.

There is no GeoJSON command and no GeoJSON input: the only radar renderer is
the fused `render archive` (Archive II -> REF frame). `VEL`, `SW`, `ZDR`,
`PHI`, and `RHO` output is not produced.

## Endpoints

| Path | Purpose |
|---|---|
| `GET /` | Home page: latest frame, readiness, schedule/radar/service status from the same snapshot as `/status`, and the mode form. |
| `GET /frame` | Current e-ink frame; `304` until the next cycle is due when the caller's `If-None-Match` matches the published frame. A stale validator gets the published frame immediately so a client that missed a tick can catch up. A failed cycle keeps serving the last good frame and backs off. |
| `GET /frame?debug=true` | Last published frame, bypassing the schedule. |
| `POST /settings` | Mode-selection form target. |
| `GET /healthz` | Liveness; always `200` while the process is running. |
| `GET /readyz` | Readiness: a frame no older than two mode intervals and a writable `data/`. |
| `GET /status` | JSON snapshot: mode, frame age, readiness, cycle/upstream counters, last event ages, and the retry schedule (`failures`, `last_attempt`, `next_attempt`, `radar_status`). |
| `GET /static/style.css` | Stylesheet. |

`/status` exposes `event=cycle` and `event=upstream` log fields as counters and
ages, so an external probe does not have to parse the log.

### Update scheduling

One background updater owns the schedule, caches, and published frame. It
performs every status probe and render cycle itself and publishes an immutable
snapshot that the handlers read, so no request holds a lock across network I/O
or rendering.

- A `GET /frame` poll wakes the updater and waits for the result only when work
  is due; concurrent polls queue behind that one updater, so a burst runs one
  cycle and the rest answer with the published frame (or `304` when their
  validator already matches it).
- A 60-second tick lets the schedule, radar-status TTL, and retry backoff
  advance even while no client is polling.
- `/`, `/readyz`, and `/status` answer from the snapshot alone. `/settings` is
  applied by the updater (serialized with any cycle in flight) so the returned
  page reflects the new state.

### Photo mode

Photo is manual-only: the radar status never selects it, so choose it under
Manual on the home page. Each cycle shows the next `.png` in `photos/` (relative
to the working directory), ordered by file name, for 10 minutes, and wraps
after the last one. Files are expected to be cropped to 800x480 and are dithered
to the seven-colour e-ink palette with Floyd–Steinberg; a differently sized or
undecodable file is skipped with a warning and the next candidate is tried. If
`photos/` is missing, empty, or contains no usable file, the cycle fails and the
last good frame stays up. The last shown file name is persisted, so a restart
continues the rotation instead of repeating the first image. Manual selection,
including Photo, expires after one day and returns to radar-driven Auto.

| Before | After |
|---|---|
| `frame-render frame` | `radar-frame render archive` |
| `frame-render streets` | `radar-frame render streets` |
| `frame-view storm\|clear\|palette` | `radar-frame view storm\|clear\|palette` |
| `radar-frame render-volume` | `radar-frame render volume` |
| `radar-frame render-clear\|render-storm` | `radar-frame demo clear\|radar` |
| `radar-frame serve\|setup` | unchanged |
| `frame-render radar` | removed |
| `nexrad-geojson` | removed |

No aliases: scripts using the old names must switch to this table or stop.

Examples:

```sh
# HTTP service (mock forecast and alerts, no network)
radar-frame serve --mock

# one-time / per-bbox artifacts: data/, palette, street mask
radar-frame setup

# offline diagnostics: sample warnings, then a rendered clear frame
radar-frame demo warnings --mock
radar-frame demo clear --mock

# live diagnostics: alerts, latest volume and storm frame
radar-frame demo warnings
radar-frame demo radar

# local inspection from an Archive II volume
radar-frame render volume --file data/nexrad-latest.ar2
```

## Configuration

Copy `config/config.example.ini` to `config/config.ini` and edit it. Required:

- `[radar] nexrad_id` — the S3 station, e.g. `KTLX`;
- `[bbox] min_lat`, `max_lat`, `min_lon`, `max_lon` — the frame and street-mask
  extent;
- `[forecast] lat`, `lon`, `timezone` — the forecast point and the IANA zone
  used by the info bar;
- `[nws] user_agent`, `zone`, `fire_zone` — the contact address NWS asks for,
  and the alert zones;
- `[osm] user_agent` — the contact address sent with Overpass street-data
  requests.

`[logging] level` (default `INFO`) sets the log level.

The example file ships placeholder bounds (`23.45` for both corners); the
service and renderers reject a zero-height bbox, so replace them before running
anything. Relative `[render] colors` paths resolve from the working directory.

The color table is embedded (`assets/gdal-colors.txt`). To override it:

```ini
[render]
colors = colors.txt
```

Omit the key to use the built-in table; deployment no longer copies
`gdal-colors.txt`.

## Run the service

```sh
radar-frame serve [--bind 0.0.0.0:8000] [--mock]
```

`--mock` uses the built-in forecast and sample alerts and skips the
Open-Meteo/NWS calls.
Startup creates `data/` if missing and writes `data/palette.png`. The street
mask is built in the background when it is missing or stamped for another
`[bbox]`; Overpass failures are retried with exponential backoff, and frames
are served without the overlay until it is ready. The mask is downloaded from
Overpass only then; a hand-written `data/streets.osm` is adopted without a
request once it parses. Downloaded OSM bodies and rendered masks are validated
before replacing the previous files, and `data/streets.osm.hash` records the
body fingerprint. `setup` performs the same steps without serving and fails if
the mask cannot be built.

Logs go to `radar-frame.log`, rotated at 100 kB with four backups. If the file
cannot be opened, the service logs to stderr (the journal) instead; a failed
rotation is reported on stderr and appending continues until the next attempt.

## Deployment

The installed layout is flat under the service user's home:

```text
~/radar-frame/
  bin/radar-frame
  config/config.ini
  data/
  photos/
  radar-frame.log
```

The example unit uses `%h`, which systemd expands to the `radar-frame` user's
home directory, for `WorkingDirectory=`, `ExecStart=`, and `ReadWritePaths=`.
The commands below therefore install into `/home/radar-frame/radar-frame`. To
run as a different user, change `User=`/`Group=` in the unit; `%h` follows that
user's home.

```sh
cd server
cargo build --release
sudo useradd --system --user-group --create-home --home-dir /home/radar-frame --shell /usr/sbin/nologin radar-frame
sudo install -d -o radar-frame -g radar-frame /home/radar-frame/radar-frame/bin /home/radar-frame/radar-frame/config
sudo install -m 0755 target/release/radar-frame /home/radar-frame/radar-frame/bin/radar-frame
sudo install -m 0644 config/config.example.ini /home/radar-frame/radar-frame/config/config.ini
sudo install -m 0644 radar-frame.example.service /etc/systemd/system/radar-frame.service
sudo chown -R radar-frame:radar-frame /home/radar-frame/radar-frame
sudo systemctl daemon-reload
sudo systemctl enable --now radar-frame
```

Edit `/home/radar-frame/radar-frame/config/config.ini` before starting.
`gdal-colors.txt` is not installed; it is embedded in the binary.

Upgrade from the old nested `/opt/radar-frame/server/` layout:

```sh
sudo systemctl stop radar-frame
sudo usermod --home /home/radar-frame radar-frame
sudo install -d -o radar-frame -g radar-frame /home/radar-frame/radar-frame
sudo mv /opt/radar-frame/server/config /opt/radar-frame/server/data /home/radar-frame/radar-frame/
sudo mv /opt/radar-frame/radar-frame.log /home/radar-frame/radar-frame/ 2>/dev/null || true
sudo mv /opt/radar-frame/radar-frame.log.* /home/radar-frame/radar-frame/ 2>/dev/null || true
sudo rm -rf /opt/radar-frame/server
sudo install -d -o radar-frame -g radar-frame /home/radar-frame/radar-frame/bin
sudo install -m 0755 target/release/radar-frame /home/radar-frame/radar-frame/bin/radar-frame
sudo install -m 0644 radar-frame.example.service /etc/systemd/system/radar-frame.service
sudo chown -R radar-frame:radar-frame /home/radar-frame/radar-frame
sudo systemctl daemon-reload
sudo systemctl start radar-frame
```

`gdal-colors.txt` is no longer copied; a leftover file is ignored.

## HTTP contract and network use

`/` and `/settings` render the same templates and fields as before;
`/settings` returns the settings partial for htmx. `/frame` keeps the polling
contract:

- `GET /frame?debug=true` returns `data/frame.png`;
- `GET /frame` returns 304 until the mode's interval elapses (10 minutes in
  Storm and Photo, 30 in Clear), then renders and returns the image;
- `GET /frame` with an `If-None-Match` that differs from the published frame
  returns that frame immediately, even between cycles, so a client that missed
  a published update (for example because the ticker ran the cycle just before
  its poll) catches up instead of waiting for the next interval;
- manual selection expires one day after it is chosen;
- auto selection re-reads the NWS radar station on each `/frame` request and
  switches to Clear when the radar is down, the VCP is unknown, or the VCP is
  clear-air (31/32/35). A switch needs three consecutive agreeing status
  samples and a five-minute minimum dwell, so an alternating
  Operate/Standby station cannot force a render on every poll. The first
  sample after startup establishes the mode immediately.

The mode selection, schedule, radar volume key/mtime, and last photo name are
persisted in `data/state.json` (written atomically). A restart resumes the same
schedule and manual selection, and reuses the volume already in `data/` instead
of downloading it again. A missing or unreadable state file starts fresh.

Network use:

- Open-Meteo: one request per clear update. Current, hourly, and daily panels
  are disjoint parameters of the same endpoint, so they are fetched together.
- NWS: one station request per automatic `/frame` poll and one alerts request
  per update; no polling beyond the existing schedule.
- S3: a paginated anonymous ListObjectsV2 query for the UTC day (following
  `NextContinuationToken`), with yesterday as the fallback when today has no
  renderable volume or only `_MDM` placeholders, and one GET for the selected
  volume.
- Overpass: only when `data/streets.osm` or the mask is missing or stale for
  `[bbox]`; a hand-written OSM file is adopted without a request.

Every outbound request uses a `User-Agent` that identifies a contact: NWS,
Open-Meteo, and S3 requests use `[nws] user_agent`, while Overpass requests use
`[osm] user_agent`. Set both to a contact address, as the NWS API and Overpass
ask for. Without the settings the generic `radar-frame` value is used, but
startup rejects it. Requests give up after a 5 s connect and 30 s total timeout,
one `/frame` render cycle is capped at 180 s, and SIGTERM/SIGINT drain in-flight
requests before the process exits.

## Rendering and inspecting output

The reference event is the 2013 Moore EF5 tornado, observed by KTLX at
`2013-05-20T20:59:14Z`, a few minutes after touchdown. The volume is
`2013/05/20/KTLX/KTLX20130520_205914_V06.gz` in the anonymous
`unidata-nexrad-level2` bucket. Use this bbox (the whole supercell; a bbox tight
on the tornado only shows the core at the frame edge):

```ini
[bbox]
min_lat = 35.1615
max_lat = 35.4785
min_lon = -97.85
max_lon = -97.15
```

```sh
cargo build --release
curl -o data/nexrad-latest.ar2 \
  "https://unidata-nexrad-level2.s3.amazonaws.com/2013/05/20/KTLX/KTLX20130520_205914_V06.gz"
target/release/radar-frame setup
target/release/radar-frame render volume --file data/nexrad-latest.ar2
```

`render volume` writes `data/nexrad-latest.png` (800x444 RGBA) and
`data/frame.png` (800x480, seven palette colours), and prints both paths.
Expected values for this volume and bbox: 146,670 covered pixels of 355,200;
22,414 pixels at 41 dBZ or more; the KTLX cone of silence is a
background-coloured 9x9 hole around `(654, 204)`. The radar frame SHA-256 is
`b30c160e6745fe2c4ee91d2b2bcfc6d45cf35700c876b99daf67a6c650af795a`
(170,246 bytes).

The lower-level commands render the same pieces without the configured
`[bbox]`:

```sh
radar-frame render archive \
  --archive assets/ktlx-mini.ar2 -e 1 \
  --min-lon -97.26 --min-lat 35.35 --max-lon -97.21 --max-lat 35.39 \
  --width 800 --height 444 \
  --streets tests/fixtures/frame/ktlx-mini/streets.png \
  --output /tmp/frame.png
radar-frame render streets \
  --input tests/fixtures/frame/ktlx-mini/streets.osm \
  --min-lon -97.26 --min-lat 35.35 --max-lon -97.21 --max-lat 35.39 \
  --width 800 --height 444 --output /tmp/streets.png
radar-frame view palette --output /tmp/palette.png
```

`render archive` accepts `-e N` or `-e N-M`; selected elevations missing from
the volume are skipped with a warning, and at least one must be present. The
projection origin is the first radial of the lowest available scan, so a
partial volume (for example, one without elevation 1) still renders. Without
`--colors` it uses the embedded table.

`view storm` reads alerts as JSON on stdin: an array of
`{event, level, status, effective, expires}` where `level` is
`Info|Watch|Warning|Emergency`. Only `actual` alerts whose interval contains
now are drawn; they are sorted by descending severity and the most severe is
shown (with a count of the others).

```sh
radar-frame view storm \
  --radar tests/fixtures/view/radar.png --palette /tmp/palette.png \
  --output /tmp/storm.png --timezone America/Chicago \
  --now 2025-06-01T12:34:56Z < tests/fixtures/view/alerts.json
```

`view clear` reads:

```json
{
  "current": {"temp_f": 101.0, "code": 57, "is_day": false},
  "hourly": [{"hour": "9pm", "temp_f": 24.0, "code": 3}],
  "daily": [{"day": "tue", "high_f": 38.8, "low_f": 123.2, "code": 1}],
  "alerts": []
}
```

Every other hourly entry is drawn, as before.

```sh
radar-frame view clear \
  --palette /tmp/palette.png --output /tmp/clear.png \
  --timezone America/Chicago --now 2025-06-01T12:34:56Z \
  < tests/fixtures/view/forecast.json
```

## Tests and fixtures

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
```

- `assets/ktlx-mini.ar2`: a trimmed real KTLX volume (2,870,734 bytes
  extracted to 18,972 bytes), embedded in the binary for `demo radar --mock`.
  It is the only volume fixture the fused renderer and the HTTP storm test
  need.
- `fixtures/frame/ktlx-mini`: the GDAL/Pillow golden frame
  (`frame.expected.png`), the GDAL street mask (`streets.png`), and the minimal
  OSM XML that reproduces it. Generated with GDAL 3.10.3, Pillow 11.1.0, and
  NumPy 2.2.4 plus the pre-3.10 `gdal_rasterize` nodata semantics described
  below, bbox `-97.26,35.35,-97.21,35.39`, 800x444. Never regenerate either
  golden with the Rust renderer. The mask's `tRNS` level 1 leaves alpha 254
  where the overlay blends; the frame background is the color table's
  transparent `0 0 0 0 0` entry flattened onto black. GDAL 3.10 changed
  `gdal_rasterize` to declare nodata 0 even without `-a_nodata`, which makes
  `gdal_calc.py` rewrite the uncovered pixels to 255 and paint them opaque
  `(46, 11, 119)`; regenerate with the raster nodata unset (`gdal_edit.py
  -unsetnodata`) to keep the pre-3.10 behavior. The historical regeneration
  recipe used the removed `nexrad-geojson` converter and belongs to the
  pre-cutover commit, so it is not runnable from this tree.
- `fixtures/view`: the Python views at commit `202792a`, Pillow 11.1.0,
  CairoSVG 2.9.1. The info bars were regenerated with the most-severe alert
  precedence fix (the Python views sorted ascending, so they always showed the
  least severe alert); the rest of the clear frame is 99.31% identical (icon
  edges) and the storm dither differs but 8x8 block means stay within 3.4/255.
- `fixtures/api`: Flask goldens at commit `aca9092` with a fixed clock
  `2026-10-21T15:30:00Z` and `America/Chicago`. Regeneration needs a worktree of
  that commit; the storm case stubs the download and uses
  `fixtures/frame/ktlx-mini/frame.expected.png`. Its info bar carries the same
  most-severe-alert correction, so it was regenerated from the fixed renderer.

The integration tests drive the real binary (render/view) and the axum router
(api). The frame and mask goldens must report 0 differing pixels; the view
goldens keep their documented tolerances.

## Assets and licenses

- Spleen fonts (`spleen-16x32.otf`, `spleen-32x64.otf`): copyright (c)
  2018-2023 Frederic Cambus, BSD 2-Clause.
- `assets/icons/*.svg`: Weather Icons by Erik Flowers, SIL Open Font License
  1.1. Only the icons named by `src/view/codes.rs` are kept.
- The project itself is MIT (`LICENSE.md`).

## Zones

NWS zone identifiers for `[nws] zone` and `fire_zone`:
<https://www.weather.gov/pimar/PubZone>.
