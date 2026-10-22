use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::process::exit;
use std::str::FromStr;

use clap::Parser;

use nexrad_geojson::archive2::{read_volume, Product};
use nexrad_geojson::elevations::parse_elevations;
use nexrad_geojson::geo::{shortest_f32_degrees, Ortho};
use nexrad_geojson::geojson::{build_bins, write_feature_collection};

#[derive(Parser)]
#[command(
    name = "nexrad-geojson",
    version,
    about = "Create GeoJSON from NEXRAD Archive II Level 2 data"
)]
struct Cli {
    /// Log level: debug, info, warn, error
    #[arg(short = 'l', long, default_value = "warn")]
    log_level: String,

    /// Minimum product value to include in the output
    #[arg(long)]
    minimum: Option<f32>,

    /// Maximum product value to include in the output
    #[arg(long)]
    maximum: Option<f32>,

    /// Product to output: REF, VEL, SW, ZDR, PHI, RHO
    #[arg(short = 'p', long, default_value = "REF")]
    product: String,

    /// Elevation or range of elevations: N or N-M (inclusive)
    #[arg(short = 'e', long, default_value = "1")]
    elevations: String,

    /// Base filename for output; elevation, product and extension are appended
    #[arg(short = 'o', long, default_value = "radar")]
    output: String,

    /// NEXRAD Archive II file
    input: PathBuf,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

impl LogLevel {
    fn parse(value: &str) -> Option<Self> {
        match value.to_ascii_lowercase().as_str() {
            "debug" => Some(LogLevel::Debug),
            "info" => Some(LogLevel::Info),
            "warn" => Some(LogLevel::Warn),
            "error" => Some(LogLevel::Error),
            _ => None,
        }
    }

    fn warns(self) -> bool {
        self != LogLevel::Error
    }
}

fn main() {
    let cli = Cli::parse();

    let Some(log_level) = LogLevel::parse(&cli.log_level) else {
        eprintln!("invalid log level {}", cli.log_level);
        exit(1);
    };

    let product_name = cli.product.to_ascii_uppercase();
    let Ok(product) = Product::from_str(&product_name) else {
        eprintln!("invalid product {product_name}");
        exit(1);
    };

    let Some(elevations) = parse_elevations(&cli.elevations) else {
        eprintln!("invalid elevations {}", cli.elevations);
        exit(1);
    };

    let volume = match read_volume(&cli.input) {
        Ok(volume) => volume,
        Err(err) => {
            eprintln!("nexrad-geojson: {err}");
            exit(1);
        }
    };

    let Some(first) = volume.scans.get(&1).and_then(|scan| scan.first()) else {
        eprintln!("nexrad-geojson: elevation 1 not present");
        exit(1);
    };
    let ortho = Ortho::new(
        shortest_f32_degrees(first.volume.lat),
        shortest_f32_degrees(first.volume.lon),
    );

    for elevation in elevations {
        let Some(scan) = volume.scans.get(&elevation) else {
            if log_level.warns() {
                eprintln!("elevation {elevation} not present");
            }
            continue;
        };

        let mut bins = Vec::new();
        for radial in scan {
            match build_bins(radial, product, &ortho, cli.minimum, cli.maximum) {
                Ok(radial_bins) => bins.extend(radial_bins),
                Err(err) => {
                    eprintln!("nexrad-geojson: {err}");
                    exit(1);
                }
            }
        }

        let path = format!("{}-{}-{}.json", cli.output, product.as_str(), elevation);
        let file = match File::create(&path) {
            Ok(file) => file,
            Err(err) => {
                eprintln!("nexrad-geojson: {path}: {err}");
                exit(1);
            }
        };

        let mut writer = BufWriter::new(file);
        if let Err(err) = write_feature_collection(&mut writer, &bins) {
            eprintln!("nexrad-geojson: {path}: {err}");
            exit(1);
        }
        if let Err(err) = writer.flush() {
            eprintln!("nexrad-geojson: {path}: {err}");
            exit(1);
        }
    }
}
