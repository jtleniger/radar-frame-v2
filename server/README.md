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

### Zones
https://www.weather.gov/pimar/PubZone