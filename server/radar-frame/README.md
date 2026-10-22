# radar-frame

The HTTP service, mode schedule, forecast sources, and setup artifacts. This
crate replaces `server/main.py`, `server/server/`, `server/modes/`,
`server/state/`, `server/sources/`, `server/config/`, and `server/setup/`
from the Flask service.

The image work is not duplicated here:

- `frame-render` rasterizes the radar frame (and the street mask);
- `frame-view` composes the e-ink frame and writes the palette.

Both are called as libraries, so the service starts no subprocesses.

## Commands

```sh
# HTTP service (replaces gunicorn + Flask)
cargo build --release --manifest-path radar-frame/Cargo.toml
./radar-frame/target/release/radar-frame serve [--bind 0.0.0.0:8000] [--mock]

# one-time / per-bbox artifacts
radar-frame setup [--force-streets]

# local inspection (mirrors the old main.py commands)
radar-frame render-volume --file data/nexrad-latest.ar2 [--minimum 10] [--force-streets]
radar-frame render-clear [--mock]
radar-frame render-storm [--mock]
```

All paths are relative to the working directory, as with the Python service:
`config/config.ini`, `gdal-colors.txt`, `data/`, and `radar-frame.log`.

## HTTP behavior

`/` and `/settings` render the same templates and fields as the Flask views;
`/settings` returns the settings partial for htmx. `/frame` keeps the polling
contract:

- `GET /frame?debug=true` returns `data/frame.png`;
- `GET /frame` returns 304 until the mode's interval elapses (10 minutes in
  Storm, 30 in Clear), then renders and returns the image;
- manual selection expires one day after it is chosen;
- auto selection re-reads the NWS radar station on each `/frame` request and
  switches to Clear when the radar is down, the VCP is unknown, or the VCP is
  clear-air (31/32/35).

## Network use

- Open-Meteo: one request per clear update. The current, hourly, and daily
  panels are disjoint parameters of the same endpoint, so they are fetched
  together instead of in three calls.
- NWS: one station request per automatic `/frame` poll and one alerts request
  per update; no polling beyond the existing schedule.
- S3: an anonymous ListObjectsV2 query for the UTC day (yesterday as a
  fallback) and one GET for the selected volume.
- Overpass: only when `data/streets.osm` or the mask is missing or stale for
  `[bbox]`; a hand-written OSM file is adopted without a request.

Every outbound request uses a `User-Agent` containing the contact address from
`[nws] user_agent` (default `radar-frame (jtleniger@gmail.com)`).

The radar station response is parsed on every `/frame` poll rather than cached,
which is the schedule the Flask service used.

## Tests

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
```

Unit tests cover the INI subset, the S3 list XML, Open-Meteo and NWS parsing,
the alert-level rules, the state schedule, and the street-mask stamps. The
integration tests in `tests/api.rs` drive the axum router with a fixed clock and
compare `/`, `/settings`, `/frame`, and the 304 path against responses captured
from Flask; see `tests/fixtures/README.md`.
