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

// Added 2026-10-28; original Ajour GPLv3 applies.
mod safe_entry_tests {
    use super::*;
    use std::{ffi::OsString, process::Output};

    fn prepare(root: &Path, state: usize) {
        fs::create_dir(root.join("work")).unwrap();
        fs::create_dir(root.join("scratch")).unwrap();

        // A rootless check must not discover its working directory.
        fs::write(
            root.join("work/.build.info"),
            concat!(
                "Active!DEC:1|Product!STRING:0|Version!STRING:0\n",
                "1|wow_unknown|13.0.0.1\n",
            ),
        )
        .unwrap();

        for directory in ["home", "config", "data", "cache", "state", "runtime"] {
            let path = root.join(directory);
            if state != 0 {
                fs::create_dir(&path).unwrap();
            }
            if state == 2 {
                fs::write(
                    path.join("existing-config"),
                    b"preserve existing config bytes\0\n",
                )
                .unwrap();
            }
        }
    }

    fn copied_root(root: &Path) -> std::path::PathBuf {
        let wow = root.join("copied-wow");
        fs::create_dir_all(wow.join("_retail_/Interface/AddOns/Foo")).unwrap();
        fs::write(
            wow.join(".build.info"),
            concat!(
                "Active!DEC:1|Product!STRING:0|Version!STRING:0\n",
                "1|wow|12.1.5.70575\n",
            ),
        )
        .unwrap();
        fs::write(
            wow.join("_retail_/Interface/AddOns/Foo/Foo.toc"),
            b"## Interface: 120105\n",
        )
        .unwrap();
        wow
    }

    fn run(root: &Path, args: &[OsString]) -> Output {
        // Literal zero-argument execution would open the desktop window.
        assert!(!args.is_empty());
        let mut child = Command::new(env!("CARGO_BIN_EXE_ajour"))
            .args(args)
            .current_dir(root.join("work"))
            .env("HOME", root.join("home"))
            .env("USERPROFILE", root.join("home"))
            .env("APPDATA", root.join("config"))
            .env("LOCALAPPDATA", root.join("data"))
            .env("XDG_CONFIG_HOME", root.join("config"))
            .env("XDG_DATA_HOME", root.join("data"))
            .env("XDG_CACHE_HOME", root.join("cache"))
            .env("XDG_STATE_HOME", root.join("state"))
            .env("XDG_RUNTIME_DIR", root.join("runtime"))
            .env("TMP", root.join("scratch"))
            .env("TEMP", root.join("scratch"))
            .env("TMPDIR", root.join("scratch"))
            .env("APPIMAGE", root.join("work/fixture-appimage"))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();

        let start = Instant::now();
        while child.try_wait().unwrap().is_none() {
            if start.elapsed() > Duration::from_secs(10) {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("Safe entry did not exit within the fixture timeout");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        child.wait_with_output().unwrap()
    }

    #[cfg(feature = "no-self-update")]
    fn report(output: &Output) -> serde_json::Value {
        serde_json::from_slice(&output.stdout)
            .unwrap_or_else(|error| panic!("Invalid JSON: {}; output: {:?}", error, output))
    }

    #[test]
    fn help_and_version_exit_without_creating_isolated_paths() {
        let temp = tempfile::tempdir().unwrap();
        prepare(temp.path(), 0);
        let before = snapshot(temp.path());

        let help = run(temp.path(), &["--help".into()]);
        assert!(help.status.success(), "{:?}", help);
        let help_text = String::from_utf8(help.stdout).unwrap();
        assert!(help_text.contains("--preview"));
        assert!(!help_text.contains("--startup-check"));
        assert!(!help_text.contains("--preview-window-check"));
        assert_eq!(snapshot(temp.path()), before);

        let version = run(temp.path(), &["--version".into()]);
        assert!(version.status.success(), "{:?}", version);
        assert!(String::from_utf8(version.stdout)
            .unwrap()
            .contains(env!("CARGO_PKG_VERSION")));
        assert_eq!(snapshot(temp.path()), before);
    }

    #[cfg(feature = "no-self-update")]
    #[test]
    fn real_hidden_rootless_entry_has_no_commands_or_persistent_side_effects() {
        for state in 0..3 {
            let temp = tempfile::tempdir().unwrap();
            prepare(temp.path(), state);
            let before = snapshot(temp.path());

            let first = run(temp.path(), &["--startup-check".into()]);
            assert!(first.status.success(), "{:?}", first);
            assert!(first.stderr.is_empty(), "{:?}", first);
            let value = report(&first);
            assert_eq!(value["entry"], "safe-startup");
            assert_eq!(value["initial_commands"], 0);
            assert_eq!(value["model"]["root"], "");
            assert!(value["model"]["inventory"].is_null());
            assert!(value["model"]["error"].is_null());
            assert!(value["model"]["selected"].is_null());
            assert_eq!(value["model"].as_object().unwrap().len(), 4);

            let second = run(temp.path(), &["--startup-check".into()]);
            assert!(second.status.success(), "{:?}", second);
            assert_eq!(first.stdout, second.stdout);
            assert_eq!(snapshot(temp.path()), before);

            if state == 0 {
                for path in ["home", "config", "data", "cache", "state", "runtime"] {
                    assert!(!temp.path().join(path).exists());
                }
            }
        }
    }

    #[cfg(feature = "no-self-update")]
    #[test]
    fn default_gui_legacy_options_reject_without_import_or_cleanup() {
        for state in [0, 2] {
            let temp = tempfile::tempdir().unwrap();
            prepare(temp.path(), state);
            let wow = copied_root(temp.path());
            let sentinel = temp.path().join("self-update-sentinel");
            fs::write(&sentinel, b"preserve cleanup target\n").unwrap();
            let missing = temp.path().join("must-not-be-created");
            let existing_data = temp.path().join("existing-data");
            fs::create_dir(&existing_data).unwrap();
            fs::write(existing_data.join("existing-config"), b"do not import\n").unwrap();
            let before = snapshot(temp.path());

            for (flag, path) in [
                ("--data", &missing),
                ("--data", &existing_data),
                ("--self-update-temp", &missing),
                ("--self-update-temp", &sentinel),
            ] {
                for mode in 0..3 {
                    let mut args = vec![OsString::from(flag), path.as_os_str().to_owned()];
                    match mode {
                        0 => {}
                        1 => args.push("--startup-check".into()),
                        _ => {
                            args.push("--preview".into());
                            args.push(wow.as_os_str().to_owned());
                            args.push("--startup-check".into());
                        }
                    }

                    let output = run(temp.path(), &args);
                    assert!(!output.status.success(), "{:?}", output);
                    assert!(output.stdout.is_empty(), "{:?}", output);
                    assert!(
                        String::from_utf8_lossy(&output.stderr).contains("cannot be combined"),
                        "{:?}",
                        output
                    );
                    assert_eq!(snapshot(temp.path()), before);
                    assert!(!missing.exists());
                    assert_eq!(fs::read(&sentinel).unwrap(), b"preserve cleanup target\n");
                }
            }
        }
    }

    #[cfg(feature = "no-self-update")]
    #[test]
    fn real_hidden_explicit_root_runs_local_scan_and_preserves_fixture_bytes() {
        let temp = tempfile::tempdir().unwrap();
        prepare(temp.path(), 2);
        let wow = copied_root(temp.path());
        let before = snapshot(temp.path());
        let args = vec![
            "--startup-check".into(),
            "--preview".into(),
            wow.as_os_str().to_owned(),
        ];

        let first = run(temp.path(), &args);
        assert!(first.status.success(), "{:?}", first);
        let value = report(&first);
        assert_eq!(value["entry"], "safe-startup");
        assert_eq!(value["initial_commands"], 1);
        assert_eq!(value["model"]["root"], serde_json::to_value(&wow).unwrap());
        assert!(value["model"]["error"].is_null());
        assert!(value["model"]["selected"].is_null());
        assert_eq!(
            value["model"]["inventory"]["installations"][0]["product"],
            "Retail"
        );
        assert_eq!(
            value["model"]["inventory"]["installations"][0]["addons"][0]["addon"],
            "Foo"
        );
        let second = run(temp.path(), &args);
        assert!(second.status.success(), "{:?}", second);
        assert_eq!(first.stdout, second.stdout);
        assert_eq!(snapshot(temp.path()), before);
    }

    #[cfg(feature = "no-self-update")]
    #[test]
    fn real_hidden_invalid_unknown_and_empty_roots_fail_with_json() {
        let temp = tempfile::tempdir().unwrap();
        prepare(temp.path(), 0);
        let wow = copied_root(temp.path());
        fs::write(
            wow.join(".build.info"),
            concat!(
                "Active!DEC:1|Product!STRING:0|Version!STRING:0\n",
                "1|wow_unknown|13.0.0.1\n",
            ),
        )
        .unwrap();
        let missing = temp.path().join("missing-wow");
        let before = snapshot(temp.path());

        for (root, expected_commands) in [
            (missing.as_os_str().to_owned(), 1),
            (wow.as_os_str().to_owned(), 1),
            (OsString::new(), 0),
        ] {
            let output = run(
                temp.path(),
                &["--startup-check".into(), "--preview".into(), root.clone()],
            );
            assert!(!output.status.success(), "{:?}", output);
            let value = report(&output);
            assert_eq!(value["entry"], "safe-startup");
            assert_eq!(value["initial_commands"], expected_commands);
            assert!(value["model"]["selected"].is_null());
            if root == wow.as_os_str() {
                let inventory = &value["model"]["inventory"];
                assert!(inventory["installations"].as_array().unwrap().is_empty());
                assert!(!inventory["issues"].as_array().unwrap().is_empty());
            } else {
                assert!(value["model"]["inventory"].is_null());
                assert!(!value["model"]["error"].as_str().unwrap().is_empty());
            }
            assert_eq!(snapshot(temp.path()), before);
            assert!(!missing.exists());
        }
    }

    #[cfg(feature = "no-self-update")]
    #[test]
    fn preview_check_keeps_plain_model_json_for_success_and_failure() {
        let temp = tempfile::tempdir().unwrap();
        prepare(temp.path(), 2);
        let wow = copied_root(temp.path());
        let args = vec![
            "--preview".into(),
            wow.as_os_str().to_owned(),
            "--preview-check".into(),
        ];
        let before = snapshot(temp.path());

        let success = run(temp.path(), &args);
        assert!(success.status.success(), "{:?}", success);
        let value = report(&success);
        assert!(value.get("entry").is_none());
        assert!(value.get("initial_commands").is_none());
        assert_eq!(value.as_object().unwrap().len(), 4);
        assert_eq!(value["inventory"]["installations"][0]["product"], "Retail");
        assert!(value["error"].is_null());
        assert!(value["selected"].is_null());
        assert_eq!(snapshot(temp.path()), before);

        fs::write(
            wow.join(".build.info"),
            concat!(
                "Active!DEC:1|Product!STRING:0|Version!STRING:0\n",
                "1|wow_unknown|13.0.0.1\n",
            ),
        )
        .unwrap();
        let before = snapshot(temp.path());
        let failure = run(temp.path(), &args);
        assert!(!failure.status.success(), "{:?}", failure);
        let value = report(&failure);
        assert!(value.get("entry").is_none());
        assert_eq!(value.as_object().unwrap().len(), 4);
        assert!(value["inventory"]["installations"]
            .as_array()
            .unwrap()
            .is_empty());
        assert!(!value["inventory"]["issues"].as_array().unwrap().is_empty());
        assert_eq!(snapshot(temp.path()), before);

        let missing = temp.path().join("missing-preview-root");
        let failure = run(
            temp.path(),
            &[
                "--preview".into(),
                missing.as_os_str().to_owned(),
                "--preview-check".into(),
            ],
        );
        assert!(!failure.status.success(), "{:?}", failure);
        let value = report(&failure);
        assert!(value.get("entry").is_none());
        assert!(value["inventory"].is_null());
        assert!(!value["error"].as_str().unwrap().is_empty());
        assert_eq!(snapshot(temp.path()), before);
        assert!(!missing.exists());
    }

    #[cfg(feature = "no-self-update")]
    #[test]
    fn missing_paths_and_mixed_check_modes_fail_before_any_window() {
        let temp = tempfile::tempdir().unwrap();
        prepare(temp.path(), 0);
        let wow = copied_root(temp.path());
        let before = snapshot(temp.path());

        for flag in ["--preview-check", "--preview-window-check"] {
            let output = run(temp.path(), &[flag.into()]);
            assert!(!output.status.success(), "{:?}", output);
            assert!(output.stdout.is_empty(), "{:?}", output);
            assert!(
                String::from_utf8_lossy(&output.stderr).contains("--preview"),
                "{:?}",
                output
            );
            assert_eq!(snapshot(temp.path()), before);
        }

        for flags in [
            vec!["--startup-check", "--preview-check"],
            vec!["--startup-check", "--preview-window-check"],
            vec!["--preview-check", "--preview-window-check"],
        ] {
            let mut args = vec!["--preview".into(), wow.as_os_str().to_owned()];
            args.extend(flags.into_iter().map(OsString::from));
            let output = run(temp.path(), &args);
            assert!(!output.status.success(), "{:?}", output);
            assert!(output.stdout.is_empty(), "{:?}", output);
            assert!(
                String::from_utf8_lossy(&output.stderr).contains("Choose one preview check mode"),
                "{:?}",
                output
            );
            assert_eq!(snapshot(temp.path()), before);
        }
    }

    #[cfg(not(feature = "no-self-update"))]
    #[test]
    fn safe_entry_without_development_feature_rejects_before_side_effects() {
        let temp = tempfile::tempdir().unwrap();
        prepare(temp.path(), 2);
        let wow = copied_root(temp.path());
        let before = snapshot(temp.path());

        for args in [
            vec!["--startup-check".into()],
            vec![
                "--preview".into(),
                wow.as_os_str().to_owned(),
                "--preview-check".into(),
            ],
            vec![
                "--data".into(),
                temp.path().join("missing-data").into_os_string(),
            ],
        ] {
            let output = run(temp.path(), &args);
            assert!(!output.status.success(), "{:?}", output);
            assert!(output.stdout.is_empty(), "{:?}", output);
            assert!(
                String::from_utf8_lossy(&output.stderr).contains("no-self-update"),
                "{:?}",
                output
            );
            assert_eq!(snapshot(temp.path()), before);
        }
    }
}
