use std::process::{Command, Stdio};

use frame_view::image::Image;
use frame_view::palette::EINK_COLORS;

fn fixture(name: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn run(command: &str, args: &[&str], stdin: &str) -> std::process::Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_frame-view"))
        .arg(command)
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
    // The Watch alert paints the info bar yellow.
    assert_eq!(image.pixel(0, 444), [255, 255, 0, 255]);
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
