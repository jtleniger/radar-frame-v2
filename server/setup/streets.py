import logging
import subprocess

from config.config import Config
from sources import osm
from constants import paths, radar

_logger = logging.getLogger(__name__)

_BBOX_KEYS = ('min_lat', 'max_lat', 'min_lon', 'max_lon')


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


def _bbox_stamp():
    config = Config.instance()
    return ''.join(f'{key}={config["bbox"][key]}\n' for key in _BBOX_KEYS)


def _read_stamp(path):
    try:
        return path.read_text()
    except FileNotFoundError:
        return None


def _write_stamp(path, stamp):
    path.write_text(stamp)


def create(force: bool = False):
    stamp = _bbox_stamp()
    mask_stamp = _read_stamp(paths.STREETS_BBOX)
    data_stamp = _read_stamp(paths.STREETS_OSM_BBOX)

    if paths.STREETS_IMG.exists() and not force:
        if mask_stamp == stamp:
            _logger.info(f'{paths.STREETS_IMG} is current for [bbox], doing nothing')
            return

        if mask_stamp is None:
            # Masks generated before the bbox stamp existed: adopt them rather
            # than refetching on every upgrade. --force-streets re-downloads.
            _logger.warning(
                f'{paths.STREETS_IMG} has no bbox stamp, assuming it matches the '
                'current [bbox]; use --force-streets if the bbox has changed')
            _write_stamp(paths.STREETS_BBOX, stamp)
            return

        _logger.info(f'{paths.STREETS_IMG} was generated for a different [bbox], regenerating')

    if (paths.STREETS_DATA.exists() and data_stamp is None
            and not paths.STREETS_IMG.exists()):
        # First run with a hand-written streets.osm: use it for the current
        # bbox instead of overwriting it.
        _logger.warning(
            f'{paths.STREETS_DATA} has no bbox stamp, assuming it matches the current [bbox]')
        _write_stamp(paths.STREETS_OSM_BBOX, stamp)
        data_stamp = stamp

    if force or not paths.STREETS_DATA.exists() or data_stamp != stamp:
        _logger.info(f'{paths.STREETS_DATA} missing or generated for a different [bbox], downloading')
        osm.get()
        _write_stamp(paths.STREETS_OSM_BBOX, stamp)

    generate_png()
    _write_stamp(paths.STREETS_BBOX, stamp)
