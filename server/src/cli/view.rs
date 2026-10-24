//! `radar-frame view`: compose e-ink frames from JSON on stdin.

use std::io::Read;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use clap::{Args, Subcommand};

use crate::error::Result;

#[derive(Subcommand)]
pub enum ViewCommand {
    /// Compose the storm frame from a radar frame and active alerts
    Storm(StormArgs),
    /// Compose the clear-sky frame from current, hourly, and daily forecasts
    Clear(ClearArgs),
    /// Write the seven-colour e-ink palette PNG
    Palette(PaletteArgs),
}

#[derive(Args)]
pub(super) struct FrameArgs {
    /// Palette PNG written by `radar-frame view palette`
    #[arg(long)]
    palette: PathBuf,
    #[arg(long)]
    output: PathBuf,
    /// IANA time zone used for the info bar
    #[arg(long)]
    timezone: String,
    /// Override the current time (RFC 3339), for tests and fixtures
    #[arg(long)]
    now: Option<String>,
}

#[derive(Args)]
pub(super) struct StormArgs {
    /// Radar frame PNG (800x444)
    #[arg(long)]
    radar: PathBuf,
    #[command(flatten)]
    frame: FrameArgs,
}

#[derive(Args)]
pub(super) struct ClearArgs {
    #[command(flatten)]
    frame: FrameArgs,
}

#[derive(Args)]
pub(super) struct PaletteArgs {
    #[arg(long)]
    output: PathBuf,
}

pub fn run(command: ViewCommand) -> Result<()> {
    match command {
        ViewCommand::Storm(args) => view_storm(args),
        ViewCommand::Clear(args) => view_clear(args),
        ViewCommand::Palette(args) => view_palette(args),
    }
}

fn now(value: Option<String>) -> Result<DateTime<Utc>> {
    match value {
        Some(value) => DateTime::parse_from_rfc3339(&value)
            .map(|time| time.with_timezone(&Utc))
            .map_err(|e| crate::error::Error::config(format!("{value}: {e}"))),
        None => Ok(Utc::now()),
    }
}

fn timezone(value: &str) -> Result<Tz> {
    value
        .parse()
        .map_err(|_| crate::error::Error::config(format!("unknown time zone {value}")))
}

fn stdin_json<T: serde::de::DeserializeOwned>() -> Result<T> {
    let mut input = String::new();
    std::io::stdin()
        .read_to_string(&mut input)
        .map_err(crate::error::Error::from)?;
    serde_json::from_str(&input).map_err(crate::error::Error::from)
}

fn view_storm(args: StormArgs) -> Result<()> {
    let alerts: Vec<crate::view::forecast::Alert> = stdin_json()?;
    let radar = crate::view::image::Image::from_png(&args.radar)?;
    let palette = crate::view::palette::Palette::from_png(&args.frame.palette)?;
    let image = crate::view::views::render_storm(
        &crate::view::font::Fonts::new(),
        &radar,
        &alerts,
        now(args.frame.now)?,
        timezone(&args.frame.timezone)?,
        &palette,
    )?;
    image.write_png(&args.frame.output)?;
    Ok(())
}

fn view_clear(args: ClearArgs) -> Result<()> {
    let input: crate::view::forecast::ClearInput = stdin_json()?;
    let palette = crate::view::palette::Palette::from_png(&args.frame.palette)?;
    let image = crate::view::views::render_clear(
        &crate::view::font::Fonts::new(),
        &input.forecast,
        &input.alerts,
        now(args.frame.now)?,
        timezone(&args.frame.timezone)?,
        &palette,
    )?;
    image.write_png(&args.frame.output)?;
    Ok(())
}

fn view_palette(args: PaletteArgs) -> Result<()> {
    crate::view::palette::write_eink_png(&args.output)?;
    Ok(())
}
