# `ktlx-mini` frame golden

`frame.expected.png` is the frame that the **pre-change GDAL/PIL renderer**
produced (`components/radar/render.py` at `main` before the Rust cutover:
`gdal_rasterize` x3, `gdal_calc.py`, `gdaldem color-relief`, `gdal_translate`,
then two Pillow `paste` calls). It is the reference for
`tests/cli.rs::matches_the_gdal_golden_frame`; never regenerate it with the Rust
renderer.

`streets.png` is the street mask that the pre-change GDAL sequence in
`server/setup/streets.py` produced (`gdal_rasterize` over the OSM `lines`
layer, then `gdal_translate` to PNG). It is the reference for
`tests/cli.rs::street_mask_matches_the_gdal_golden_mask`.

`streets.osm` is a minimal Overpass-style OSM XML document that produces the
same mask: one primary highway way with three nodes forming the diagonal line
used to generate `streets.png`. The Rust street rasterizer parses this XML and
must reproduce the GDAL golden pixel for pixel.

Versions used: GDAL 3.10.3 (Debian `gdal-bin` 3.10.3, `python3-gdal` 3.10.3),
Pillow 11.1.0, NumPy 2.2.4.

Fixed render parameters: bbox `min_lon=-97.26, min_lat=35.35,
max_lon=-97.21, max_lat=35.39`, size `800x444`, color table
`server/gdal-colors.txt`.

## Regenerating

From the repo root, with `server/bin/nexrad-geojson` built:

```sh
rm -rf /tmp/frameverify && mkdir -p /tmp/frameverify/golden
git archive main server | tar -x -C /tmp/frameverify/golden   # pre-change renderer
cd /tmp/frameverify/golden/server
mkdir -p bin data config
cp "$OLDPWD"/server/bin/nexrad-geojson bin/
cp "$OLDPWD"/server/nexrad-geojson/tests/fixtures/ktlx-mini.ar2 data/nexrad-latest.ar2
# config/config.ini: [radar] nexrad_id = KTLX, [bbox] the bounds above,
# [logging] level = INFO, [forecast], [nws] as in server/config/config.ini.dist
bin/nexrad-geojson --minimum 10 -e 1 -p REF -o data/nexrad-latest data/nexrad-latest.ar2

# The mini volume has a single elevation; the renderer always reads three.
cp data/nexrad-latest-REF-1.json data/nexrad-latest-REF-2.json
cp data/nexrad-latest-REF-1.json data/nexrad-latest-REF-3.json

# Street mask: one diagonal line, burned the way setup/streets.py does it.
cat > /tmp/frameverify/streets.geojson <<'EOF'
{"type":"FeatureCollection","features":[{"type":"Feature","properties":{},"geometry":{"type":"LineString","coordinates":[[-97.27,35.34],[-97.25,35.40],[-97.20,35.36]]}}]}
EOF
gdal_rasterize -q -burn 255 -ot byte -a_nodata 1 \
  -te -97.26 35.35 -97.21 35.39 -ts 800 444 \
  /tmp/frameverify/streets.geojson data/streets.tif
gdal_translate -q -of PNG data/streets.tif data/streets.png

python3 -c 'from components.radar.render import render; render()'
cp data/nexrad-latest.png /tmp/frameverify/golden.png
```

Then copy into this directory:

```sh
cp data/nexrad-latest-REF-1.json server/frame-render/tests/fixtures/ktlx-mini/
cp data/streets.png server/frame-render/tests/fixtures/ktlx-mini/
cp /tmp/frameverify/golden.png server/frame-render/tests/fixtures/ktlx-mini/frame.expected.png
```

## Notes on the fixture bytes

Two properties of the old pipeline are load-bearing and are reproduced by the
Rust renderer:

- `gdal_calc.py` defaults an unspecified output nodata to the Byte maximum
  (255), and the rasterized gates carry nodata 0, so every pixel no elevation
  covers is 255. `gdaldem` then maps 255 to the **last** color table entry,
  which is why the uncovered background is opaque `(46, 11, 119)`.
- `streets.png` carries a `tRNS` chunk (gray level 1), written by GDAL for the
  `-a_nodata 1` of `setup/streets.py`. Pillow turns those samples fully
  transparent when it pastes the mask, so uncovered background pixels end up
  with alpha 254 rather than 255.

`tests/cli.rs` compares decoded RGBA pixels, not file bytes; PNG encoders
differ. Current result: 0 differing pixels of 355200.
