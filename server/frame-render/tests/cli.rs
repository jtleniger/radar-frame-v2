//! End-to-end CLI tests: a synthetic 8x4 frame plus the GDAL golden frame.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use frame_render::pngio;

const BIN: &str = env!("CARGO_BIN_EXE_frame-render");
const COLORS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../gdal-colors.txt");

/// One rectangular gate on pixel row `row`, spanning columns 2..=5 of an
/// 8x4 frame over the bbox (0, 0) - (8, 4).
fn gate_ring(row: i32) -> Vec<[f64; 2]> {
    let low = 3.0 - f64::from(row);
    let high = 4.0 - f64::from(row);
    vec![[2.0, low], [2.0, high], [6.0, high], [6.0, low], [2.0, low]]
}

fn write_collection(path: &Path, gates: &[(f64, Vec<[f64; 2]>)]) {
    let features: Vec<String> = gates
        .iter()
        .map(|(value, ring)| {
            let coordinates: Vec<String> = ring
                .iter()
                .map(|point| format!("[{},{}]", point[0], point[1]))
                .collect();
            format!(
                r#"{{"type":"Feature","geometry":{{"type":"Polygon","coordinates":[[{}]]}},"properties":{{"value":{value}}}}}"#,
                coordinates.join(",")
            )
        })
        .collect();
    std::fs::write(
        path,
        format!(
            r#"{{"type":"FeatureCollection","features":[{}]}}"#,
            features.join(",")
        ),
    )
    .unwrap();
}

fn write_mask(path: &Path, width: u32, height: u32, lit: &[(u32, u32)]) {
    let mut samples = vec![0u8; (width * height) as usize];
    for &(x, y) in lit {
        samples[(y * width + x) as usize] = 255;
    }

    let file = std::fs::File::create(path).unwrap();
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Grayscale);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().unwrap();
    writer.write_image_data(&samples).unwrap();
    writer.finish().unwrap();
}

fn radar_args(input: &Path, streets: &Path, output: &Path) -> Vec<String> {
    vec![
        "radar".to_string(),
        "--min-lon".into(),
        "0".into(),
        "--min-lat".into(),
        "0".into(),
        "--max-lon".into(),
        "8".into(),
        "--max-lat".into(),
        "4".into(),
        "--width".into(),
        "8".into(),
        "--height".into(),
        "4".into(),
        "--input".into(),
        input.display().to_string(),
        "--colors".into(),
        COLORS.to_string(),
        "--streets".into(),
        streets.display().to_string(),
        "--output".into(),
        output.display().to_string(),
    ]
}

fn run(args: &[String]) -> Output {
    Command::new(BIN).args(args).output().unwrap()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn synthetic_frame_matches_the_expected_pixels() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("frame");
    let streets = dir.path().join("streets.png");
    let output = dir.path().join("frame.png");

    // Every row is gated from two or three elevations, with the higher value
    // always on a lower elevation for one row and on a higher one for the
    // other, so neither "first wins" nor "last wins" can pass the composite.
    write_collection(
        &PathBuf::from(format!("{}-REF-1.json", input.display())),
        &[
            (41.0, gate_ring(0)),
            (10.5, gate_ring(2)),
            (10.5, gate_ring(3)),
        ],
    );
    write_collection(
        &PathBuf::from(format!("{}-REF-2.json", input.display())),
        &[(20.0, gate_ring(1))],
    );
    write_collection(
        &PathBuf::from(format!("{}-REF-3.json", input.display())),
        &[(10.5, gate_ring(0)), (20.0, gate_ring(2))],
    );
    write_mask(&streets, 8, 4, &[(0, 0), (7, 3)]);

    let result = run(&radar_args(&input, &streets, &output));
    assert!(result.status.success(), "{}", stderr(&result));

    let (width, height, rgba) = pngio::read_rgba(&output).unwrap();
    assert_eq!((width, height), (8, 4));

    // gdal_calc writes its Byte nodata (255) wherever no elevation has a gate,
    // and 255 maps to the last color table entry.
    let uncovered = [46, 11, 119, 255];
    let orange = [234, 121, 36, 255]; // value 41
    let green = [57, 159, 93, 255]; // value 20
    let navy = [29, 45, 71, 255]; // value 11, from 10.5
    let white = [255, 255, 255, 255];

    let rows = [orange, green, green, navy];
    let mut expected = vec![uncovered; 32];
    for (y, color) in rows.iter().enumerate() {
        for x in 2..=5 {
            expected[y * 8 + x] = *color;
        }
    }
    expected[0] = white;
    expected[3 * 8 + 7] = white;

    let actual: &[[u8; 4]] = rgba.as_chunks::<4>().0;
    assert_eq!(actual, expected);
}

#[test]
fn missing_elevation_file_fails_with_its_path() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("frame");
    let streets = dir.path().join("streets.png");
    let output = dir.path().join("frame.png");

    write_collection(
        &PathBuf::from(format!("{}-REF-1.json", input.display())),
        &[(10.5, gate_ring(0))],
    );
    write_collection(
        &PathBuf::from(format!("{}-REF-3.json", input.display())),
        &[(80.0, gate_ring(0))],
    );
    write_mask(&streets, 8, 4, &[]);

    let result = run(&radar_args(&input, &streets, &output));
    assert!(!result.status.success());
    let err = stderr(&result);
    assert!(err.starts_with("frame-render: "), "{err}");
    assert!(err.contains("-REF-2.json"), "{err}");
    assert!(!output.exists());
}

#[test]
fn street_mask_size_mismatch_fails() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("frame");
    let streets = dir.path().join("streets.png");
    let output = dir.path().join("frame.png");

    for elevation in 1..=3 {
        write_collection(
            &PathBuf::from(format!("{}-REF-{elevation}.json", input.display())),
            &[(10.5, gate_ring(0))],
        );
    }
    write_mask(&streets, 7, 4, &[]);

    let result = run(&radar_args(&input, &streets, &output));
    assert!(!result.status.success());
    let err = stderr(&result);
    assert!(err.contains("7x4"), "{err}");
    assert!(err.contains("8x4"), "{err}");
}

/// The frame the pre-change GDAL/PIL renderer produced for the KTLX mini
/// volume, over the production bbox. See `tests/fixtures/README.md`.
#[test]
fn matches_the_gdal_golden_frame() {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ktlx-mini");
    let source = fixtures.join("nexrad-latest-REF-1.json");
    let streets = fixtures.join("streets.png");
    let expected_png = fixtures.join("frame.expected.png");
    for path in [&source, &streets, &expected_png] {
        assert!(path.exists(), "missing fixture {}", path.display());
    }

    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("nexrad-latest");
    let output = dir.path().join("nexrad-latest.png");
    // The mini volume has a single elevation; the renderer always reads three.
    for elevation in 1..=3 {
        std::fs::copy(
            &source,
            PathBuf::from(format!("{}-REF-{elevation}.json", input.display())),
        )
        .unwrap();
    }

    let args: Vec<String> = vec![
        "radar".into(),
        "--min-lon".into(),
        "-97.26".into(),
        "--min-lat".into(),
        "35.35".into(),
        "--max-lon".into(),
        "-97.21".into(),
        "--max-lat".into(),
        "35.39".into(),
        "--width".into(),
        "800".into(),
        "--height".into(),
        "444".into(),
        "--input".into(),
        input.display().to_string(),
        "--colors".into(),
        COLORS.to_string(),
        "--streets".into(),
        streets.display().to_string(),
        "--output".into(),
        output.display().to_string(),
    ];

    let result = run(&args);
    assert!(result.status.success(), "{}", stderr(&result));

    let actual = pngio::read_rgba(&output).unwrap();
    let expected = pngio::read_rgba(&expected_png).unwrap();

    assert_eq!(actual.0, expected.0);
    assert_eq!(actual.1, expected.1);
    assert_eq!(actual.2.len(), expected.2.len());

    let differing = actual
        .2
        .as_chunks::<4>()
        .0
        .iter()
        .zip(expected.2.as_chunks::<4>().0)
        .enumerate()
        .filter(|(_, (a, b))| a != b)
        .map(|(index, (a, b))| (index % actual.0, index / actual.0, *a, *b))
        .collect::<Vec<_>>();

    assert!(
        differing.is_empty(),
        "{} of {} pixels differ; first 10: {:?}",
        differing.len(),
        actual.0 * actual.1,
        &differing[..differing.len().min(10)]
    );
}

/// The street mask that the pre-change GDAL sequence produced for the ktlx-mini
/// fixture bbox. See `tests/fixtures/README.md`.
#[test]
fn street_mask_matches_the_gdal_golden_mask() {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ktlx-mini");
    let source = fixtures.join("streets.osm");
    let expected_png = fixtures.join("streets.png");
    for path in [&source, &expected_png] {
        assert!(path.exists(), "missing fixture {}", path.display());
    }

    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("streets.png");

    let args: Vec<String> = vec![
        "streets".into(),
        "--min-lon".into(),
        "-97.26".into(),
        "--min-lat".into(),
        "35.35".into(),
        "--max-lon".into(),
        "-97.21".into(),
        "--max-lat".into(),
        "35.39".into(),
        "--width".into(),
        "800".into(),
        "--height".into(),
        "444".into(),
        "--input".into(),
        source.display().to_string(),
        "--output".into(),
        output.display().to_string(),
    ];

    let result = run(&args);
    assert!(result.status.success(), "{}", stderr(&result));

    let actual = pngio::read_gray8(&output).unwrap();
    let expected = pngio::read_gray8(&expected_png).unwrap();

    assert_eq!(
        (actual.width, actual.height),
        (expected.width, expected.height)
    );
    assert_eq!(actual.transparent, expected.transparent);

    let differing = actual
        .samples
        .iter()
        .zip(&expected.samples)
        .enumerate()
        .filter(|(_, (a, b))| a != b)
        .map(|(index, (a, b))| (index % actual.width, index / actual.width, *a, *b))
        .collect::<Vec<_>>();

    assert!(
        differing.is_empty(),
        "{} of {} pixels differ; first 10: {:?}",
        differing.len(),
        actual.width * actual.height,
        &differing[..differing.len().min(10)]
    );
}
