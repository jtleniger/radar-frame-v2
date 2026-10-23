use chrono::{DateTime, Utc};
use chrono_tz::Tz;

use crate::view::colors::{BLACK, WHITE};
use crate::view::components::{
    current_conditions, daily_forecast, hourly_forecast, HEIGHT, HEIGHT_LESS_INFO, INFO_HEIGHT,
    WIDTH, WIDTH_1_3,
};
use crate::view::error::Result;
use crate::view::font::Fonts;
use crate::view::forecast::{Alert, ClearInput};
use crate::view::image::Image;
use crate::view::info;
use crate::view::palette::Palette;

/// Radar frame with the info bar, dithered onto the e-ink palette.
pub fn render_storm(
    fonts: &Fonts,
    radar: &Image,
    alerts: &[Alert],
    now: DateTime<Utc>,
    timezone: Tz,
    palette: &Palette,
) -> Result<Image> {
    let mut image = Image::new(WIDTH, HEIGHT, WHITE);
    image.paste(radar, 0, 0);

    let bar = info::render(fonts, alerts, now, timezone);
    image.paste(&bar, 0, (HEIGHT - INFO_HEIGHT) as i64);

    palette.quantize(&mut image, true);
    Ok(image)
}

/// Current conditions, hourly and daily forecasts side by side, with the info
/// bar. The forecast panels are mapped to the palette without dithering.
pub fn render_clear(
    fonts: &Fonts,
    input: &ClearInput,
    now: DateTime<Utc>,
    timezone: Tz,
    palette: &Palette,
) -> Result<Image> {
    let mut image = Image::new(WIDTH, HEIGHT, WHITE);

    let current = current_conditions(fonts, &input.current)?;
    image.paste(&current, 0, 0);

    let hourly = hourly_forecast(fonts, &input.hourly)?;
    image.paste(&hourly, WIDTH_1_3 as i64, 0);

    let daily = daily_forecast(fonts, &input.daily)?;
    image.paste(&daily, (2 * WIDTH_1_3) as i64, 0);

    let bar = info::render(fonts, &input.alerts, now, timezone);
    image.paste(&bar, 0, HEIGHT_LESS_INFO as i64);

    image.vertical_line(WIDTH_1_3 as i64, 20, HEIGHT_LESS_INFO as i64 - 20, 2, BLACK);
    image.vertical_line(
        (2 * WIDTH_1_3) as i64,
        20,
        HEIGHT_LESS_INFO as i64 - 20,
        2,
        BLACK,
    );

    palette.quantize(&mut image, false);
    Ok(image)
}
