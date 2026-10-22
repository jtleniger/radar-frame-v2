use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};

use frame_render::archive::{render_archive, Options as ArchiveOptions};
use frame_render::render::{render_radar, Options as RadarOptions};
use frame_render::streets::{render_streets, Options as StreetsOptions};
use nexrad_geojson::elevations::parse_elevations;

#[derive(Parser)]
#[command(name = "frame-render", version, about = "Render the radar frame PNG")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Rasterize elevations 1-3, colorize, and overlay the street mask
    Radar(RadarArgs),
    /// Render the frame straight from an Archive II volume, without GeoJSON
    Frame(FrameArgs),
    /// Rasterize OSM highway ways into an 8-bit street mask PNG
    Streets(StreetsArgs),
}

#[derive(Args)]
struct RadarArgs {
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
    /// Base path of {input}-REF-{1,2,3}.json
    #[arg(long)]
    input: PathBuf,
    /// Text color table, e.g. gdal-colors.txt
    #[arg(long)]
    colors: PathBuf,
    /// 8-bit grayscale street mask, same dimensions as the frame
    #[arg(long)]
    streets: PathBuf,
    #[arg(long)]
    output: PathBuf,
}

#[derive(Args)]
struct FrameArgs {
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
    /// Text color table, e.g. gdal-colors.txt
    #[arg(long)]
    colors: PathBuf,
    /// 8-bit grayscale street mask, same dimensions as the frame
    #[arg(long)]
    streets: PathBuf,
    #[arg(long)]
    output: PathBuf,
}

#[derive(Args)]
struct StreetsArgs {
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

fn main() -> ExitCode {
    let cli = Cli::parse();

    let result = match cli.command {
        Command::Radar(args) => render_radar(&RadarOptions {
            min_lon: args.min_lon,
            min_lat: args.min_lat,
            max_lon: args.max_lon,
            max_lat: args.max_lat,
            width: args.width as usize,
            height: args.height as usize,
            input: args.input,
            colors: args.colors,
            streets: args.streets,
            output: args.output,
        }),
        Command::Frame(args) => match parse_elevations(&args.elevations) {
            Some(elevations) => render_archive(&ArchiveOptions {
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
                streets: args.streets,
                output: args.output,
            }),
            None => Err(frame_render::Error(format!(
                "invalid elevations {}",
                args.elevations
            ))),
        },
        Command::Streets(args) => render_streets(&StreetsOptions {
            min_lon: args.min_lon,
            min_lat: args.min_lat,
            max_lon: args.max_lon,
            max_lat: args.max_lat,
            width: args.width as usize,
            height: args.height as usize,
            input: args.input,
            output: args.output,
        }),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("frame-render: {e}");
            ExitCode::FAILURE
        }
    }
}
