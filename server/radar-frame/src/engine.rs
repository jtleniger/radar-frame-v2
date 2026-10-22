//! Image composition and radar rasterization, calling the `frame-render` and
//! `frame-view` libraries directly instead of spawning their binaries.

use std::path::Path;

use chrono::{DateTime, Utc};
use chrono_tz::Tz;

use crate::config::Config;
use crate::constants::{RADAR_HEIGHT, WIDTH};
use crate::error::Result;
use crate::paths::Paths;
use crate::sources::Forecast;
use frame_view::font::Fonts;
use frame_view::forecast::{Alert, ClearInput, CurrentConditions, ForecastDay, ForecastHour};
use frame_view::image::Image;
use frame_view::palette::Palette;
use frame_view::views;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bounds {
    pub min_lon: f64,
    pub min_lat: f64,
    pub max_lon: f64,
    pub max_lat: f64,
}

/// Rendering is a trait so the HTTP layer can be tested without a full
/// three-elevation volume; production always uses [`Engine`].
pub trait Renderer: Send + Sync {
    fn render_radar(&self, volume: &Path, minimum: f64) -> Result<()>;
    fn render_storm(&self, alerts: &[Alert], now: DateTime<Utc>) -> Result<()>;
    fn render_clear(
        &self,
        forecast: &Forecast,
        alerts: Vec<Alert>,
        now: DateTime<Utc>,
    ) -> Result<()>;
}

pub struct Engine {
    paths: Paths,
    bounds: Bounds,
    timezone: Tz,
    fonts: Fonts,
}

impl Engine {
    pub fn new(paths: Paths, config: &Config) -> Result<Self> {
        Ok(Engine {
            paths,
            bounds: Bounds {
                min_lon: config.f64("bbox", "min_lon")?,
                min_lat: config.f64("bbox", "min_lat")?,
                max_lon: config.f64("bbox", "max_lon")?,
                max_lat: config.f64("bbox", "max_lat")?,
            },
            timezone: config.timezone("forecast", "timezone")?,
            fonts: Fonts::new(),
        })
    }

    pub fn bounds(&self) -> Bounds {
        self.bounds
    }

    /// Rasterize the local Archive II volume to `data/nexrad-latest.png`.
    /// `components/radar/render.py` ran `frame-render frame` for this.
    pub fn render_radar(&self, volume: &Path, minimum: f64) -> Result<()> {
        frame_render::archive::render_archive(&frame_render::archive::Options {
            min_lon: self.bounds.min_lon,
            min_lat: self.bounds.min_lat,
            max_lon: self.bounds.max_lon,
            max_lat: self.bounds.max_lat,
            width: WIDTH,
            height: RADAR_HEIGHT,
            archive: volume.to_path_buf(),
            elevations: vec![1, 2, 3],
            minimum: Some(minimum as f32),
            maximum: None,
            colors: self.paths.colors(),
            streets: self.paths.streets_img(),
            output: self.paths.radar_img(),
        })?;
        Ok(())
    }

    /// Radar frame plus info bar, dithered to the palette.
    pub fn render_storm(&self, alerts: &[Alert], now: DateTime<Utc>) -> Result<()> {
        let radar = Image::from_png(&self.paths.radar_img())?;
        let palette = Palette::from_png(&self.paths.palette_img())?;
        let image = views::render_storm(&self.fonts, &radar, alerts, now, self.timezone, &palette)?;
        image.write_png(&self.paths.output_img())?;
        Ok(())
    }

    /// Current, hourly, and daily panels plus the info bar.
    pub fn render_clear(
        &self,
        forecast: &Forecast,
        alerts: Vec<Alert>,
        now: DateTime<Utc>,
    ) -> Result<()> {
        let palette = Palette::from_png(&self.paths.palette_img())?;

        // The panels copy the scalar fields; the view owns its input.
        let input = ClearInput {
            current: CurrentConditions {
                temp_f: forecast.current.temp_f,
                code: forecast.current.code,
                is_day: forecast.current.is_day,
            },
            hourly: forecast
                .hourly
                .iter()
                .map(|hour| ForecastHour {
                    hour: hour.hour.clone(),
                    temp_f: hour.temp_f,
                    code: hour.code,
                })
                .collect(),
            daily: forecast
                .daily
                .iter()
                .map(|day| ForecastDay {
                    day: day.day.clone(),
                    high_f: day.high_f,
                    low_f: day.low_f,
                    code: day.code,
                })
                .collect(),
            alerts,
        };

        let image = views::render_clear(&self.fonts, &input, now, self.timezone, &palette)?;
        image.write_png(&self.paths.output_img())?;
        Ok(())
    }
}

impl Renderer for Engine {
    fn render_radar(&self, volume: &Path, minimum: f64) -> Result<()> {
        Engine::render_radar(self, volume, minimum)
    }

    fn render_storm(&self, alerts: &[Alert], now: DateTime<Utc>) -> Result<()> {
        Engine::render_storm(self, alerts, now)
    }

    fn render_clear(
        &self,
        forecast: &Forecast,
        alerts: Vec<Alert>,
        now: DateTime<Utc>,
    ) -> Result<()> {
        Engine::render_clear(self, forecast, alerts, now)
    }
}
