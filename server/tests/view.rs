use std::path::PathBuf;
use std::process::{Command, Stdio};

use chrono::{TimeZone, Utc};
use chrono_tz::America::Chicago;

use radar_frame::view::font::Fonts;
use radar_frame::view::forecast::{Alert, ClearInput};
use radar_frame::view::image::Image;
use radar_frame::view::palette::{write_eink_png, Palette, EINK_COLORS};
use radar_frame::view::views::{render_clear, render_storm};

const BIN: &str = env!("CARGO_BIN_EXE_radar-frame");

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/view")
        .join(name)
}

fn run(command: &str, args: &[&str], stdin: &str) -> std::process::Output {
    let mut child = Command::new(BIN)
        .args(["view", command])
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    {
        use std::io::Write;
        child
            .stdin
            .as_mut()
            .unwrap()
            .write_all(stdin.as_bytes())
            .unwrap();
    }

    child.wait_with_output().unwrap()
}

#[test]
fn storm_reads_alerts_from_stdin_and_writes_the_frame() {
    let dir = tempfile::tempdir().unwrap();
    let palette = dir.path().join("palette.png");
    let output = dir.path().join("frame.png");

    let prepared = run("palette", &["--output", palette.to_str().unwrap()], "");
    assert!(prepared.status.success(), "{prepared:?}");

    let alerts = std::fs::read_to_string(fixture("alerts.json")).unwrap();
    let result = run(
        "storm",
        &[
            "--radar",
            fixture("radar.png").to_str().unwrap(),
            "--palette",
            palette.to_str().unwrap(),
            "--output",
            output.to_str().unwrap(),
            "--timezone",
            "America/Chicago",
            "--now",
            "2025-06-01T12:34:56Z",
        ],
        &alerts,
    );
    assert!(result.status.success(), "{result:?}");

    let image = Image::from_png(&output).unwrap();
    assert_eq!((image.width, image.height), (800, 480));
    // The Warning alert outranks the Watch, so the info bar is orange.
    assert_eq!(image.pixel(0, 444), [255, 128, 0, 255]);
    for y in (0..480).step_by(23) {
        for x in (0..800).step_by(37) {
            let pixel = image.pixel(x, y);
            assert!(EINK_COLORS.contains(&[pixel[0], pixel[1], pixel[2]]));
        }
    }
}

#[test]
fn clear_reads_forecast_json_from_stdin() {
    let dir = tempfile::tempdir().unwrap();
    let palette = dir.path().join("palette.png");
    let output = dir.path().join("frame.png");

    let prepared = run("palette", &["--output", palette.to_str().unwrap()], "");
    assert!(prepared.status.success(), "{prepared:?}");

    let forecast = std::fs::read_to_string(fixture("forecast.json")).unwrap();
    let result = run(
        "clear",
        &[
            "--palette",
            palette.to_str().unwrap(),
            "--output",
            output.to_str().unwrap(),
            "--timezone",
            "America/Chicago",
            "--now",
            "2025-06-01T12:34:56Z",
        ],
        &forecast,
    );
    assert!(result.status.success(), "{result:?}");

    let image = Image::from_png(&output).unwrap();
    assert_eq!((image.width, image.height), (800, 480));
    // The column rule at x=266 is black, and the info bar has white text.
    assert_eq!(image.pixel(266, 100), [0, 0, 0, 255]);
    let white_in_bar =
        (540..800).any(|x| (444..480).any(|y| image.pixel(x, y) == [255, 255, 255, 255]));
    assert!(white_in_bar);
}

fn palette() -> Palette {
    Palette::eink()
}

fn is_palette_color(image: &Image) -> bool {
    for y in 0..image.height {
        for x in 0..image.width {
            let pixel = image.pixel(x, y);
            let rgb = [pixel[0], pixel[1], pixel[2]];
            if !EINK_COLORS.contains(&rgb) {
                return false;
            }
        }
    }
    true
}

/// Mean absolute channel difference after averaging each 8x8 block. This is a
/// "does it look the same" check: pixel-level dither differences are fine,
/// misplaced text, colours, or layout are not.
fn block_mean_abs_diff(actual: &Image, expected: &Image, block: u32) -> f64 {
    assert_eq!(actual.width, expected.width);
    assert_eq!(actual.height, expected.height);

    let mut total = 0.0;
    let mut blocks = 0;
    let mut y = 0;
    while y < actual.height {
        let mut x = 0;
        while x < actual.width {
            let mut sums = [0f64; 3];
            let mut count = 0;
            for by in y..(y + block).min(actual.height) {
                for bx in x..(x + block).min(actual.width) {
                    let a = actual.pixel(bx, by);
                    let e = expected.pixel(bx, by);
                    for channel in 0..3 {
                        sums[channel] += a[channel] as f64 - e[channel] as f64;
                    }
                    count += 1;
                }
            }
            total += sums
                .iter()
                .map(|sum| (sum / count as f64).abs())
                .sum::<f64>()
                / 3.0;
            blocks += 1;
            x += block;
        }
        y += block;
    }

    total / blocks as f64
}

fn ticking_pixels(actual: &Image, expected: &Image) -> f64 {
    let mut differing = 0;
    for y in 0..actual.height {
        for x in 0..actual.width {
            if actual.pixel(x, y) != expected.pixel(x, y) {
                differing += 1;
            }
        }
    }
    differing as f64 / (actual.width * actual.height) as f64
}

fn now() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2025, 6, 1, 12, 34, 56).unwrap()
}

#[test]
fn clear_frame_matches_the_pillow_renderer() {
    let fonts = Fonts::new();
    let input: ClearInput =
        serde_json::from_str(&std::fs::read_to_string(fixture("forecast.json")).unwrap()).unwrap();

    let actual = render_clear(
        &fonts,
        &input.forecast,
        &input.alerts,
        now(),
        Chicago,
        &palette(),
    )
    .unwrap();
    let expected = Image::from_png(&fixture("clear.expected.png")).unwrap();

    assert_eq!((actual.width, actual.height), (800, 480));
    assert!(is_palette_color(&actual));
    assert!(ticking_pixels(&actual, &expected) < 0.01);
    assert!(block_mean_abs_diff(&actual, &expected, 8) < 6.0);
}

#[test]
fn storm_frame_matches_the_pillow_renderer() {
    let fonts = Fonts::new();
    let alerts: Vec<Alert> =
        serde_json::from_str(&std::fs::read_to_string(fixture("alerts.json")).unwrap()).unwrap();
    let radar = Image::from_png(&fixture("radar.png")).unwrap();

    let actual = render_storm(&fonts, &radar, &alerts, now(), Chicago, &palette()).unwrap();
    let expected = Image::from_png(&fixture("storm.expected.png")).unwrap();

    assert_eq!((actual.width, actual.height), (800, 480));
    assert!(is_palette_color(&actual));
    assert!(block_mean_abs_diff(&actual, &expected, 8) < 6.0);

    // The bar has no dithering, only text edges can differ.
    let mut bar_differing = 0;
    for y in 444..480 {
        for x in 0..800 {
            if actual.pixel(x, y) != expected.pixel(x, y) {
                bar_differing += 1;
            }
        }
    }
    assert!(bar_differing < 800 * 36 / 10);
}

#[test]
fn info_bar_without_alerts_is_black() {
    let fonts = Fonts::new();
    let radar = Image::from_png(&fixture("radar.png")).unwrap();
    let actual = render_storm(&fonts, &radar, &[], now(), Chicago, &palette()).unwrap();

    assert_eq!(actual.pixel(0, 444), [0, 0, 0, 255]);
    assert_eq!(actual.pixel(10, 470), [0, 0, 0, 255]);
    // The update time is drawn in the top-right of the bar.
    let white = (540..800).any(|x| (444..480).any(|y| actual.pixel(x, y) == [255, 255, 255, 255]));
    assert!(white);
}

#[test]
fn palette_round_trips_through_a_png() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("palette.png");
    write_eink_png(&path).unwrap();

    assert_eq!(Palette::from_png(&path).unwrap().colors, EINK_COLORS);
}
