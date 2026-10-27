//! Disposable, offline discovery CLI fixtures; original Ajour GPLv3 applies.
use std::{fs, path::Path, process::Command};

fn snapshot(root: &Path) -> Vec<(std::path::PathBuf, Option<Vec<u8>>)> {
    let mut result: Vec<_> = walkdir::WalkDir::new(root)
        .into_iter()
        .map(|entry| {
            let entry = entry.unwrap();
            (
                entry.path().strip_prefix(root).unwrap().to_path_buf(),
                if entry.file_type().is_file() {
                    Some(fs::read(entry.path()).unwrap())
                } else {
                    None
                },
            )
        })
        .collect();
    result.sort();
    result
}

#[test]
fn cli_reports_candidates_without_changing_install_or_legacy_config() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("copied-wow");
    let addon = root.join("_classic_/Interface/AddOns/Foo");
    fs::create_dir_all(&addon).unwrap();
    fs::write(
        root.join(".build.info"),
        "\u{feff}Version!STRING:0|Active!DEC:1|Product!STRING:0\r\n5.5.4.70579|1|wow_classic\r\n",
    )
    .unwrap();
    fs::write(addon.join("Foo_Mists.toc"), "## Interface: 50504\n").unwrap();
    let appdata = temp.path().join("legacy-config");
    fs::create_dir(&appdata).unwrap();
    fs::write(
        appdata.join("ajour.yml"),
        "wow:\n  directory: old-path\n  flavor: ClassicTbc\n",
    )
    .unwrap();
    let before = snapshot(temp.path());
    let run = || {
        Command::new(env!("CARGO_BIN_EXE_discover_wow"))
            .arg(&root)
            .env("APPDATA", &appdata)
            .env("XDG_CONFIG_HOME", &appdata)
            .env("HOME", &appdata)
            .output()
            .unwrap()
    };
    let output = run();
    assert!(output.status.success(), "{:?}", output);
    let report: serde_yaml::Value = serde_yaml::from_slice(&output.stdout).unwrap();
    assert_eq!(
        report["installations"][0]["product"].as_str(),
        Some("ClassicProgression")
    );
    assert_eq!(
        report["installations"][0]["interface"].as_u64(),
        Some(50504)
    );
    assert!(report["installations"][0]["addons"][0]["selected"]
        .as_str()
        .unwrap()
        .ends_with("Foo_Mists.toc"));
    assert_eq!(snapshot(temp.path()), before);
    fs::write(
        root.join(".build.info"),
        "Active!DEC:1|Product!STRING:0|Version!STRING:0\n1|wow_future|13.0.0.1\n",
    )
    .unwrap();
    let before = snapshot(temp.path());
    assert_eq!(run().status.code(), Some(1));
    assert_eq!(snapshot(temp.path()), before);
    fs::write(
        root.join(".build.info"),
        "Active!DEC:1|Product!STRING:0|Version!STRING:0\n0|wow|12.1.5.70575\n",
    )
    .unwrap();
    assert_eq!(run().status.code(), Some(1));
}
