use crate::view::codes;
use crate::view::colors::{BLACK, WHITE};
use crate::view::error::Result;
use crate::view::font::{Anchor, Fonts, Size};
use crate::view::forecast::{CurrentConditions, ForecastDay, ForecastHour};
use crate::view::icons;
use crate::view::image::Image;

pub const WIDTH: u32 = 800;
pub const HEIGHT: u32 = 480;
pub const WIDTH_1_3: u32 = WIDTH / 3;
pub const INFO_HEIGHT: u32 = 36;
pub const HEIGHT_LESS_INFO: u32 = HEIGHT - INFO_HEIGHT;
pub const HEIGHT_LESS_INFO_1_3: u32 = HEIGHT_LESS_INFO / 3;

/// Python's `round()` (ties to even), which these views use for every
/// displayed integer.
pub fn round_int(value: f64) -> i64 {
    let floor = value.floor();
    if value - floor == 0.5 {
        let floor = floor as i64;
        if floor % 2 == 0 {
            floor
        } else {
            floor + 1
        }
    } else {
        value.round() as i64
    }
}

pub fn current_conditions(fonts: &Fonts, current: &CurrentConditions) -> Result<Image> {
    let mut image = Image::new(WIDTH_1_3, HEIGHT_LESS_INFO, WHITE);

    let center = WIDTH_1_3 as f32 / 2.0 + 6.0;
    fonts.draw_text(
        &mut image,
        (center, 40.0),
        Size::XLarge,
        &round_int(current.temp_f).to_string(),
        Anchor::MiddleTop,
        BLACK,
    );

    let icon = icons::render_icon(codes::code_to_icon(current.code, !current.is_day), 172)?;
    let x = (WIDTH_1_3 / 2) as i64 - (icon.width / 2) as i64;
    image.paste_masked(&icon, x, 40 + HEIGHT_LESS_INFO_1_3 as i64 - 36);

    fonts.draw_text(
        &mut image,
        (center, 40.0 + 2.0 * HEIGHT_LESS_INFO_1_3 as f32 + 24.0),
        Size::Large,
        &codes::code_to_string(current.code),
        Anchor::MiddleTop,
        BLACK,
    );

    Ok(image)
}

fn forecast_day(fonts: &Fonts, day: &ForecastDay) -> Result<Image> {
    let mut image = Image::new(WIDTH_1_3, HEIGHT_LESS_INFO_1_3, WHITE);

    fonts.draw_text(
        &mut image,
        (16.0, 0.0),
        Size::Large,
        &day.day,
        Anchor::LeftAscender,
        BLACK,
    );

    let icon = icons::render_icon(codes::code_to_icon(day.code, false), 86)?;
    image.paste_masked(&icon, 164, -6);

    let text = format!("{:<3} {:>3}", round_int(day.high_f), round_int(day.low_f));
    fonts.draw_text(
        &mut image,
        (16.0, 64.0),
        Size::Large,
        &text,
        Anchor::LeftAscender,
        BLACK,
    );

    Ok(image)
}

pub fn daily_forecast(fonts: &Fonts, days: &[ForecastDay]) -> Result<Image> {
    let mut image = Image::new(WIDTH_1_3, HEIGHT_LESS_INFO, WHITE);

    let mut y = 12;
    for day in days.iter().take(3) {
        let day = forecast_day(fonts, day)?;
        image.paste(&day, 0, y);
        y += day.height as i64;
    }

    Ok(image)
}

fn forecast_hour(fonts: &Fonts, hour: &ForecastHour) -> Result<Image> {
    let mut image = Image::new(WIDTH_1_3, HEIGHT_LESS_INFO_1_3, WHITE);

    fonts.draw_text(
        &mut image,
        (16.0, 12.0),
        Size::Large,
        &hour.hour,
        Anchor::LeftAscender,
        BLACK,
    );

    let icon = icons::render_icon(codes::code_to_icon(hour.code, false), 86)?;
    image.paste_masked(&icon, 164, 6);

    fonts.draw_text(
        &mut image,
        (16.0, 76.0),
        Size::Large,
        &round_int(hour.temp_f).to_string(),
        Anchor::LeftAscender,
        BLACK,
    );

    Ok(image)
}

pub fn hourly_forecast(fonts: &Fonts, hours: &[ForecastHour]) -> Result<Image> {
    let mut image = Image::new(WIDTH_1_3, HEIGHT_LESS_INFO, WHITE);

    let mut y = 0;
    for hour in hours.iter().step_by(2).take(3) {
        let hour = forecast_hour(fonts, hour)?;
        image.paste(&hour, 0, y);
        y += hour.height as i64;
    }

    Ok(image)
}

#[cfg(test)]
mod tests {
    use super::round_int;

    #[test]
    fn rounds_like_python() {
        assert_eq!(round_int(38.8), 39);
        assert_eq!(round_int(12.2), 12);
        assert_eq!(round_int(-25.0), -25);
        assert_eq!(round_int(2.5), 2);
        assert_eq!(round_int(3.5), 4);
        assert_eq!(round_int(-2.5), -2);
        assert_eq!(round_int(-3.5), -4);
    }
}
