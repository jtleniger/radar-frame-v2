import logging
import subprocess

from constants import paths

_logger = logging.getLogger(__name__)


def create():
    """Write the seven-colour e-ink palette PNG."""
    cmd = [paths.FRAME_VIEW_BIN.as_posix(), 'palette', '--output', paths.PALETTE_IMG.as_posix()]

    _logger.info('command: %s', ' '.join(cmd))

    try:
        subprocess.run(cmd, check=True, capture_output=True, text=True)
    except subprocess.CalledProcessError as e:
        _logger.error('Command failed with exit code %d', e.returncode)
        _logger.error('STDOUT: %s', e.stdout)
        _logger.error('STDERR: %s', e.stderr)
        raise
