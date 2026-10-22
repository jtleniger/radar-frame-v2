import argparse
import logging
import logging.handlers
import os
from pathlib import Path

from constants import paths

_logger = logging.getLogger(__name__)


def provider(mock: bool):
    from sources.mock_forecast import MockForecast
    from sources.online_forecast import OnlineForecast

    if not mock:
        return OnlineForecast()

    return MockForecast()


def render_clear(args):
    """render the clear-sky frame."""
    from modes.mode import Mode
    from state.state import State

    Mode.Clear.run(State.instance(), provider(args.mock))


def render_storm(args):
    """render the storm frame from the latest NEXRAD volume."""
    from modes.mode import Mode
    from state.state import State

    Mode.Storm.run(State.instance(), provider(args.mock))


def render_volume(args):
    """render the frames from a local Archive II volume."""
    from components.radar.render import render
    from setup import palette, streets
    from views import storm

    volume = Path(args.file).expanduser()
    if not volume.is_file():
        raise SystemExit(f'{volume}: no such file')

    create_data_dir()
    palette.create()
    streets.create(force=args.force_streets)

    render(volume, minimum=args.minimum)
    storm.render([])

    print(f'radar frame: {paths.RADAR_IMG}')
    print(f'e-ink frame: {paths.OUTPUT_IMG}')


def setup(args):
    """create the data directory, palette, and street mask."""
    prepare(args.force_streets)


def prepare(force_streets: bool = False):
    from setup import palette, streets

    create_data_dir()
    palette.create()
    streets.create(force=force_streets)


def create_data_dir():
    if not paths.DATA_DIR.is_dir():
        _logger.info(f'{paths.DATA_DIR} missing, creating it')
        os.mkdir(paths.DATA_DIR)
    else:
        _logger.info(f'{paths.DATA_DIR} exists')


def run_server(_):
    """run the HTTP server."""
    from server import server

    prepare()
    s = server.create()
    s.run()


def setup_logs():
    from config.config import Config

    config = Config.instance()
    handler = logging.handlers.RotatingFileHandler('radar-frame.log',
                                                   maxBytes=100_000,
                                                   backupCount=4)

    handler.setFormatter(
        logging.Formatter('{asctime} {levelname} {name} {filename}:{lineno} {message}',
        style='{'))

    logging.basicConfig(
        handlers=[handler],
        level=logging.getLevelName(config['logging']['level']))


def create_server():
    """Helper to create a server to be used by Gunicorn."""
    from server import server

    setup_logs()
    prepare()
    return server.create()


COMMANDS = {
    'render-volume': render_volume,
    'setup': setup,
    'render-clear': render_clear,
    'render-storm': render_storm,
    'run-server': run_server,
}


def main():
    epilog = 'commands:\n'
    epilog += '\n'.join([f"  {c[0] + ':':<20}{c[1].__doc__}" for c in COMMANDS.items()])
    parser = argparse.ArgumentParser(
        usage='%(prog)s [COMMAND] [-d]',
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=epilog
    )

    parser.add_argument('command', choices=COMMANDS.keys(),
                        help=f'one of {", ".join(COMMANDS.keys())}')
    parser.add_argument('--mock', action='store_true', help='use mock forecast data')
    parser.add_argument('--file', help='Archive II volume to render (render-volume)')
    parser.add_argument('--minimum', type=float, default=10.0,
                        help='minimum product value to render (render-volume; default: 10)')
    parser.add_argument('--force-streets', action='store_true',
                        help='re-download and re-rasterize the street mask (setup, render-volume)')

    args = parser.parse_args()

    if args.command == 'render-volume' and not args.file:
        parser.error('render-volume requires --file <path.ar2>')

    setup_logs()

    COMMANDS[args.command](args)


if __name__ == "__main__":
    main()
