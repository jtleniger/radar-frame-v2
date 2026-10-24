# Operational Audit — radar-frame

Scope: server (`server/`), plus the polling client where it affects server availability.
Basis: release build `c26a44e`, release binary, code trace, and the runtime checks in [Verification](#verification).

Severity: **Critical** (core function wrong), **High** (outage/data loss/external abuse), **Medium** (degraded operations), **Low** (hygiene).
Likelihood: **Certain**, **High**, **Medium**, **Low**.
ID form: `<category>.<finding>.<action>`.

## Summary

| ID | Finding / action | Severity | Likelihood |
|---|---|---|---|
| 1.1 | Upstream failure blocks serving a valid frame | High | Certain |
| 1.1.1 | Render the base frame first; publish a degraded frame if alerts fail | — | — |
| 1.1.2 | Serve the last good frame on upstream errors; `503` only when none exists; keep the last mode if radar status fails | — | — |
| 1.2 | Unbounded download/decompression can OOM the process | High | Medium |
| 1.2.1 | Stream S3 GET to `.part` with a byte cap | — | — |
| 1.2.2 | Cap gzip/bzip2 expansion; reject oversized LDM records; use checked arithmetic | — | — |
| 1.3 | Non-atomic writes and last-good volume overwritten before validation | High | High |
| 1.3.1 | Write every artifact via temp file + `rename` | — | — |
| 1.3.2 | Validate a volume before replacing the last good volume/frame; keep last-good pair | — | — |
| 1.4 | Startup hard-depends on Overpass; restart loop; poisoned OSM stamp | High | High |
| 1.4.1 | Make streets optional at startup; background retry with backoff | — | — |
| 1.4.2 | Validate OSM XML/mask before writing the OSM file and stamps; use atomic stamps/hashes | — | — |
| 1.5 | One bad upstream record rejects the whole frame | Medium | Medium |
| 1.5.1 | Skip malformed alerts with a warning | — | — |
| 1.5.2 | Treat missing `rda`/station as down/Clear, not a hard error | — | — |
| 1.6 | Smaller error-handling gaps | Medium | High |
| 1.6.1 | Map `png_file` errors: `404` only for `NotFound`, else `500` | — | — |
| 1.6.2 | Fail startup or fall back to stderr if logging init fails; log rotation failures | — | — |
| 1.6.3 | Add a request timeout to the client | — | — |
| 2.1 | No health, readiness, or status endpoint | Medium | Certain |
| 2.1.1 | Add `/healthz`, `/readyz`, `/status` | — | — |
| 2.2 | No structured cycle/upstream events or metrics | Medium | Certain |
| 2.2.1 | Log one structured event per cycle and per upstream call | — | — |
| 2.2.2 | Expose counters/ages (at least in `/status`) | — | — |
| ~~2.3~~ | ~~Logs only to a 100 kB × 4 file; no journald; retention days~~ | Medium | Certain |
| ~~2.3.1~~ | ~~Log to stdout + journald; honour `RUST_LOG`; retain 7–14 days~~ | — | — |
| 2.4 | No startup summary or frame provenance | Medium | Certain |
| 2.4.1 | Log config/station/zones/timezone/endpoints/version at startup | — | — |
| 2.4.2 | Log mode, volume key, elevations, alerts, duration, frame bytes/hash per cycle | — | — |
| 3.1 | NWS radar status fetched on every `/frame` poll | High | Certain |
| 3.1.1 | TTL-cache status (60 s) with single-flight until the scheduler lands | — | — |
| 3.2 | No backoff; `last_updated` never advances on failure | High | Certain |
| 3.2.1 | Add `last_attempt`/`next_attempt`, exponential backoff + jitter, capped at the interval | — | — |
| 3.2.2 | Keep last-known-good per source; never retry every poll forever | — | — |
| 3.3 | Mode flapping forces repeated renders and fetches | Medium | Medium |
| 3.3.1 | Add hysteresis/minimum dwell before switching modes | — | — |
| 3.4 | Restarts refetch state and the latest volume | Medium | High |
| 3.4.1 | Persist mode selection, `last_updated`, and radar key/mtime | — | — |
| ~~3.5~~ | ~~Unauthenticated `0.0.0.0` exposure enables upstream amplification~~ | Medium | Medium |
| ~~3.5.1~~ | ~~Bind localhost by default; add auth/rate limiting for `/frame` and `/settings`~~ | — | — |
| 4.1 | S3 listing ignores pagination and can pin to a stale volume | High | Medium |
| 4.1.1 | Loop `continuation-token` until `IsTruncated=false`; handle MDM-only days | — | — |
| 4.2 | Elevation 1 is a hard requirement even for valid partial volumes | Medium | Medium |
| 4.2.1 | Project from the first available scan; composite whichever elevations exist | — | — |
| 4.3 | Per-gate allocations and string-based rounding dominate the render loop | Medium | Certain |
| 4.3.1 | Replace `Vec<Bin>`/per-polygon `Vec`s with reusable buffers and a closed 4-corner ring | — | — |
| 4.3.2 | Replace `format!`/`parse` rounding with an exact fixed-point helper | — | — |
| 4.3.3 | Burn max directly into the composite; drop the per-elevation grid + merge pass | — | — |
| 4.3.4 | Borrow moment data from the decompressed buffer instead of `to_vec()` per moment | — | — |
| 5.1 | Clear-mode forecast is cached for the process lifetime | Critical | Certain |
| 5.1.1 | Remove the `OnceCell`; fetch once per Clear cycle or use a TTL cache | — | — |
| 5.1.2 | Add a two-cycle regression test proving the forecast refreshes | — | — |
| 5.2 | One provider trait mixes forecast, alerts, and radar status | Medium | Certain |
| 5.2.1 | Split into `ForecastSource`/`AlertSource`/`RadarStatusSource` or a per-cycle snapshot | — | — |
| 5.3 | Forecast and `ClearInput` are cloned each frame | Low | Certain |
| 5.3.1 | Pass borrowed forecast/alerts into the view; remove `ClearInput` | — | — |
| 5.4 | Upstream bodies are unbounded; no gzip/`Accept`/conditional requests | Medium | Medium |
| 5.4.1 | Cap bodies, enable gzip, set `Accept`, log status/latency/bytes | — | — |
| ~~6.1~~ | ~~Palette and SVG icons are re-decoded/re-rasterized every frame~~ | Medium | Certain |
| ~~6.1.1~~ | ~~Cache palette and icon bitmaps at startup~~ | — | — |
| 6.2 | Frames are served from disk and can be read torn | High | Medium |
| 6.2.1 | Publish `Arc<[u8]>` + ETag; serve bytes from memory; support `If-None-Match` | — | — |
| 6.3 | `Renderer` only forwards to `Engine`; config is read twice | Low | Certain |
| 6.3.1 | Collapse the trait or make it return frame metadata; build one validated `Settings` | — | — |
| 7.1 | Global state mutex is held across network I/O and rendering | High | Certain |
| 7.1.1 | Move updates to a background task that publishes an immutable snapshot | — | — |
| 7.1.2 | Interim: never hold the lock across `.await`; use `try_lock`/single-flight | — | — |
| 7.2 | No handler timeouts, graceful shutdown, or panic containment | Medium | Medium |
| 7.2.1 | Add upstream connect/total timeouts and a handler deadline | — | — |
| 7.2.2 | Add graceful shutdown and a logging panic hook/catch-panic layer | — | — |
| 7.3 | Stringly errors force every failure to `500` | Medium | Certain |
| 7.3.1 | Introduce a typed error enum and `IntoResponse` mapping (`503` vs `500`) with short bodies | — | — |
| 7.4 | Config/artifacts are not validated at startup | Medium | High |
| 7.4.1 | Validate bbox, timezone, zones, UA, palette/streets dimensions, and writable `data/`; fail fast | — | — |
| 7.5 | Unit file amplifies failures and permits host-wide OOM | Medium | High |
| 7.5.1 | Add `StartLimit*`, `Restart=on-failure`, `MemoryMax`, `ProtectSystem`, `ReadWritePaths` | — | — |
| 7.6 | Tests miss the failure/concurrency paths above | Medium | Certain |
| 7.6.1 | Add failure/backoff, concurrency/single-flight, forecast-refresh, pagination, poisoned-stamp, atomic-write, startup-validation tests | — | — |

## Progress

| Done | ID | Action |
|:---:|---|---|
| [x] | 1.1.1 | Render base frame first; publish degraded frame on alert failure |
| [x] | 1.1.2 | Serve last good frame on upstream errors; `503` only if none; keep last mode |
| [x] | 1.2.1 | Stream S3 GET to `.part` with a byte cap |
| [x] | 1.2.2 | Cap decompression; reject oversized LDM records; checked arithmetic |
| [x] | 1.3.1 | Temp file + `rename` for all artifacts |
| [x] | 1.3.2 | Validate before replacing last-good volume/frame; retain last-good pair |
| [x] | 1.4.1 | Make streets optional at startup; background retry with backoff |
| [x] | 1.4.2 | Validate OSM/mask before writing OSM file and stamps |
| [x] | 1.5.1 | Skip malformed alerts with a warning |
| [x] | 1.5.2 | Map missing `rda` to down/Clear |
| [x] | 1.6.1 | Distinguish `404`/`500` in `png_file` |
| [x] | 1.6.2 | Handle logging init/rotation failures |
| [x] | 1.6.3 | Add client request timeout |
| [x] | 2.1.1 | Add `/healthz`, `/readyz`, `/status` |
| [x] | 2.2.1 | Structured per-cycle and per-upstream logs |
| [x] | 2.2.2 | Expose counters/ages in `/status` |
| [rejected] | ~~2.3.1~~ | ~~stdout + journald, `RUST_LOG`, 7–14 day retention~~ |
| [x] | 2.4.1 | Startup summary log |
| [x] | 2.4.2 | Per-cycle frame provenance log |
| [x] | 3.1.1 | TTL-cache radar status with single-flight |
| [x] | 3.2.1 | Backoff + jitter with `last_attempt`/`next_attempt` |
| [x] | 3.2.2 | Per-source last-known-good cache |
| [ ] | 3.3.1 | Mode hysteresis/minimum dwell |
| [ ] | 3.4.1 | Persist state and radar key/mtime across restarts |
| [rejected] | ~~3.5.1~~ | ~~Localhost default; auth/rate limit public endpoints~~ |
| [x] | 4.1.1 | Paginate S3 listing; handle MDM-only days |
| [x] | 4.2.1 | Accept partial elevations; pick first available origin |
| [ ] | 4.3.1 | Reusable ring fill with no per-gate allocations |
| [ ] | 4.3.2 | Fixed-point rounding instead of `format!`/`parse` |
| [ ] | 4.3.3 | Burn max directly into the composite |
| [ ] | 4.3.4 | Borrow moment slices |
| [x] | 5.1.1 | Remove process-lifetime forecast cache |
| [x] | 5.1.2 | Two-cycle forecast refresh test |
| [ ] | 5.2.1 | Split provider traits/snapshot |
| [x] | 5.3.1 | Remove forecast/`ClearInput` clones |
| [x] | 5.4.1 | Cap bodies, gzip, `Accept`, upstream logging |
| [rejected] | ~~6.1.1~~ | ~~Cache palette and icons~~ |
| [x] | 6.2.1 | Publish in-memory frame bytes + ETag |
| [ ] | 6.3.1 | Collapse `Renderer`/single `Settings` |
| [ ] | 7.1.1 | Background updater + immutable snapshot |
| [ ] | 7.1.2 | Interim no-lock-across-await / single-flight |
| [x] | 7.2.1 | Upstream and handler timeouts |
| [x] | 7.2.2 | Graceful shutdown + panic hook |
| [ ] | 7.3.1 | Typed errors + HTTP mapping |
| [ ] | 7.4.1 | Startup validation |
| [x] | 7.5.1 | systemd hardening |
| [ ] | 7.6.1 | Add regression tests |

## Details

### 1. Error handling and crash exposure

**1.1 Upstream failure blocks serving a valid frame — High / Certain.**
`api.rs:239-269` returns `500` when radar status, S3, alerts, or render fails, even when `data/frame.png` is valid; `last_updated` advances only on success.
- **1.1.1** Split each mode into *collect → render → publish*. Publish the radar/forecast frame even when `alerts()` fails; record `degraded` plus the error.
- **1.1.2** On any upstream error, return the last published frame with `200`. Return `503` only before the first successful frame. A radar-status error must not change the mode or block `304`/`200`.

**1.2 Unbounded download/decompression — High / Medium.**
`nexrad.rs:58` buffers and copies the whole object; `archive2.rs:159,201` expand gzip/bzip2 without a limit. A corrupt/hostile stream can OOM-kill the service.
- **1.2.1** Stream the S3 GET to `data/nexrad-latest.ar2.part` with a size cap and `Content-Length` check.
- **1.2.2** Wrap decoders in `Read::take(N)`; reject LDM records above a configured maximum; use `checked_add` for offsets.

**1.3 Non-atomic writes / last-good loss — High / High.**
`modes.rs:41-44` overwrites `nexrad-latest.ar2` before parsing; `image.rs:107-108` and `pngio.rs:130,165` truncate outputs in place; `?debug=true` bypasses the state lock and can read a torn PNG.
- **1.3.1** Write to a temp file in the same directory, `fsync`, then `rename()`; apply to `frame.png`, `nexrad-latest.png`, `.ar2`, OSM, stamps.
- **1.3.2** Parse/validate a downloaded volume and render the radar frame before committing. Keep `last-good.ar2`/`last-good.png`; on failure, restore/serve the previous pair and do not advance `radar_last_updated`.

**1.4 Startup coupled to Overpass / poisoned stamp — High / High.**
`cli.rs:238` runs `create_streets` before binding; `setup.rs:111-117` writes the OSM body and stamp before XML validation. A `200` error page is permanently stamped as valid; `Restart=always` + 10 s then loops one Overpass request per restart.
- **1.4.1** If the mask is missing/stale and Overpass fails, warn and serve without the overlay; retry in the background with exponential backoff. Do not fail startup for a cosmetic layer.
- **1.4.2** Parse and render the OSM to a temp mask, then write `streets.osm`, stamps, and mask atomically; store a content hash. Unreadable stamps must trigger regeneration, not silent adoption.

**1.5 Fragile upstream parsing — Medium / Medium.**
`nws.rs:93-119` fails the entire alert list on one bad timestamp; `nws.rs:141-147` errors when `rda` is absent, producing `500` every poll for a decommissioned station.
- **1.5.1** Skip invalid alert features with a warning; render the rest.
- **1.5.2** Map missing `rda`/unparsable station to `RadarStatus { up: false, vcp: -1 }` (Clear).

**1.6 Smaller gaps — Medium / High.**
- **1.6.1** `api.rs:278-282`: return `404` only for `NotFound`, otherwise `500`.
- **1.6.2** `logging.rs:115-129`: fail fast or fall back to stderr; log rotation failures instead of ignoring them (`logging.rs:78-100`).
- **1.6.3** `client/main.py:20`: add `timeout=(5, 30)` so a blocked server cannot hang the Pi forever.

### 2. Observability

**2.1 No operational endpoints — Medium / Certain.** There is no way to read mode, VCP, frame age, or upstream health except by parsing HTML/logs.
- **2.1.1** Add `/healthz` (liveness), `/readyz` (frame exists and age ≤ 2× interval, `data/` writable), and `/status` JSON.

**2.2 No structured events/metrics — Medium / Certain.** Logs lack durations, statuses, byte counts, request/cycle correlation, and counters.
- **2.2.1** Log `event=cycle mode= outcome= elapsed_ms= volume_key= alerts= frame_bytes= frame_sha=`, and `event=upstream source= status= elapsed_ms= bytes= error_kind=`.
- **2.2.2** Track request/update/upstream counters and latencies; expose in `/status` (or Prometheus if desired).

**2.3 Log sink/retention — Medium / Certain.** 100 kB × 4 rotates within days; tracing writes only to the file, so `journalctl` is empty after startup.
- **2.3.1** Write to stdout and journald, optionally also the file; use `EnvFilter`/`RUST_LOG`; retain 7–14 days; log rotation failures.

**2.4 No provenance — Medium / Certain.**
- **2.4.1** Startup summary: version, bbox, station, zones, timezone, endpoints, data path/writability.
- **2.4.2** Per-cycle: mode, volume key, elevations found, radial/gate count, alert count, render duration, output size/SHA.

### 3. Free API use and retry spirals

**3.1 NWS status per poll — High / Certain.** `api.rs:239` calls the station endpoint on every `/frame` in Auto mode (720/day per 2-minute client).
- **3.1.1** TTL-cache the status for 60 s with single-flight; use last-known-good on error. This is the interim step until `7.1.1`.

**3.2 No backoff — High / Certain.** `api.rs:266-269` leaves `last_updated` unchanged on failure; every poll repeats the mode and all upstream calls.
- **3.2.1** Add `last_attempt`/`next_attempt`; back off `1,2,4,8,16 min` with jitter, capped at the mode interval, reset on success.
- **3.2.2** Cache last-known-good per source and serve it while backing off; do not let failure state be “retry now” forever.

**3.3 Flapping — Medium / Medium.** A mode change resets `last_updated` to epoch (`api.rs:241-243`), forcing an immediate render; alternating `Operate`/`Standby` causes repeated cycles.
- **3.3.1** Require N consecutive samples or a minimum dwell time before switching.

**3.4 Restart-driven requests — Medium / High.** State is memory-only, so a restart re-downloads the newest volume and loses manual selection.
- **3.4.1** Persist `mode_select`, `manual_til`, `last_updated`, and radar key/mtime in `data/state.json`; load at startup.

**3.5 Amplification/exposure — Medium / Medium.** `cli.rs:71` binds `0.0.0.0`; `/frame` and `/settings` are unauthenticated, so any client can drive upstream calls and serialize the server.
- **3.5.1** Bind `127.0.0.1` by default; add a bearer token or rate limit if exposed beyond the LAN.

### 4. Radar processing pipeline

**4.1 S3 pagination — High / Medium.** `nexrad.rs:70-86` reads only the first ListObjectsV2 page and ignores `IsTruncated`; above 1000 keys/day the newest volume is never seen.
- **4.1.1** Loop `continuation-token` until complete; parse `IsTruncated`; if today's page contains only `_MDM`, fall back to yesterday.

**4.2 Hard elevation 1 — Medium / Medium.** `render/archive.rs:70-94` errors when elevation 1 or any requested elevation is missing.
- **4.2.1** Use the first available scan for the projection origin; composite whichever requested elevations exist; log missing ones.

**4.3 Hot-loop waste — Medium / Certain.** Per radial: `Vec<Bin>`; per gate: `format!`/`parse` for five coordinates and one value; per polygon: several `Vec` allocations in `fill_polygon`.
- **4.3.1** Add `fill_ring(grid, w, h, &[(f64,f64);4], value)` using a reusable row/intersection buffer; drop `Vec<Bin>` and the duplicate closing point.
- **4.3.2** Implement exact fixed-point rounding that reproduces `{:.4}`/`{:.1}`; test against the current formatter. This removes tens of millions of heap allocations per volume.
- **4.3.3** Add a `burn_max` fill and write directly into the composite; remove the per-elevation grid plus `merge` pass.
- **4.3.4** Store moment data as `&[u8]` ranges into the decompressed buffer instead of `to_vec()` per moment.

Keep the GDAL-parity goldens; make these changes behind the existing tests.

### 5. Weather data pipeline

**5.1 Forecast cached for process lifetime — Critical / Certain.** `online.rs:15,33-39` uses a `OnceCell`; the provider is built once in `cli.rs:226` and stored in `AppState` (`cli.rs:242`). Clear mode therefore serves the startup forecast forever. The comment claiming once-per-cycle is wrong.
- **5.1.1** Remove the `OnceCell`; fetch once per `modes::clear` call (already the only caller), or use a short TTL cache with last-good fallback.
- **5.1.2** Add a regression test with a counting provider across two Clear cycles; the second cycle must fetch again.

**5.2 Mixed trait — Medium / Certain.** `ForecastProvider` couples forecast, alerts, and radar status, forcing unrelated mocks and error coupling.
- **5.2.1** Split into three traits or return a per-cycle `WeatherSnapshot` with independent `Result`s.

**5.3 Clones — Low / Certain.** `modes.rs:68` and `engine.rs:124-146` clone the full forecast into `ClearInput`.
- **5.3.1** Make the view take `&Forecast`/`&[Alert]`; delete `ClearInput` or make it borrow.

**5.4 Upstream policy — Medium / Medium.** Bodies are unbounded; gzip is not enabled; no `Accept`; no conditional requests.
- **5.4.1** Cap bodies (e.g., 2 MB), enable reqwest gzip, set `Accept`, and log status/latency/bytes (never bodies).

### 6. Frame generation pipeline

**6.1 Static assets — Medium / Certain.** `view/icons.rs:39-60` parses and rasterizes seven SVGs per clear frame; the palette is re-decoded each render.
- **6.1.1** Cache palette, icon bitmaps, and fonts at startup; reuse across frames.

**6.2 Serving from disk — High / Medium.** `/frame` re-reads 126 kB from disk; `?debug=true` can observe a partial write.
- **6.2.1** Publish `Arc<Frame { bytes, etag, generated_at, mode, degraded }>` and serve from memory; return `304` for matching `If-None-Match`. With `1.3.1`, the file is only a debug artifact.

**6.3 Trait/config duplication — Low / Certain.** `Renderer` only forwards to `Engine`; bounds/timezone are parsed in both `AppState` and `Engine`.
- **6.3.1** Either delete `Renderer` (test `Engine` directly) or have it return frame metadata; build one validated `Settings` shared by server and engine.

### 7. Server code

**7.1 Global mutex across I/O — High / Certain.** `api.rs:228-269` holds `Mutex<State>` across NWS/S3/Open-Meteo and rendering; `/` (`api.rs:145`) and `/settings` (`api.rs:182`) queue behind it.
- **7.1.1** Move scheduling to one background task (1-minute tick) that owns `State`, performs updates, and publishes an immutable snapshot via `watch`/`ArcSwap`. Handlers read the snapshot with no lock and no network.
- **7.1.2** Interim: never hold the lock across `.await`; use `try_lock` for snapshots and a single-flight guard for updates (return `304` while one is in flight).

**7.2 Timeouts/shutdown/panics — Medium / Medium.** Only one 30 s reqwest timeout exists; sequential upstream calls can hold a request for minutes. No graceful shutdown or panic hook.
- **7.2.1** Add connect/total timeouts per upstream and a handler deadline; abort the cycle cleanly and record the failure.
- **7.2.2** Add `with_graceful_shutdown` for SIGTERM/SIGINT and a panic hook (optionally a catch-panic layer) that logs and returns `500`.

**7.3 Stringly errors — Medium / Certain.** `error.rs` is `Error(String)`; every failure becomes `500`.
- **7.3.1** Add `Config`, `Io`, `Http { status, retry_after }`, `Parse`, `Render`; map upstream failures to `503` + `Retry-After`, internal to `500`, and return a short text body.

**7.4 No startup validation — Medium / High.** `serve` starts with an invalid bbox, missing zones, or wrong artifact dimensions; failures appear only on the first request.
- **7.4.1** Validate config fields, bbox via `Transform::new`, timezone, zones, non-placeholder UA, palette (7-colour indexed), streets (800×444 gray), and `data/` writability. Fail in <1 s with a clear message.

**7.5 Unit hardening — Medium / High.** `radar-frame.example.service:8-9` uses `Restart=always` forever and no memory limit.
- **7.5.1** Set `StartLimitIntervalSec=300`, `StartLimitBurst=6`, `Restart=on-failure`; add `MemoryMax=1G`, `MemoryHigh=800M`, `ProtectSystem=strict`, `ReadWritePaths=/opt/radar-frame/data`, `PrivateTmp=true`, `Environment=RUST_LOG=info`.

**7.6 Test gaps — Medium / Certain.**
- **7.6.1** Add tests for: upstream failure + backoff/stale serving; concurrent `/frame` single-flight; forecast refresh across cycles; S3 truncation; poisoned OSM stamp; atomic write/reader isolation; startup validation. The existing API tests use `MockProvider` and a single cycle, which is why 5.1 escaped.

## Verification

Release binary, local HTTPS proxy returning `502` after 1 s:

- 3 sequential `/frame` → `500` each at ~1.0 s, **3** `CONNECT api.weather.gov:443`, log still `last updated: 1970-01-01`. Confirms 3.1/3.2.
- 4 concurrent `/frame` → 4.0 s wall, **4** upstream `CONNECT`s. Confirms 7.1 serialization and amplification.
- `GET /` 0.4 s after `/frame` started → **0.599 s** (blocked until the frame released the lock). Confirms 7.1.
- Two `setup` invocations with the mask removed → one Overpass request each, each failing the process. With `RestartSec=10`, this becomes the 1.4 loop.

No repository files were changed during verification; the temporary test directory was removed.
