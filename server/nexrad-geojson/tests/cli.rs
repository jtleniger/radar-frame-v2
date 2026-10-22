use std::path::Path;
use std::process::Command;

use serde_json::Value;

const FIXTURE: &str = "tests/fixtures/ktlx-mini.ar2";
const EXPECTED: &str = "tests/fixtures/ktlx-mini-REF-1.expected.json";

fn run(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_nexrad-geojson"))
        .args(args)
        .output()
        .expect("binary runs")
}

fn read_json(path: &Path) -> Value {
    let text = std::fs::read_to_string(path).expect("json file readable");
    serde_json::from_str(&text).expect("json parses")
}

#[test]
fn matches_golden_reflectivity() {
    let dir = tempfile::tempdir().expect("temp dir");
    let base = dir.path().join("out");
    let base = base.to_str().expect("utf-8 path");

    let output = run(&[
        "--minimum",
        "0",
        "-e",
        "1",
        "-p",
        "REF",
        "-o",
        base,
        FIXTURE,
    ]);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let actual_path = format!("{base}-REF-1.json");
    let actual = read_json(Path::new(&actual_path));
    let expected = read_json(Path::new(EXPECTED));

    let actual_features = actual["features"].as_array().expect("features");
    let expected_features = expected["features"].as_array().expect("features");
    assert_eq!(actual_features.len(), expected_features.len());

    for (index, (actual, expected)) in actual_features
        .iter()
        .zip(expected_features.iter())
        .enumerate()
    {
        assert_eq!(actual["geometry"]["type"], "Polygon", "feature {index}");

        let actual_ring = actual["geometry"]["coordinates"][0]
            .as_array()
            .expect("ring");
        let expected_ring = expected["geometry"]["coordinates"][0]
            .as_array()
            .expect("ring");
        assert_eq!(actual_ring.len(), 5, "feature {index} ring length");
        assert_eq!(
            actual_ring.first(),
            actual_ring.last(),
            "feature {index} closed"
        );

        let actual_value = actual["properties"]["value"].as_f64().expect("value");
        let expected_value = expected["properties"]["value"].as_f64().expect("value");
        assert!(
            (actual_value - expected_value).abs() <= 0.05,
            "feature {index} value {actual_value} != {expected_value}"
        );

        for (point_index, (actual_point, expected_point)) in
            actual_ring.iter().zip(expected_ring.iter()).enumerate()
        {
            for (component, label) in [(0, "lon"), (1, "lat")] {
                let actual_component = actual_point[component].as_f64().expect("component");
                let expected_component = expected_point[component].as_f64().expect("component");
                assert!(
                    (actual_component - expected_component).abs() <= 1e-4,
                    "feature {index} point {point_index} {label}: {actual_component} != {expected_component}"
                );
            }
        }
    }
}

#[test]
fn missing_elevations_are_skipped() {
    let dir = tempfile::tempdir().expect("temp dir");
    let base = dir.path().join("out");
    let base = base.to_str().expect("utf-8 path");

    let output = run(&["-e", "1-3", "-p", "REF", "-o", base, FIXTURE]);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    assert!(Path::new(&format!("{base}-REF-1.json")).exists());
    assert!(!Path::new(&format!("{base}-REF-2.json")).exists());
    assert!(!Path::new(&format!("{base}-REF-3.json")).exists());

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("elevation 2 not present"),
        "stderr: {stderr}"
    );
    assert!(
        stderr.contains("elevation 3 not present"),
        "stderr: {stderr}"
    );
}

#[test]
fn missing_input_fails() {
    let output = run(&["-o", "/tmp/nexrad-geojson-missing", "/nonexistent/file.ar2"]);
    assert!(!output.status.success());
}
