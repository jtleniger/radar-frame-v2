import logging
import os.path
import subprocess

from config.config import Config
from sources import osm
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


def generate_png():
    config = Config.instance()

    _run_and_check([
        paths.FRAME_RENDER_BIN.as_posix(),
        'streets',
        '--min-lon', config['bbox']['min_lon'],
        '--min-lat', config['bbox']['min_lat'],
        '--max-lon', config['bbox']['max_lon'],
        '--max-lat', config['bbox']['max_lat'],
        '--width', str(radar.WIDTH),
        '--height', str(radar.HEIGHT),
        '--input', paths.STREETS_DATA.as_posix(),
        '--output', paths.STREETS_IMG.as_posix(),
    ])


def create():
    img_exists = paths.STREETS_IMG.exists()
    data_exists = paths.STREETS_DATA.exists()

    if img_exists:
        _logger.info(f'{paths.STREETS_IMG} already exists, doing nothing')
        return

    if data_exists and not img_exists:
        _logger.info(f'{paths.STREETS_DATA} exists, generating png')
        generate_png()

    if not data_exists:
        _logger.info(f'{paths.STREETS_DATA} missing, downloading and generating png')
        osm.get()
        generate_png()
