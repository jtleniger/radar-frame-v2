import io
import time
import requests
from datetime import timedelta
import configparser
import logging
import logging.handlers
import waveshare_epd.interface
import waveshare_epd.noop
import waveshare_epd.epd7in3f
from PIL import Image
from http import HTTPStatus

def setup_logs(config):
    handler = logging.handlers.RotatingFileHandler(config['logging']['file'], maxBytes=100_000, backupCount=4)
    handler.setFormatter(logging.Formatter('{asctime} {levelname} {name} {filename}:{lineno} {message}', style='{'))

    logging.basicConfig(
        handlers=[handler],
        encoding='utf-8',
        level=logging.getLevelName(config['logging']['level']))


def get_config():
    config = configparser.ConfigParser()
    config.read('./config/config.ini')
    return config


def get(url) -> requests.Response:
    response = requests.get(url, timeout=(5, 30))

    if response.status_code == HTTPStatus.OK:
        return response
    
    if response.status_code == HTTPStatus.NOT_MODIFIED:
        return None

    logging.error(f'bad status code from server for {response.url}, code: {response.status_code}')
    return None


def get_frame(config):
    response = get(f"{config['server']['base_url']}frame")

    if response is None:
        return None

    return response.content

def get_epaper(config) -> waveshare_epd.interface.EPaperInterface:
    if config['epaper'].getboolean('use_noop'):
        return waveshare_epd.noop.NoOpDisplay()
    else:
        return waveshare_epd.epd7in3f.EPD()

def main():
    config = get_config()
    setup_logs(config)

    delay = timedelta(minutes=2)

    epaper = get_epaper(config)

    while True:
        logging.info("fetching image...")
        try:
            frame = get_frame(config)
        except Exception as e:
            logging.error(e)
            time.sleep(delay.total_seconds())
            continue
            

        if frame is None:
            logging.info("no new frame image")
            time.sleep(delay.total_seconds())
            continue

        # Write image  
        logging.info('displaying new image')
        epaper.init()
        epaper.Clear()
        image = Image.open(io.BytesIO(frame))
        epaper.display(epaper.getbuffer(image))
        epaper.sleep()

        time.sleep(delay.total_seconds())


if __name__ == '__main__':
    main()
