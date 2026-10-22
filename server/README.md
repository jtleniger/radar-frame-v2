# radar-frame
## Raspberry Pi powered e-ink weather display
### Server
Build the local binaries once after checkout:

```sh
cargo build --release --manifest-path nexrad-geojson/Cargo.toml
cargo build --release --manifest-path frame-render/Cargo.toml
mkdir -p bin
cp nexrad-geojson/target/release/nexrad-geojson bin/
cp frame-render/target/release/frame-render bin/
```

Run the server:

```sh
gunicorn --timeout 90 --log-level=info -b 0.0.0.0:8000 "main:create_server()"
```

### Inspecting a historical volume

The NEXRAD archive bucket is public, so no credentials are needed. For example,
the 2013 Moore EF5 tornado volume from KTLX:

```sh
curl -o data/nexrad-latest.ar2 \
  "https://unidata-nexrad-level2.s3.amazonaws.com/2013/05/20/KTLX/KTLX20130520_205914_V06.gz"

python3 main.py render-volume --file data/nexrad-latest.ar2
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

### Zones
https://www.weather.gov/pimar/PubZone