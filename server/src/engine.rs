//! Image composition and radar rasterization.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use chrono_tz::Tz;

use crate::constants::{RADAR_HEIGHT, WIDTH};
use crate::error::Result;
use crate::paths::Paths;
use crate::settings::Settings;
use crate::sources::Forecast;
use crate::view::font::Fonts;
use crate::view::forecast::Alert;
use crate::view::image::Image;
use crate::view::palette::Palette;
use crate::view::views;

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
    fn render_radar(
        &self,
        volume: &Path,
        minimum: f64,
        output: &Path,
    ) -> Result<crate::render::archive::Summary>;
    fn render_storm(&self, alerts: &[Alert], now: DateTime<Utc>) -> Result<()>;
    fn render_clear(&self, forecast: &Forecast, alerts: &[Alert], now: DateTime<Utc>)
        -> Result<()>;
}

pub struct Engine {
    paths: Paths,
    bounds: Bounds,
    timezone: Tz,
    colors: Option<PathBuf>,
    fonts: Fonts,
}

impl Engine {
    pub fn new(paths: Paths, settings: &Settings) -> Self {
        Engine {
            paths,
            bounds: settings.bounds,
            timezone: settings.timezone,
            colors: settings.colors.clone(),
            fonts: Fonts::new(),
        }
    }
}

impl Renderer for Engine {
    /// Rasterize the local Archive II volume to `output`.
    /// `components/radar/render.py` ran `frame-render frame` for this.
    fn render_radar(
        &self,
        volume: &Path,
        minimum: f64,
        output: &Path,
    ) -> Result<crate::render::archive::Summary> {
        let streets = self.paths.streets_img();
        Ok(crate::render::archive::render_archive(
            &crate::render::archive::Options {
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
                colors: self.colors.clone(),
                streets: streets.is_file().then_some(streets),
                output: output.to_path_buf(),
            },
        )?)
    }

    /// Radar frame plus info bar, dithered to the palette.
    fn render_storm(&self, alerts: &[Alert], now: DateTime<Utc>) -> Result<()> {
        let radar = Image::from_png(&self.paths.radar_img())?;
        let palette = Palette::from_png(&self.paths.palette_img())?;
        let image = views::render_storm(&self.fonts, &radar, alerts, now, self.timezone, &palette)?;
        image.write_png(&self.paths.output_img())?;
        Ok(())
    }

    /// Current, hourly, and daily panels plus the info bar.
    fn render_clear(
        &self,
        forecast: &Forecast,
        alerts: &[Alert],
        now: DateTime<Utc>,
    ) -> Result<()> {
        let palette = Palette::from_png(&self.paths.palette_img())?;
        let image =
            views::render_clear(&self.fonts, forecast, alerts, now, self.timezone, &palette)?;
        image.write_png(&self.paths.output_img())?;
        Ok(())
    }
}
