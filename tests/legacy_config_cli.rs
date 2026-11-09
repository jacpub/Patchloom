//! Explicit copied-config CLI entry regressions, 2026-11-09; GPLv3 applies.
use std::{
    fs,
    path::Path,
    process::{Command, Stdio},
};

fn child_command(program: &str) -> Command {
    let mut command = Command::new(program);
    command.stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    command
}

fn fixture() -> tempfile::TempDir {
    #[cfg(unix)]
    {
        // Resolve only our owned fixture-root alias, not a supplied source leaf.
        tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap()
    }
    #[cfg(not(unix))]
    {
        tempfile::tempdir().unwrap()
    }
}

fn run(root: &Path, copy: &Path, extra: &[&str]) -> std::process::Output {
    child_command(env!("CARGO_BIN_EXE_ajour"))
        .arg("--inspect-legacy-config")
        .arg(copy)
        .args(extra)
        .env("APPDATA", root.join("must-not-exist"))
        .env("LOCALAPPDATA", root.join("must-not-exist"))
        .env("XDG_CONFIG_HOME", root.join("must-not-exist"))
        .env("HOME", root.join("must-not-exist"))
        .output()
        .unwrap()
}

#[test]
fn explicit_copy_reports_hints_without_legacy_startup_or_source_changes() {
    let temp = fixture();
    let copy = temp.path().join("settings-copy.yml");
    let bytes = b"wow:\n  directory: 'Z:/never-probe-this/_classic_'\n  flavor: Classic\napi_key: private-fixture-sentinel\n";
    fs::write(&copy, bytes).unwrap();
    let output = run(temp.path(), &copy, &[]);
    assert!(output.status.success(), "{:?}", output);
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["Present"]["source_bytes"], bytes.len());
    assert_eq!(
        report["Present"]["directories"].as_array().unwrap().len(),
        1
    );
    assert!(!String::from_utf8_lossy(&output.stdout).contains("private-fixture-sentinel"));
    assert!(output.stderr.is_empty());
    assert_eq!(fs::read(&copy).unwrap(), bytes);
    assert!(!temp.path().join("must-not-exist").exists());
    assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
}

#[test]
fn malformed_unsupported_empty_and_oversized_copies_fail_without_echoing_values() {
    let temp = fixture();
    let copy = temp.path().join("copy.yml");
    for bytes in [
        b"wow: [private-fixture-sentinel".to_vec(),
        b"version: 1\nwow: {}\napi_key: private-fixture-sentinel\n".to_vec(),
        vec![],
        vec![b'x'; 65537],
    ] {
        fs::write(&copy, &bytes).unwrap();
        let output = run(temp.path(), &copy, &[]);
        assert_eq!(output.status.code(), Some(1), "{:?}", output);
        assert!(output.stdout.is_empty());
        assert!(!String::from_utf8_lossy(&output.stderr).contains("private-fixture-sentinel"));
        assert!(output.stderr.len() < 512);
        assert_eq!(fs::read(&copy).unwrap(), bytes);
        assert!(!temp.path().join("must-not-exist").exists());
    }
}

#[test]
fn missing_directory_and_parent_component_inputs_are_not_opened_or_created() {
    let temp = fixture();
    let copy = temp.path().join("copy.yml");
    fs::write(&copy, b"wow: {}\n").unwrap();
    for path in [
        temp.path().join("absent.yml"),
        temp.path().to_path_buf(),
        temp.path().join("missing/../copy.yml"),
    ] {
        let output = run(temp.path(), &path, &[]);
        assert_eq!(output.status.code(), Some(1), "{:?}", output);
        assert!(output.stdout.is_empty());
        assert!(output.stderr.len() < 512);
    }
    assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
    assert_eq!(fs::read(&copy).unwrap(), b"wow: {}\n");
}

#[test]
fn conflicting_startup_options_and_commands_never_enter_their_routes() {
    let temp = fixture();
    let copy = temp.path().join("copy.yml");
    fs::write(&copy, b"wow: {}\n").unwrap();
    for args in [
        vec!["--preview", "Z:/must-not-probe"],
        vec!["--preview", "Z:/must-not-probe", "--preview-window-check"],
        vec!["--startup-check"],
        vec!["--data", "Z:/must-not-create"],
        vec!["--aa", "true"],
        vec!["--self-update-temp", "Z:/must-not-rename"],
        vec!["update"],
        vec!["path-add", "Z:/must-not-probe"],
    ] {
        let output = run(temp.path(), &copy, &args);
        assert!(!output.status.success(), "{:?}", output);
        assert!(output.stdout.is_empty());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("Use --inspect-legacy-config"),
            "{:?}",
            output
        );
        assert!(!temp.path().join("must-not-exist").exists());
    }
    assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
}

#[test]
fn explicit_source_is_required_and_the_help_is_safe_to_request() {
    let temp = fixture();
    let output = child_command(env!("CARGO_BIN_EXE_ajour"))
        .arg("--inspect-legacy-config")
        .env("APPDATA", temp.path().join("must-not-exist"))
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(!temp.path().join("must-not-exist").exists());
    let output = run(temp.path(), &temp.path().join("missing.yml"), &["--help"]);
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("--inspect-legacy-config"));
    assert!(!temp.path().join("must-not-exist").exists());
}

#[test]
fn exact_read_limit_is_accepted_without_truncation_and_relative_copy_is_explicit() {
    let temp = fixture();
    let mut bytes = b"wow: {}\n#".to_vec();
    bytes.resize(65536, b'x');
    let copy = temp.path().join("copy.yml");
    fs::write(&copy, &bytes).unwrap();
    let output = child_command(env!("CARGO_BIN_EXE_ajour"))
        .args(["--inspect-legacy-config", "copy.yml"])
        .current_dir(temp.path())
        .env("APPDATA", temp.path().join("must-not-exist"))
        .env("LOCALAPPDATA", temp.path().join("must-not-exist"))
        .env("XDG_CONFIG_HOME", temp.path().join("must-not-exist"))
        .env("HOME", temp.path().join("must-not-exist"))
        .output()
        .unwrap();
    assert!(output.status.success(), "{:?}", output);
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["Present"]["source_bytes"], 65536);
    assert_eq!(fs::read(&copy).unwrap(), bytes);
    assert!(!temp.path().join("must-not-exist").exists());
}

#[cfg(windows)]
#[test]
fn windows_sharing_failure_and_alternate_stream_do_not_fall_back_to_defaults() {
    use std::os::windows::fs::OpenOptionsExt;
    let temp = fixture();
    let copy = temp.path().join("copy.yml");
    fs::write(&copy, b"wow: {}\n").unwrap();
    let held = fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&copy)
        .unwrap();
    let output = run(temp.path(), &copy, &[]);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    drop(held);
    let stream = temp.path().join("copy.yml:private-stream");
    let output = run(temp.path(), &stream, &[]);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert_eq!(fs::read(&copy).unwrap(), b"wow: {}\n");
    assert!(!temp.path().join("must-not-exist").exists());
}

#[cfg(unix)]
#[test]
fn unix_linked_sources_ancestors_and_special_files_are_refused() {
    use std::os::{unix::fs::symlink, unix::net::UnixListener};
    let temp = fixture();
    let plain = temp.path().join("plain");
    fs::create_dir(&plain).unwrap();
    let copy = plain.join("copy.yml");
    fs::write(&copy, b"wow: {}\n").unwrap();
    let link = temp.path().join("link.yml");
    symlink(&copy, &link).unwrap();
    let ancestor = temp.path().join("linked-parent");
    symlink(&plain, &ancestor).unwrap();
    let dangling = temp.path().join("dangling");
    symlink(temp.path().join("missing"), &dangling).unwrap();
    let special = temp.path().join("socket");
    let _listener = UnixListener::bind(&special).unwrap();
    for path in [link, ancestor.join("copy.yml"), dangling, special] {
        let output = run(temp.path(), &path, &[]);
        assert_eq!(output.status.code(), Some(1), "{:?}", output);
        assert!(output.stdout.is_empty());
    }
    assert_eq!(fs::read(&copy).unwrap(), b"wow: {}\n");
    assert!(!temp.path().join("must-not-exist").exists());
}

#[cfg(windows)]
#[test]
fn windows_junction_ancestor_is_refused_without_reading_its_copy() {
    let temp = fixture();
    let plain = temp.path().join("plain");
    fs::create_dir(&plain).unwrap();
    let copy = plain.join("copy.yml");
    fs::write(&copy, b"wow: {}\n").unwrap();
    let junction = temp.path().join("linked-parent");
    let output = child_command("cmd.exe")
        .args(["/d", "/c", "mklink", "/J"])
        .arg(&junction)
        .arg(&plain)
        .output()
        .unwrap();
    assert!(output.status.success(), "{:?}", output);
    let output = run(temp.path(), &junction.join("copy.yml"), &[]);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert_eq!(fs::read(&copy).unwrap(), b"wow: {}\n");
    assert!(!temp.path().join("must-not-exist").exists());
}
