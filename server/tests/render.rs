//! End-to-end CLI tests for the fused Archive II renderer and the GDAL goldens.

use std::path::Path;
use std::process::{Command, Output};

use radar_frame::render::pngio;

const BIN: &str = env!("CARGO_BIN_EXE_radar-frame");

const MINI_ARCHIVE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/nexrad/ktlx-mini.ar2"
);

/// `render archive` over the ktlx-mini fixture bbox (`tests/fixtures/frame/README.md`).
fn fixture_frame_args(archive: &Path, streets: &Path, output: &Path) -> Vec<String> {
    vec![
        "render".into(),
        "archive".into(),
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
        "--archive".into(),
        archive.display().to_string(),
        "--minimum".into(),
        "10".into(),
        "--streets".into(),
        streets.display().to_string(),
        "--output".into(),
        output.display().to_string(),
    ]
}

/// Compare two decoded frames pixel for pixel and report the first differences.
fn assert_same_frame(actual: &Path, expected: &Path) {
    let actual = pngio::read_rgba(actual).unwrap();
    let expected = pngio::read_rgba(expected).unwrap();
    assert_eq!((actual.0, actual.1), (expected.0, expected.1));
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

fn run(args: &[String]) -> Output {
    Command::new(BIN).args(args).output().unwrap()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// The street mask that the pre-change GDAL sequence produced for the ktlx-mini
/// fixture bbox. See `tests/fixtures/frame/README.md`.
#[test]
fn street_mask_matches_the_gdal_golden_mask() {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/frame/ktlx-mini");
    let source = fixtures.join("streets.osm");
    let expected_png = fixtures.join("streets.png");
    for path in [&source, &expected_png] {
        assert!(path.exists(), "missing fixture {}", path.display());
    }

    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("streets.png");

    let args: Vec<String> = vec![
        "render".into(),
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

fn with_elevations(mut args: Vec<String>, elevations: &str) -> Vec<String> {
    args.push("-e".into());
    args.push(elevations.into());
    args
}

/// The frame the pre-change GDAL/PIL renderer produced for the KTLX mini
/// volume, over the production bbox. See `tests/fixtures/frame/README.md`.
#[test]
fn fused_frame_matches_the_gdal_golden() {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/frame/ktlx-mini");
    let archive = Path::new(MINI_ARCHIVE);
    let streets = fixtures.join("streets.png");
    let expected_png = fixtures.join("frame.expected.png");
    for path in [archive, &streets, &expected_png] {
        assert!(path.exists(), "missing fixture {}", path.display());
    }

    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("frame.png");
    // The mini volume has a single elevation.
    let args = with_elevations(fixture_frame_args(archive, &streets, &output), "1");
    let result = run(&args);
    assert!(result.status.success(), "{}", stderr(&result));

    assert_same_frame(&output, &expected_png);

    let mut entries: Vec<String> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    entries.sort();
    assert_eq!(entries, vec!["frame.png"], "wrote an intermediate");
}

/// The renderer reports the elevations and counts it actually composited.
#[test]
fn archive_summary_reports_elevations_and_counts() {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/frame/ktlx-mini");
    let archive = Path::new(MINI_ARCHIVE);
    let streets = fixtures.join("streets.png");
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("frame.png");

    let summary =
        radar_frame::render::archive::render_archive(&radar_frame::render::archive::Options {
            min_lon: -97.26,
            min_lat: 35.35,
            max_lon: -97.21,
            max_lat: 35.39,
            width: 800,
            height: 444,
            archive: archive.to_path_buf(),
            elevations: vec![1],
            minimum: Some(10.0),
            maximum: None,
            colors: None,
            streets: Some(streets),
            output,
        })
        .unwrap();

    assert_eq!(summary.elevations, vec![1]);
    assert!(summary.radials > 0, "no radials counted");
    assert!(summary.gates > 0, "no gates counted");
}

/// The radar frame renders without the overlay while the mask is unavailable.
#[test]
fn archive_renders_without_a_street_mask() {
    let archive = Path::new(MINI_ARCHIVE);
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("frame.png");

    let summary =
        radar_frame::render::archive::render_archive(&radar_frame::render::archive::Options {
            min_lon: -97.26,
            min_lat: 35.35,
            max_lon: -97.21,
            max_lat: 35.39,
            width: 800,
            height: 444,
            archive: archive.to_path_buf(),
            elevations: vec![1],
            minimum: Some(10.0),
            maximum: None,
            colors: None,
            streets: None,
            output: output.clone(),
        })
        .unwrap();

    assert_eq!(summary.elevations, vec![1]);
    let (width, height, rgba) = pngio::read_rgba(&output).unwrap();
    assert_eq!((width, height), (800, 444));
    assert!(rgba.iter().any(|&byte| byte != 0), "frame is blank");
}

/// Missing selected elevations are skipped; at least one must be present.
#[test]
fn fused_frame_skips_missing_elevations() {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/frame/ktlx-mini");
    let archive = Path::new(MINI_ARCHIVE);
    let streets = fixtures.join("streets.png");
    let expected_png = fixtures.join("frame.expected.png");

    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("frame.png");

    // Default `-e 1-3`; the mini volume only has elevation 1. The composited
    // frame therefore matches the elevation-1 golden.
    let result = run(&fixture_frame_args(archive, &streets, &output));
    assert!(result.status.success(), "{}", stderr(&result));
    assert_same_frame(&output, &expected_png);

    // No requested elevation is present: that is still an error.
    let output = dir.path().join("missing.png");
    let args = with_elevations(fixture_frame_args(archive, &streets, &output), "2");
    let result = run(&args);
    assert!(!result.status.success());
    let err = stderr(&result);
    assert!(err.contains("none of the requested elevations"), "{err}");
    assert!(!output.exists());
}

#[test]
fn fused_frame_rejects_invalid_elevations() {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/frame/ktlx-mini");
    let archive = Path::new(MINI_ARCHIVE);
    let streets = fixtures.join("streets.png");
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("frame.png");

    let args = with_elevations(fixture_frame_args(archive, &streets, &output), "a");
    let result = run(&args);
    assert!(!result.status.success());
    let err = stderr(&result);
    assert!(err.contains("invalid elevations a"), "{err}");
    assert!(!output.exists());
}

#[test]
fn colors_flag_overrides_the_builtin_table() {
    let archive = Path::new(MINI_ARCHIVE);

    let dir = tempfile::tempdir().unwrap();
    let colors = dir.path().join("colors.txt");
    std::fs::write(&colors, "0 255 0 0 255\n").unwrap();
    // A blank mask isolates the color table from the street overlay blending.
    let streets = dir.path().join("streets.png");
    pngio::write_gray8(&streets, 800, 444, &vec![0u8; 800 * 444], None).unwrap();
    let output = dir.path().join("frame.png");

    let mut args = with_elevations(fixture_frame_args(archive, &streets, &output), "1");
    args.push("--colors".into());
    args.push(colors.display().to_string());

    let result = run(&args);
    assert!(result.status.success(), "{}", stderr(&result));

    let (width, height, rgba) = pngio::read_rgba(&output).unwrap();
    assert_eq!((width, height), (800, 444));
    assert_eq!(&rgba[0..4], [255, 0, 0, 255]);
}
