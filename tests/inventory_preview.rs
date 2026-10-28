//! Actual startup isolation fixtures, 2026-10-27; original Ajour GPLv3 applies.
use std::{
    fs,
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};
fn snapshot(root: &Path) -> Vec<(std::path::PathBuf, Option<Vec<u8>>)> {
    let mut result = vec![];
    fn visit(root: &Path, path: &Path, result: &mut Vec<(std::path::PathBuf, Option<Vec<u8>>)>) {
        result.push((
            path.strip_prefix(root).unwrap().to_path_buf(),
            if path.is_file() {
                Some(fs::read(path).unwrap())
            } else {
                None
            },
        ));
        if path.is_dir() {
            for entry in fs::read_dir(path).unwrap() {
                visit(root, &entry.unwrap().path(), result);
            }
        }
    }
    visit(root, root, &mut result);
    result.sort();
    result
}
#[test]
fn preview_binary_bypasses_legacy_startup_and_never_changes_fixture_or_config() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("copied-wow");
    fs::create_dir_all(root.join("_retail_/Interface/AddOns/Foo")).unwrap();
    fs::write(
        root.join(".build.info"),
        "Active!DEC:1|Product!STRING:0|Version!STRING:0\n1|wow|12.1.5.70575\n",
    )
    .unwrap();
    fs::write(
        root.join("_retail_/Interface/AddOns/Foo/Foo.toc"),
        "## Interface: 120105\n",
    )
    .unwrap();
    let config = temp.path().join("must-not-be-created");
    let run = |extra: &[&str]| {
        let mut child = Command::new(env!("CARGO_BIN_EXE_ajour"))
            .arg("--preview")
            .arg(&root)
            .arg("--preview-check")
            .args(extra)
            .env("APPDATA", &config)
            .env("XDG_CONFIG_HOME", &config)
            .env("HOME", &config)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let start = Instant::now();
        while child.try_wait().unwrap().is_none() {
            if start.elapsed() > Duration::from_secs(10) {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("Preview check did not exit; legacy startup may have been reached");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        child.wait_with_output().unwrap()
    };
    let before = snapshot(temp.path());
    let output = run(&[]);
    assert!(output.status.success(), "{:?}", output);
    let model: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        model["inventory"]["installations"][0]["product"].as_str(),
        Some("Retail")
    );
    assert!(model["selected"].is_null());
    let repeated = run(&[]);
    assert!(repeated.status.success(), "{:?}", repeated);
    assert_eq!(repeated.stdout, output.stdout);
    assert_eq!(snapshot(temp.path()), before);
    assert!(!config.exists());
    fs::write(
        root.join(".build.info"),
        "Active!DEC:1|Product!STRING:0|Version!STRING:0\n1|wow_unknown|13.0.0.1\n",
    )
    .unwrap();
    let before = snapshot(temp.path());
    let output = run(&[]);
    assert!(!output.status.success());
    let model: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(!model["inventory"]["issues"].as_array().unwrap().is_empty());
    assert_eq!(snapshot(temp.path()), before);
    assert!(!config.exists());
    let sentinel = temp.path().join("self-update-sentinel");
    fs::write(&sentinel, b"preserve").unwrap();
    let before = snapshot(temp.path());
    // The rejected legacy request also fails before configuration/network if routing regresses.
    assert!(!run(&[
        "--self-update-temp",
        sentinel.to_str().unwrap(),
        "install",
        "retail",
        "https://invalid.invalid/fixture"
    ])
    .status
    .success());
    assert!(!run(&[
        "--data",
        config.to_str().unwrap(),
        "install",
        "retail",
        "https://invalid.invalid/fixture"
    ])
    .status
    .success());
    assert!(
        !run(&["install", "retail", "https://invalid.invalid/fixture"])
            .status
            .success()
    );
    assert!(!run(&["--preview-window-check"]).status.success());
    assert_eq!(snapshot(temp.path()), before);
    assert!(!config.exists());
    fs::create_dir(&config).unwrap();
    fs::write(config.join("existing-config"), b"preserve config bytes").unwrap();
    let before = snapshot(temp.path());
    let first = run(&[]);
    let second = run(&[]);
    assert!(!first.status.success());
    assert!(!second.status.success());
    assert_eq!(first.stdout, second.stdout);
    assert!(serde_json::from_slice::<serde_json::Value>(&first.stdout).is_ok());
    assert_eq!(snapshot(temp.path()), before);
}
