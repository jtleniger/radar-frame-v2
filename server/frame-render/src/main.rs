use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};

use frame_render::render::{render_radar, Options as RadarOptions};
use frame_render::streets::{render_streets, Options as StreetsOptions};

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
