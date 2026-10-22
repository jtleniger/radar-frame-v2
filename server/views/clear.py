from typing import List

from constants import paths
from sources import forecast
from views import frame_view


def render(
        current: forecast.CurrentConditions,
        hourly: List[forecast.ForecastHour],
        daily: List[forecast.ForecastDay],
        alerts: List[forecast.Alert]):
    """Current, hourly, and daily forecast panels composed by frame-view."""
    frame_view.run(
        'clear',
        {
            'current': {
                'temp_f': current.temp_f,
                'code': current.code,
                'is_day': current.is_day,
            },
            'hourly': [
                {'hour': h.hour, 'temp_f': h.temp_f, 'code': h.code}
                for h in hourly
            ],
            'daily': [
                {'day': d.day, 'high_f': d.high_f, 'low_f': d.low_f, 'code': d.code}
                for d in daily
            ],
            'alerts': [frame_view.alert(a) for a in alerts],
        },
        paths.OUTPUT_IMG,
    )
