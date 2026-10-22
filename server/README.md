# radar-frame
## Raspberry Pi powered e-ink weather display
### Server

The server is the Rust `radar-frame` binary: HTTP, the mode schedule, the
forecast sources, and the setup artifacts. It calls `frame-render` (radar frame
and street mask) and `frame-view` (e-ink frame and palette) as libraries, so
there is no Python, Flask, gunicorn, or subprocess left in the deployment.

Build the binary once after checkout:

```sh
cargo build --release --manifest-path radar-frame/Cargo.toml
mkdir -p bin
cp radar-frame/target/release/radar-frame bin/
```

The standalone CLIs are only needed for debugging. Build them the same way if
you want them (the service itself does not use them):

```sh
cargo build --release --manifest-path frame-render/Cargo.toml
cargo build --release --manifest-path frame-view/Cargo.toml
cp frame-render/target/release/frame-render bin/
cp frame-view/target/release/frame-view bin/
```

`nexrad-geojson` stays available from the same checkout for hand-made GeoJSON
(VEL, SW, ZDR, PHI, RHO); the server does not call it.

Configure it by copying the example and editing it:

```sh
cp config/config.example.ini config/config.ini
```

Set `[radar] nexrad_id`, the `[bbox]` for the frame, `[forecast] lat`, `lon`, and
`timezone`, and `[nws] user_agent` (a contact address; the NWS API asks for
one), `zone`, and `fire_zone`. The example `[bbox]` values are placeholders:
`frame-render` rejects a zero-height box.

Run the server from `server/`:

```sh
bin/radar-frame serve [--bind 0.0.0.0:8000] [--mock]
```

It binds `0.0.0.0:8000` and writes `radar-frame.log` in the working directory.
On startup it creates `data/`, `data/palette.png`, and the street mask (the
mask is downloaded only when it is missing or stamped for a different
`[bbox]`). `--mock` uses the built-in forecast data.

All outbound requests (Open-Meteo, NWS, S3, Overpass) carry the `User-Agent`
from `[nws] user_agent`. When the setting is missing the generic `radar-frame`
value is used. Forecast data is fetched once per scheduled update.

### Deployment

The service file expects the repository's `server/` directory at
`/opt/radar-frame/server`, running as the `radar-frame` user:

```sh
sudo useradd --system --user-group --home /opt/radar-frame --shell /usr/sbin/nologin radar-frame
sudo mkdir -p /opt/radar-frame/server/bin /opt/radar-frame/server/config
sudo cp bin/radar-frame /opt/radar-frame/server/bin/
sudo cp gdal-colors.txt /opt/radar-frame/server/
sudo cp config/config.example.ini /opt/radar-frame/server/config/config.ini
sudo cp radar-frame.example.service /etc/systemd/system/radar-frame.service
sudo chown -R radar-frame:radar-frame /opt/radar-frame/server
sudo systemctl daemon-reload
sudo systemctl enable --now radar-frame
```

Edit `/opt/radar-frame/server/config/config.ini` before starting. The process
needs write access to its working directory for `data/` and `radar-frame.log`;
`chown` above covers both. Logs rotate at 100 kB with four backups.

The first start downloads the street mask from Overpass. That service rate
limits automated clients, so if it refuses the request, write
`data/streets.osm` by hand with the query in `MIGRATION.md` (unit 2c) and
restart; the hand-written file is adopted for the current `[bbox]`. Run
`bin/radar-frame setup --force-streets` to re-download and re-rasterize
explicitly.

### Inspecting a historical volume

The NEXRAD archive bucket is public, so no credentials are needed. For example,
the 2013 Moore EF5 tornado volume from KTLX:

```sh
curl -o data/nexrad-latest.ar2 \
  "https://unidata-nexrad-level2.s3.amazonaws.com/2013/05/20/KTLX/KTLX20130520_205914_V06.gz"

bin/radar-frame render-volume --file data/nexrad-latest.ar2
```

This writes `data/nexrad-latest.png` (800x444 radar frame) and
`data/frame.png` (800x480 e-ink frame) using the `[bbox]` from
`config/config.ini`. Set it to the event area before rendering. For the KTLX
volume above, use the whole supercell around the radar site; the same box also
drives the OSM street mask:

```ini
[radar]
nexrad_id = KTLX

[bbox]
min_lat = 35.1615
max_lat = 35.4785
min_lon = -97.85
max_lon = -97.15

[forecast]
timezone = America/Chicago
```

Expected result for this volume and bbox: 800x444 with 146,670 covered pixels
of 355,200 and 22,414 pixels at 41 dBZ or more; KTLX is a 9x9
background-coloured hole around (654, 204). Pass `--minimum <dBZ>` to change
the value cutoff (default 10).

`render-clear [--mock]` and `render-storm [--mock]` compose a frame without
serving it over HTTP.

### Zones
https://www.weather.gov/pimar/PubZone
