use std::io::Read;
use std::path::PathBuf;
use std::process::ExitCode;

use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use clap::{Args, Parser, Subcommand};

use frame_view::error::{Error, Result};
use frame_view::font::Fonts;
use frame_view::forecast::{Alert, ClearInput};
use frame_view::image::Image;
use frame_view::palette::{self, Palette};
use frame_view::views;

#[derive(Parser)]
#[command(name = "frame-view", version, about = "Compose the e-ink frames")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Compose the storm frame from a radar frame and active alerts
    Storm(StormArgs),
    /// Compose the clear-sky frame from current, hourly, and daily forecasts
    Clear(ClearArgs),
    /// Write the seven-colour e-ink palette PNG
    Palette(PaletteArgs),
}

#[derive(Args)]
struct FrameArgs {
    /// Palette PNG written by `frame-view palette`
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
struct StormArgs {
    /// Radar frame PNG (800x444)
    #[arg(long)]
    radar: PathBuf,
    #[command(flatten)]
    frame: FrameArgs,
}

#[derive(Args)]
struct ClearArgs {
    #[command(flatten)]
    frame: FrameArgs,
}

#[derive(Args)]
struct PaletteArgs {
    #[arg(long)]
    output: PathBuf,
}

fn now(value: Option<String>) -> Result<DateTime<Utc>> {
    match value {
        Some(value) => DateTime::parse_from_rfc3339(&value)
            .map(|time| time.with_timezone(&Utc))
            .map_err(|e| Error::Time(format!("{value}: {e}"))),
        None => Ok(Utc::now()),
    }
}

fn timezone(value: &str) -> Result<Tz> {
    value
        .parse()
        .map_err(|_| Error::Timezone(format!("unknown time zone {value}")))
}

fn stdin_json<T: serde::de::DeserializeOwned>() -> Result<T> {
    let mut input = String::new();
    std::io::stdin()
        .read_to_string(&mut input)
        .map_err(Error::Io)?;
    serde_json::from_str(&input).map_err(Error::Json)
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    let fonts = Fonts::new();

    match cli.command {
        Command::Storm(args) => {
            let alerts: Vec<Alert> = stdin_json()?;
            let radar = Image::from_png(&args.radar)?;
            let palette = Palette::from_png(&args.frame.palette)?;
            let image = views::render_storm(
                &fonts,
                &radar,
                &alerts,
                now(args.frame.now)?,
                timezone(&args.frame.timezone)?,
                &palette,
            )?;
            image.write_png(&args.frame.output)?;
        }
        Command::Clear(args) => {
            let input: ClearInput = stdin_json()?;
            let palette = Palette::from_png(&args.frame.palette)?;
            let image = views::render_clear(
                &fonts,
                &input,
                now(args.frame.now)?,
                timezone(&args.frame.timezone)?,
                &palette,
            )?;
            image.write_png(&args.frame.output)?;
        }
        Command::Palette(args) => palette::write_eink_png(&args.output)?,
    }

    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("frame-view: {error}");
            ExitCode::FAILURE
        }
    }
}
