import subprocess
import logging

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


def render():
    config = Config.instance()

    _run_and_check([
        paths.NEXRAD_JSON_BIN.as_posix(),
        '--minimum', '10',
        '-e', '1-3',
        '-p', 'REF',
        '-o', paths.RADAR_JSON_BASENAME.as_posix(),
        paths.RADAR_RAW.as_posix(),
    ])

    _run_and_check([
        paths.FRAME_RENDER_BIN.as_posix(),
        'radar',
        '--min-lon', config['bbox']['min_lon'],
        '--min-lat', config['bbox']['min_lat'],
        '--max-lon', config['bbox']['max_lon'],
        '--max-lat', config['bbox']['max_lat'],
        '--width', str(radar.WIDTH),
        '--height', str(radar.HEIGHT),
        '--input', paths.RADAR_JSON_BASENAME.as_posix(),
        '--colors', paths.COLORS_FILE.as_posix(),
        '--streets', paths.STREETS_IMG.as_posix(),
        '--output', paths.RADAR_IMG.as_posix(),
    ])
