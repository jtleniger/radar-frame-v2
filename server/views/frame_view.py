import json
import logging
import subprocess

from config.config import Config
from constants import paths

_logger = logging.getLogger(__name__)


def alert(alert):
    return {
        'event': alert.event,
        'level': alert.level.name,
        'status': alert.status,
        'effective': alert.effective.isoformat(),
        'expires': alert.expires.isoformat(),
    }


def run(command: str, payload, output, radar=None):
    """Run the Rust frame compositor, feeding it the forecast data as JSON."""
    config = Config.instance()

    cmd = [paths.FRAME_VIEW_BIN.as_posix(), command]
    if radar is not None:
        cmd += ['--radar', radar]
    cmd += [
        '--palette', paths.PALETTE_IMG.as_posix(),
        '--output', output.as_posix(),
        '--timezone', config['forecast']['timezone'],
    ]

    _logger.info('command: %s', ' '.join(cmd))

    result = subprocess.run(
        cmd, input=json.dumps(payload), text=True, capture_output=True)

    if result.returncode != 0:
        _logger.error('Command failed with exit code %d', result.returncode)
        _logger.error('STDOUT: %s', result.stdout)
        _logger.error('STDERR: %s', result.stderr)
        raise subprocess.CalledProcessError(result.returncode, cmd)
