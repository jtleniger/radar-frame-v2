from typing import List

from constants import paths
from sources import forecast
from views import frame_view


def render(alerts: List[forecast.Alert]):
    """Radar frame plus the info bar, composed by the Rust frame-view binary."""
    frame_view.run(
        'storm',
        [frame_view.alert(alert) for alert in alerts],
        paths.OUTPUT_IMG,
        radar=paths.RADAR_IMG.as_posix(),
    )
