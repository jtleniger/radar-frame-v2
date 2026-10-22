#!/usr/bin/env python3
"""Regenerate the golden HTTP responses from the Python Flask service.

Run from anywhere, with the Python requirements and the release `frame-view`
binary available:

    cargo build --release --manifest-path server/frame-view/Cargo.toml
    python3 server/radar-frame/tests/fixtures/capture.py

The script runs Flask in a temporary working directory whose `views/`,
`static/`, `config/`, `data/`, and `bin/` mirror the deployed layout, freezes
the clock at 2026-10-21T15:30:00Z, and drives each request through the test
client. It then writes the HTML, PNG, and status codes next to this file under
`golden/`. No network request is made: NWS responses come from
`nws-alerts.json`, and the radar volume is never consulted (the storm case
stubs the download and reuses the frame-render golden radar image).
"""

import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

FIXTURES = Path(__file__).resolve().parent
SERVER = FIXTURES.parents[2]
GOLDEN = FIXTURES / "golden"

NOW = "2026-10-21T15:30:00Z"
TIMEZONE = "America/Chicago"

CONFIG = f"""[logging]
level = INFO

[radar]
nexrad_id = KTLX

[bbox]
min_lat = 35.35
max_lat = 35.39
min_lon = -97.26
max_lon = -97.21

[forecast]
lat = 35.37
lon = -97.23
timezone = {TIMEZONE}

[nws]
user_agent = radar-frame (jtleniger@gmail.com)
zone = OKZ025
fire_zone = OKZ025
"""


class FakeResponse:
    def __init__(self, body: str):
        self.status_code = 200
        self.text = body

    def json(self):
        return json.loads(self.text)


class FakeRequests:
    """Stands in for `requests` in `sources/nws_api.py`."""

    def __init__(self, body: str):
        self.body = body
        self.calls = 0

    def get(self, url, headers=None):
        self.calls += 1
        assert "jtleniger@gmail.com" in headers["User-Agent"], headers
        return FakeResponse(self.body)


def frozen_run(command, payload, output, radar=None):
    """`views.frame_view.run` with a fixed `--now`."""
    from views import frame_view

    config = frame_view.Config.instance()
    cmd = [frame_view.paths.FRAME_VIEW_BIN.as_posix(), command]
    if radar is not None:
        cmd += ["--radar", radar]
    cmd += [
        "--palette", frame_view.paths.PALETTE_IMG.as_posix(),
        "--output", output.as_posix(),
        "--timezone", config["forecast"]["timezone"],
        "--now", NOW,
    ]

    result = subprocess.run(cmd, input=json.dumps(payload), text=True, capture_output=True)
    if result.returncode != 0:
        raise RuntimeError(f"{cmd}: {result.stdout}\n{result.stderr}")


def prepare_root() -> Path:
    root = Path(tempfile.mkdtemp(prefix="radar-frame-capture-"))
    (root / "config").mkdir()
    (root / "data").mkdir()
    (root / "bin").mkdir()
    (root / "config" / "config.ini").write_text(CONFIG)
    shutil.copytree(SERVER / "views", root / "views")
    shutil.copytree(SERVER / "static", root / "static")
    shutil.copy(SERVER / "gdal-colors.txt", root / "gdal-colors.txt")
    shutil.copy(SERVER / "frame-view/target/release/frame-view", root / "bin" / "frame-view")

    subprocess.run(
        [root / "bin/frame-view", "palette", "--output", root / "data/palette.png"],
        check=True,
    )

    # The radar frame the storm view consumes is the frame-render golden: it is
    # also what the Rust engine produced for the same fixture volume and bbox.
    radar_png = SERVER / "frame-render/tests/fixtures/ktlx-mini/frame.expected.png"
    shutil.copy(radar_png, root / "data/nexrad-latest.png")
    shutil.copy(radar_png, root / "data/frame.png")
    return root


def main() -> None:
    root = prepare_root()
    os.chdir(root)
    sys.path.insert(0, str(SERVER))

    import datetime as dt

    import server.api as api
    import state.state as state_mod
    import sources.forecast as forecast
    import sources.mock_forecast as mock_forecast
    import sources.nexrad_level2 as nexrad
    import sources.nws_api as nws_api
    import modes.storm as storm_mode
    from server import server
    from views import frame_view

    frozen = dt.datetime(2026, 10, 21, 15, 30, tzinfo=dt.timezone.utc)

    class Frozen(dt.datetime):
        @classmethod
        def now(cls, tz=None):
            if tz is None:
                return frozen.astimezone(dt.timezone.utc).replace(tzinfo=None)
            return frozen.astimezone(tz)

    api.datetime = Frozen
    frame_view.run = lambda command, payload, output, radar=None: frozen_run(
        command, payload, output, radar
    )

    nws_body = (FIXTURES / "nws-alerts.json").read_text()
    fake_requests = FakeRequests(nws_body)
    nws_api.requests = fake_requests

    def fresh_client():
        state_mod.State._state = None
        return server.create().test_client()

    statuses = {}
    GOLDEN.mkdir(exist_ok=True)

    def save(name, response):
        statuses[name] = response.status_code
        return response

    client = fresh_client()
    response = save("index", client.get("/"))
    (GOLDEN / "index.html").write_bytes(response.data)

    client = fresh_client()
    response = save("settings-auto", client.post("/settings", data={"mode-select": "Auto"}))
    (GOLDEN / "settings-auto.html").write_bytes(response.data)

    client = fresh_client()
    response = save(
        "settings-manual-storm",
        client.post("/settings", data={"mode-select": "Manual", "mode": "Storm"}),
    )
    (GOLDEN / "settings-manual-storm.html").write_bytes(response.data)
    (GOLDEN / "index-manual-storm.html").write_bytes(client.get("/").data)

    client = fresh_client()
    response = save(
        "settings-manual-no-mode", client.post("/settings", data={"mode-select": "Manual"})
    )
    (GOLDEN / "settings-manual-no-mode.html").write_bytes(response.data)

    client = fresh_client()
    response = save("debug-frame", client.get("/frame?debug=true"))
    assert response.headers["Content-Type"] == "image/png", response.headers
    assert response.data == (root / "data/frame.png").read_bytes()
    (GOLDEN / "debug-frame.png").write_bytes(response.data)

    # Clear-air schedule: one 200 followed by 304 until the interval elapses.
    client = fresh_client()
    api.OnlineForecast = mock_forecast.MockForecast
    api.nws_api.radar_status = lambda: forecast.RadarStatus(True, 31)
    response = save("clear-frame", client.get("/frame"))
    assert response.headers["Content-Type"] == "image/png"
    (GOLDEN / "clear-frame.png").write_bytes((root / "data/frame.png").read_bytes())
    save("clear-not-modified", client.get("/frame"))

    # Precipitation schedule with the NWS alert fixture.
    class StormProvider:
        @staticmethod
        def alerts():
            return nws_api.alerts()

        @staticmethod
        def current():
            raise AssertionError("the storm view must not fetch current conditions")

        hourly = daily = current

    client = fresh_client()
    nexrad.latest_object = lambda: None
    storm_mode.render_radar = lambda: None
    api.nws_api.radar_status = lambda: forecast.RadarStatus(True, 212)
    api.OnlineForecast = StormProvider

    response = save("storm-frame", client.get("/frame"))
    assert response.headers["Content-Type"] == "image/png"
    (GOLDEN / "storm-frame.png").write_bytes((root / "data/frame.png").read_bytes())
    save("storm-not-modified", client.get("/frame"))

    assert fake_requests.calls == 1, f"alerts fetched {fake_requests.calls} times"

    (GOLDEN / "responses.json").write_text(json.dumps(statuses, indent=2, sort_keys=True) + "\n")
    print(f"wrote {GOLDEN}")
    print(json.dumps(statuses, indent=2, sort_keys=True))


if __name__ == "__main__":
    main()
