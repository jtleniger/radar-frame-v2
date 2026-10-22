import os
from pathlib import Path

DATA_DIR = Path(os.getcwd()) / 'data'

STREETS_DATA = DATA_DIR / 'streets.osm'
STREETS_TIF = DATA_DIR / 'streets.tif'
STREETS_IMG = DATA_DIR / 'streets.png'

PALETTE_IMG = DATA_DIR / 'palette.png'

RADAR_IMG = DATA_DIR / 'nexrad-latest.png'
RADAR_RAW = DATA_DIR / 'nexrad-latest.ar2'
RADAR_JSON_BASENAME = DATA_DIR / 'nexrad-latest'

OUTPUT_IMG = DATA_DIR / 'frame.png'


BIN_DIR = Path(os.getcwd()) / 'bin'

NEXRAD_JSON_BIN = BIN_DIR / 'nexrad-geojson'
FRAME_RENDER_BIN = BIN_DIR / 'frame-render'

COLORS_FILE = Path(os.getcwd()) / 'gdal-colors.txt'