import subprocess
import logging
from pathlib import Path

from config.config import Config
from constants import paths, radar

_logger = logging.getLogger(__name__)


def _run_and_check(cmd):
    _logger.info('command:')
    _logger.info(' '.join(cmd))
    try:
        subprocess.run(cmd, check=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    except subprocess.CalledProcessError as e:
        _logger.error(f'Command failed with exit code {e.returncode}')
        _logger.error(f'STDOUT: {e.stdout}')
        _logger.error(f'STDERR: {e.stderr}')
        raise


def render(volume=paths.RADAR_RAW, minimum: float = 10.0):
    config = Config.instance()
    volume = Path(volume)

    # One process reads the volume, projects the gates, and rasterizes the
    # frame; no per-elevation GeoJSON files are written.
    _run_and_check([
        paths.FRAME_RENDER_BIN.as_posix(),
        'frame',
        '--archive', volume.as_posix(),
        '--minimum', str(minimum),
        '--elevations', '1-3',
        '--min-lon', config['bbox']['min_lon'],
        '--min-lat', config['bbox']['min_lat'],
        '--max-lon', config['bbox']['max_lon'],
        '--max-lat', config['bbox']['max_lat'],
        '--width', str(radar.WIDTH),
        '--height', str(radar.HEIGHT),
        '--colors', paths.COLORS_FILE.as_posix(),
        '--streets', paths.STREETS_IMG.as_posix(),
        '--output', paths.RADAR_IMG.as_posix(),
    ])
