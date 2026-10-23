# radar-frame

One Rust package, one binary. `radar-frame` serves the e-ink frame over HTTP,
builds the setup artifacts, renders radar and e-ink frames for inspection, and
writes the palette. There is no Python, gunicorn, GDAL, PROJ, or subprocess in
the server: the HTTP service, radar rasterizer, e-ink composition, and Archive
II parser are all linked into `target/release/radar-frame`.

The client (`client/`) is still the Python service for the Raspberry Pi; it
polls this server.

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
    lib.rs
    cli.rs                 clap structs and dispatch
    api.rs clock.rs config.rs constants.rs engine.rs error.rs
    logging.rs modes.rs paths.rs setup.rs state.rs
    sources/               mock nexrad nws online open_meteo osm
    nexrad/                archive2 bins elevations error geo
    render/                archive colors osm pngio raster render streets
    view/                  codes colors components error font forecast
                           icons image info palette views
  tests/
    api.rs render.rs view.rs
    fixtures/
      api/, nexrad/, frame/, view/
```

Runtime paths stay relative to the working directory: `config/config.ini`,
`data/`, and `radar-frame.log`. `config/config.ini`, `data/`, and the log are
gitignored. The only build directory is `target/`.

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

radar-frame render volume --file <volume.ar2> [--minimum 10] [--force-streets]
radar-frame render storm [--mock]
radar-frame render clear [--mock]
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

There is no GeoJSON command and no GeoJSON input: the only radar renderer is
the fused `render archive` (Archive II -> REF frame). `VEL`, `SW`, `ZDR`,
`PHI`, and `RHO` output is not produced.

| Before | After |
|---|---|
| `frame-render frame` | `radar-frame render archive` |
| `frame-render streets` | `radar-frame render streets` |
| `frame-view storm\|clear\|palette` | `radar-frame view storm\|clear\|palette` |
| `radar-frame render-volume` | `radar-frame render volume` |
| `radar-frame render-clear\|render-storm` | `radar-frame render clear\|storm` |
| `radar-frame serve\|setup` | unchanged |
| `frame-render radar` | removed |
| `nexrad-geojson` | removed |

No aliases: scripts using the old names must switch to this table or stop.

Examples:

```sh
# HTTP service (mock forecasts, no network)
radar-frame serve --mock

# one-time / per-bbox artifacts: data/, palette, street mask
radar-frame setup

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
  and the alert zones.

`[logging] level` (default `INFO`) sets the log level.

The example file ships placeholder bounds (`23.45` for both corners); the
service and renderers reject a zero-height bbox, so replace them before running
anything. Relative `[render] colors` paths resolve from the working directory.

The color table is embedded (`assets/gdal-colors.txt`). To override it:

```ini
[render]
colors = /opt/radar-frame/colors.txt
```

Omit the key to use the built-in table; deployment no longer copies
`gdal-colors.txt`.

## Run the service

```sh
radar-frame serve [--bind 0.0.0.0:8000] [--mock]
```

`--mock` uses built-in forecast data and skips Open-Meteo/NWS forecast calls.
Startup creates `data/` if missing, writes `data/palette.png`, and builds the
street mask when it is missing or stamped for another `[bbox]`. The mask is
downloaded from Overpass only then; a hand-written `data/streets.osm` is
adopted without a request. `setup` performs the same steps without serving.

Logs go to `radar-frame.log`, rotated at 100 kB with four backups.

## Deployment

The installed layout is flat:

```text
/opt/radar-frame/
  bin/radar-frame
  config/config.ini
  data/
  radar-frame.log
```

```sh
cd server
cargo build --release
sudo useradd --system --user-group --home /opt/radar-frame --shell /usr/sbin/nologin radar-frame
sudo install -d -o radar-frame -g radar-frame /opt/radar-frame/bin /opt/radar-frame/config
sudo install -m 0755 target/release/radar-frame /opt/radar-frame/bin/radar-frame
sudo install -m 0644 config/config.example.ini /opt/radar-frame/config/config.ini
sudo install -m 0644 radar-frame.example.service /etc/systemd/system/radar-frame.service
sudo chown -R radar-frame:radar-frame /opt/radar-frame
sudo systemctl daemon-reload
sudo systemctl enable --now radar-frame
```

Edit `/opt/radar-frame/config/config.ini` before starting. `gdal-colors.txt` is
not installed; it is embedded in the binary.

Upgrade from the old nested `/opt/radar-frame/server/` layout:

```sh
sudo systemctl stop radar-frame
sudo mv /opt/radar-frame/server/config /opt/radar-frame/server/data /opt/radar-frame/
sudo rm -rf /opt/radar-frame/server
sudo install -d -o radar-frame -g radar-frame /opt/radar-frame/bin
sudo install -m 0755 target/release/radar-frame /opt/radar-frame/bin/radar-frame
sudo install -m 0644 radar-frame.example.service /etc/systemd/system/radar-frame.service
sudo chown -R radar-frame:radar-frame /opt/radar-frame
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
  Storm, 30 in Clear), then renders and returns the image;
- manual selection expires one day after it is chosen;
- auto selection re-reads the NWS radar station on each `/frame` request and
  switches to Clear when the radar is down, the VCP is unknown, or the VCP is
  clear-air (31/32/35).

Network use:

- Open-Meteo: one request per clear update. Current, hourly, and daily panels
  are disjoint parameters of the same endpoint, so they are fetched together.
- NWS: one station request per automatic `/frame` poll and one alerts request
  per update; no polling beyond the existing schedule.
- S3: an anonymous ListObjectsV2 query for the UTC day (yesterday as a
  fallback) and one GET for the selected volume.
- Overpass: only when `data/streets.osm` or the mask is missing or stale for
  `[bbox]`; a hand-written OSM file is adopted without a request.

Every outbound request uses the `User-Agent` from `[nws] user_agent`; set it to
a contact address, as the NWS API asks. Without the setting the generic
`radar-frame` value is used.

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
`c8a90300ea730dfd045a651431864a4fdf0c2272aedbf3852e7dd6cbbedd12ad`
(177,246 bytes).

The lower-level commands render the same pieces without the configured
`[bbox]`:

```sh
radar-frame render archive \
  --archive tests/fixtures/nexrad/ktlx-mini.ar2 -e 1 \
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

`render archive` accepts `-e N` or `-e N-M`; every selected elevation must be
present in the volume. Without `--colors` it uses the embedded table.

`view storm` reads alerts as JSON on stdin: an array of
`{event, level, status, effective, expires}` where `level` is
`Info|Watch|Warning|Emergency`. Only `actual` alerts whose interval contains
now are drawn; they are sorted by ascending severity and the first is shown.

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

- `fixtures/nexrad/ktlx-mini.ar2`: a trimmed real KTLX volume (2,870,734 bytes
  extracted to 18,972 bytes; the extraction rule is in
  `fixtures/nexrad/README.md`). It is the only volume fixture the fused
  renderer and the HTTP storm test need.
- `fixtures/frame/ktlx-mini`: the GDAL/Pillow golden frame
  (`frame.expected.png`), the GDAL street mask (`streets.png`), and the minimal
  OSM XML that reproduces it. Generated with GDAL 3.10.3, Pillow 11.1.0, and
  NumPy 2.2.4, bbox `-97.26,35.35,-97.21,35.39`, 800x444. Never regenerate
  either golden with the Rust renderer. The mask's `tRNS` level 1 and the frame
  background `(46, 11, 119)` come from GDAL's Byte nodata 255; the notes in
  `fixtures/frame/README.md` explain them. The historical regeneration recipe
  used the removed `nexrad-geojson` converter and belongs to the pre-cutover
  commit, so it is not runnable from this tree.
- `fixtures/view`: the Python views at commit `202792a`, Pillow 11.1.0,
  CairoSVG 2.9.1. The info bar is identical; the clear frame is 99.31%
  identical (icon edges); the storm dither differs but 8x8 block means stay
  within 3.4/255.
- `fixtures/api`: Flask goldens at commit `aca9092` with a fixed clock
  `2026-10-21T15:30:00Z` and `America/Chicago`. Regeneration needs a worktree of
  that commit; the storm case stubs the download and uses
  `fixtures/frame/ktlx-mini/frame.expected.png`.

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
