# frame-view

Composes the 800x480 e-ink frames that the server serves on `/frame`:

- `storm`: the 800x444 radar frame plus the info bar, dithered onto the
  seven-colour e-ink palette.
- `clear`: current conditions, hourly, and daily forecast panels plus the info
  bar, mapped to the palette without dithering.
- `palette`: writes the 7x1 indexed palette PNG (`data/palette.png`).

This replaces `components/info`, `components/forecast`, `components/font`, and
`setup/palette.py` from the Python implementation. `views/storm.py` and
`views/clear.py` still gather data from the Python sources; they pass it to this
binary as JSON on stdin.

## Command line

```sh
frame-view storm \
  --radar data/nexrad-latest.png \
  --palette data/palette.png \
  --output data/frame.png \
  --timezone America/Chicago \
  [--now 2025-06-01T12:34:56Z] \
  < alerts.json

frame-view clear \
  --palette data/palette.png \
  --output data/frame.png \
  --timezone America/Chicago \
  [--now 2025-06-01T12:34:56Z] \
  < forecast.json

frame-view palette --output data/palette.png
```

`--now` exists for fixtures and tests; without it the current UTC time is used.
The time zone only affects the info-bar timestamp and alert activity checks.

### `storm` stdin: `Vec<Alert>`

```json
[
  {
    "event": "flash flood watch",
    "level": "Watch",
    "status": "actual",
    "effective": "2025-06-01T07:24:56-05:00",
    "expires": "2025-06-01T08:34:56-05:00"
  }
]
```

`level` is one of `Info`, `Watch`, `Warning`, `Emergency`. Only `actual` alerts
whose interval contains now are drawn. As in the Python view, alerts are sorted
by ascending severity and the first one is shown (the historical behaviour).

### `clear` stdin: `ClearInput`

```json
{
  "current": {"temp_f": 101.0, "code": 57, "is_day": false},
  "hourly": [{"hour": "9pm", "temp_f": 24.0, "code": 3}],
  "daily": [{"day": "tue", "high_f": 38.8, "low_f": 123.2, "code": 1}],
  "alerts": []
}
```

Hourly and daily lists may be longer than the three entries drawn; every other
hourly entry is used, as before.

## Rendering

- Type is Spleen, embedded as the original OTF files and rasterized in
  monochrome mode at Pillow's sizes (32px Small, 64px Large, 128px XLarge).
- Weather icons are the 17 Weather Icons SVGs the weather-code mapping uses,
  embedded and rendered with resvg. The Python path rasterized them with
  CairoSVG and reduced them to two colours; the result is the same black shape
  with edge pixels differing by a few percent.
- The palette is read from the PNG written by `palette`; nearest-colour mapping
  is plain RGB distance, with Floyd–Steinberg diffusion for the storm frame and
  no dithering for the clear frame, matching the two Pillow calls.

## Tests

```sh
cargo test --all-targets
```

`tests/render.rs` compares both views against images produced by the Python
implementation before this crate landed (see `tests/fixtures/README.md`). The
info bar is pixel-identical; the clear frame is >99% identical with differences
confined to icon edges; the storm frame's dither pattern differs pixel-by-pixel
because the diffusion implementation is not Pillow's, but 8x8 block averages
stay within 4/255.
