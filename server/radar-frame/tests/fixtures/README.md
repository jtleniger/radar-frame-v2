# Fixtures

| File | Meaning |
|---|---|
| `nws-alerts.json` | Raw `/alerts/active` response with one alert per level (Emergency, Warning, Watch, Info), active at the fixture time |
| `golden/index.html` | `GET /` with fresh state, fixed clock |
| `golden/settings-auto.html` | `POST /settings` with `mode-select=Auto` |
| `golden/settings-manual-storm.html` | `POST /settings` with `mode-select=Manual&mode=Storm` |
| `golden/settings-manual-no-mode.html` | `POST /settings` with `mode-select=Manual` only, fresh state |
| `golden/index-manual-storm.html` | `GET /` after the manual Storm selection |
| `golden/debug-frame.png` | `GET /frame?debug=true`; equal to `data/frame.png` at capture time |
| `golden/clear-frame.png` | `GET /frame` in auto mode with clear-air VCP 31 and mock forecasts |
| `golden/storm-frame.png` | `GET /frame` in auto mode with VCP 212 and the alerts above |
| `golden/responses.json` | Status codes for each request, including the 304s |

Fixed time `2026-10-21T15:30:00Z`, time zone `America/Chicago` (so 10:30 local
and a manual expiry at `2026-10-22 10:30AM`).

## Regenerating

These goldens are the frozen output of the Flask service. Its capture script
was removed together with the Python server in unit 3c, so regeneration starts
from the last pre-cutover commit (`aca9092`):

```sh
git worktree add /tmp/radar-frame-flask aca9092
cargo build --release --manifest-path /tmp/radar-frame-flask/server/frame-view/Cargo.toml
python3 /tmp/radar-frame-flask/server/radar-frame/tests/fixtures/capture.py
```

The script mirrors the deployment layout in a temporary directory, freezes
`server.api.datetime`, appends `--now` to every `frame-view` invocation, and
stubs the NWS `requests` module with `nws-alerts.json`. The storm case also
stubs the NEXRAD download and pre-places
`frame-render/tests/fixtures/ktlx-mini/frame.expected.png` as
`data/nexrad-latest.png`, so no network access is needed and the radar image is
the same one the Rust engine produces for the fixture volume. Copy `golden/`
back into this directory and run `cargo test` in `server/radar-frame`.

`tests/api.rs` sends the identical requests through the axum router with a
`FixedClock` and compares the bodies byte for byte (HTML) or pixel for pixel
(PNG).
