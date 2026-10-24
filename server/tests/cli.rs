//! End-to-end tests for `radar-frame demo`, all offline through the mocks.

use std::path::Path;
use std::process::{Command, Output};

const BIN: &str = env!("CARGO_BIN_EXE_radar-frame");

const CONFIG: &str = "\
[logging]
level = INFO

[radar]
nexrad_id = KTLX

[bbox]
min_lat = 35.35
max_lat = 35.39
min_lon = -97.26
max_lon = -97.21

[forecast]
lat = 35.37
lon = -97.23
timezone = America/Chicago

[nws]
user_agent = radar-frame (test@example.com)
zone = OKZ025
fire_zone = OKZ025
";

fn fixture() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    std::fs::create_dir_all(root.join("config")).unwrap();
    std::fs::write(root.join("config/config.ini"), CONFIG).unwrap();
    (dir, root)
}

fn run(root: &Path, args: &[&str]) -> Output {
    Command::new(BIN)
        .current_dir(root)
        .args(args)
        .output()
        .unwrap()
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn warnings_mock_lists_the_sample_alerts() {
    let (_dir, root) = fixture();
    let output = run(&root, &["demo", "warnings", "--mock"]);
    assert!(output.status.success(), "{}", stderr(&output));

    let stdout = stdout(&output);
    assert!(stdout.contains("warnings: 3 active of 3"), "{stdout}");
    assert!(stdout.contains("tornado warning"), "{stdout}");
    assert!(stdout.contains("flash flood watch"), "{stdout}");
}

#[test]
fn clear_mock_renders_a_frame_with_alerts() {
    let (_dir, root) = fixture();
    let output = run(&root, &["demo", "clear", "--mock"]);
    assert!(output.status.success(), "{}", stderr(&output));

    let stdout = stdout(&output);
    assert!(stdout.contains("forecast: 101.0 F"), "{stdout}");
    assert!(stdout.contains("warnings: 3 active of 3"), "{stdout}");
    assert!(root.join("data/frame.png").is_file());
}
