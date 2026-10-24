//! `radar-frame render`: local frame rendering for inspection and fixtures.

use std::path::PathBuf;
use std::sync::Arc;

use clap::{Args, Subcommand};

use crate::config::Settings;
use crate::error::Result;
use crate::service::clock::{Clock, SystemClock};
use crate::service::engine::{Engine, Renderer};
use crate::service::setup;
use crate::sources::osm::Overpass;

use super::{context, http_client, init_logging};

#[derive(Subcommand)]
pub enum RenderCommand {
    /// Render the radar and e-ink frames from a local Archive II volume
    Volume(RenderVolume),
    /// Render a radar frame straight from an Archive II volume
    Archive(ArchiveArgs),
    /// Rasterize OSM highway ways into an 8-bit street mask PNG
    Streets(StreetsArgs),
}

#[derive(Args)]
pub(super) struct RenderVolume {
    /// Archive II volume, plain or gzip-compressed
    #[arg(long)]
    file: PathBuf,
    /// Minimum product value to render
    #[arg(long, default_value_t = 10.0)]
    minimum: f64,
    /// Re-download and re-rasterize the street mask
    #[arg(long)]
    force_streets: bool,
}

#[derive(Args)]
pub(super) struct ArchiveArgs {
    #[arg(long, allow_hyphen_values = true)]
    min_lon: f64,
    #[arg(long, allow_hyphen_values = true)]
    min_lat: f64,
    #[arg(long, allow_hyphen_values = true)]
    max_lon: f64,
    #[arg(long, allow_hyphen_values = true)]
    max_lat: f64,
    #[arg(long)]
    width: u32,
    #[arg(long)]
    height: u32,
    /// NEXRAD Archive II volume, plain or gzip-compressed
    #[arg(long)]
    archive: PathBuf,
    /// Minimum product value to include
    #[arg(long, allow_hyphen_values = true)]
    minimum: Option<f32>,
    /// Maximum product value to include
    #[arg(long, allow_hyphen_values = true)]
    maximum: Option<f32>,
    /// Elevation or range of elevations: N or N-M (inclusive)
    #[arg(short = 'e', long, default_value = "1-3")]
    elevations: String,
    /// Optional text color table; defaults to the embedded gdal-colors table
    #[arg(long)]
    colors: Option<PathBuf>,
    /// 8-bit grayscale street mask, same dimensions as the frame
    #[arg(long)]
    streets: PathBuf,
    #[arg(long)]
    output: PathBuf,
}

#[derive(Args)]
pub(super) struct StreetsArgs {
    #[arg(long, allow_hyphen_values = true)]
    min_lon: f64,
    #[arg(long, allow_hyphen_values = true)]
    min_lat: f64,
    #[arg(long, allow_hyphen_values = true)]
    max_lon: f64,
    #[arg(long, allow_hyphen_values = true)]
    max_lat: f64,
    #[arg(long)]
    width: u32,
    #[arg(long)]
    height: u32,
    /// Overpass OSM XML file
    #[arg(long)]
    input: PathBuf,
    /// Output grayscale PNG path
    #[arg(long)]
    output: PathBuf,
}

pub async fn run(command: RenderCommand) -> Result<()> {
    match command {
        RenderCommand::Volume(args) => render_volume(args).await,
        RenderCommand::Archive(args) => render_archive(args),
        RenderCommand::Streets(args) => render_streets(args),
    }
}

async fn render_volume(args: RenderVolume) -> Result<()> {
    let (paths, config) = context()?;
    init_logging(&paths, &config);

    if !args.file.is_file() {
        return Err(crate::error::Error::io(format!(
            "{}: no such file",
            args.file.display()
        )));
    }

    setup::create_data_dir(&paths)?;
    setup::create_palette(&paths)?;

    let client = http_client(&config)?;
    if let Err(error) =
        setup::create_streets(&paths, &config, &Overpass::new(client), args.force_streets).await
    {
        tracing::warn!(
            event = "streets_unavailable",
            error = %error,
            "rendering without the street overlay"
        );
    }

    let settings = Settings::from_config(&config)?;
    let engine = Arc::new(Engine::new(paths.clone(), &settings));
    let volume = args.file.clone();
    let radar_output = paths.radar_img();
    let radar_engine = Arc::clone(&engine);
    tokio::task::spawn_blocking(move || {
        radar_engine.render_radar(&volume, args.minimum, &radar_output)
    })
    .await
    .map_err(|e| crate::error::Error::render(format!("render task: {e}")))??;

    let now = SystemClock.now();
    tokio::task::spawn_blocking(move || engine.render_storm(&[], now))
        .await
        .map_err(|e| crate::error::Error::render(format!("render task: {e}")))??;

    println!("radar frame: {}", paths.radar_img().display());
    println!("e-ink frame: {}", paths.output_img().display());
    Ok(())
}

fn render_archive(args: ArchiveArgs) -> Result<()> {
    let elevations =
        crate::nexrad::elevations::parse_elevations(&args.elevations).ok_or_else(|| {
            crate::error::Error::config(format!("invalid elevations {}", args.elevations))
        })?;

    crate::render::archive::render_archive(&crate::render::archive::Options {
        min_lon: args.min_lon,
        min_lat: args.min_lat,
        max_lon: args.max_lon,
        max_lat: args.max_lat,
        width: args.width as usize,
        height: args.height as usize,
        archive: args.archive,
        elevations,
        minimum: args.minimum,
        maximum: args.maximum,
        colors: args.colors,
        streets: Some(args.streets),
        output: args.output,
    })?;
    Ok(())
}

fn render_streets(args: StreetsArgs) -> Result<()> {
    crate::render::streets::render_streets(&crate::render::streets::Options {
        min_lon: args.min_lon,
        min_lat: args.min_lat,
        max_lon: args.max_lon,
        max_lat: args.max_lat,
        width: args.width as usize,
        height: args.height as usize,
        input: args.input,
        output: args.output,
    })?;
    Ok(())
}
