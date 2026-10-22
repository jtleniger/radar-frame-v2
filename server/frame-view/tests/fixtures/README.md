# Fixtures

Generated from the Python view implementation at `202792a` (the last commit
before `server/frame-view`), with:

- Python 3.13.5
- Pillow 11.1.0
- CairoSVG 2.9.1
- Fixed time `2025-06-01T12:34:56Z`, time zone `America/Chicago`

| File | Meaning |
|---|---|
| `radar.png` | 800x444 radar frame, copied from `frame-render/tests/fixtures/ktlx-mini/frame.expected.png` (the GDAL renderer's golden) |
| `forecast.json` | Mock current/hourly/daily data, serialized as `frame-view clear` expects it |
| `alerts.json` | A Watch and a Warning used for both views; the Watch wins because the Python view sorts alerts by ascending severity |
| `clear.expected.png` | `views.clear.render(...)`, palette-quantized without dithering |
| `storm.expected.png` | `views.storm.render(...)`, Floyd–Steinberg quantized |

To regenerate, check out `202792a`, install the packages above, put `radar.png`
at `data/nexrad-latest.png`, make a `data/palette.png` with the seven palette
colours, and run the Python views with `components.info.info.datetime` replaced
by a fixed clock (this is what the old `main.py render-volume` and
`render-storm` commands did, with `views.storm.render(alerts)` /
`views.clear.render(...)`).

The tests in `tests/render.rs` compare pixel-identical output for the info bar
and allow differences at icon edges and in the dither pattern; see the crate
README.
