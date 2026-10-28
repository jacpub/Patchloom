//! Disposable CLI recovery inspection checks; original Ajour GPLv3 applies.
use std::{fs, path::Path, process::Command};

#[test]
fn restore_command_requires_explicit_choice_and_preserves_replacement() {
    use std::{io::Write, process::Stdio};
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("AddOns");
    let transaction = root.join(".patchloom-install");
    fs::create_dir_all(transaction.join("backup/Foo")).unwrap();
    fs::create_dir(transaction.join("staged")).unwrap();
    fs::create_dir(root.join("Foo")).unwrap();
    fs::write(root.join(".patchloom-install.lock"), b"").unwrap();
    fs::write(
        transaction.join("plan.yml"),
        b"version: 1\nprevious: [Foo]\nincoming: [Foo]\n",
    )
    .unwrap();
    fs::write(transaction.join("ready"), b"").unwrap();
    fs::write(transaction.join("backup/Foo/old.lua"), b"original").unwrap();
    fs::write(root.join("Foo/new.lua"), b"replacement").unwrap();
    let run = |answer: &[u8]| {
        let mut child = Command::new(env!("CARGO_BIN_EXE_restore_originals"))
            .arg(&root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("APPDATA", temp.path().join("unused-config"))
            .env("XDG_CONFIG_HOME", temp.path().join("unused-config"))
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(answer).unwrap();
        child.wait_with_output().unwrap()
    };
    assert!(run(b"\n").status.success());
    assert!(!root.join(".patchloom-restore").exists());
    assert!(root.join("Foo/new.lua").exists());
    let output = run(b"RESTORE\n");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(fs::read(root.join("Foo/old.lua")).unwrap(), b"original");
    assert_eq!(
        fs::read(root.join(".patchloom-restore/backup/Foo/new.lua")).unwrap(),
        b"replacement"
    );
    assert!(!temp.path().join("unused-config").exists());
}

#[test]
fn command_reports_retained_versions_and_rejects_bad_journal_without_writes() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("AddOns");
    fs::create_dir_all(root.join("Foo")).unwrap();
    fs::write(root.join("Foo/new.lua"), b"replacement").unwrap();
    let inspect = |path: &Path| {
        Command::new(env!("CARGO_BIN_EXE_inspect_recovery"))
            .arg(path)
            .env("APPDATA", temp.path().join("unused-config"))
            .env("XDG_CONFIG_HOME", temp.path().join("unused-config"))
            .output()
            .unwrap()
    };
    let clean = inspect(&root);
    assert!(clean.status.success());
    assert!(String::from_utf8(clean.stdout)
        .unwrap()
        .contains("phase: none"));
    assert_eq!(fs::read_dir(&root).unwrap().count(), 1);

    let transaction = root.join(".patchloom-install");
    fs::create_dir_all(transaction.join("backup/Foo")).unwrap();
    fs::create_dir(transaction.join("staged")).unwrap();
    fs::write(transaction.join("backup/Foo/old.lua"), b"original").unwrap();
    let plan = b"version: 1\nprevious: [Foo]\nincoming: [Foo]\n";
    fs::write(transaction.join("plan.yml"), plan).unwrap();
    fs::write(transaction.join("ready"), b"").unwrap();
    let output = inspect(&root);
    assert!(output.status.success());
    let report: serde_yaml::Value = serde_yaml::from_slice(&output.stdout).unwrap();
    assert_eq!(report["phase"].as_str(), Some("ready-marker"));
    assert_eq!(report["backups"][0].as_str(), Some("Foo"));
    assert_eq!(report["destinations"][0].as_str(), Some("Foo"));
    assert_eq!(fs::read(transaction.join("plan.yml")).unwrap(), plan);
    assert!(!root.join(".patchloom-install.lock").exists());

    fs::write(transaction.join("plan.yml"), b"version: [invalid").unwrap();
    assert!(!inspect(&root).status.success());
    assert_eq!(fs::read(root.join("Foo/new.lua")).unwrap(), b"replacement");
    assert_eq!(
        fs::read(transaction.join("backup/Foo/old.lua")).unwrap(),
        b"original"
    );
    assert_eq!(
        fs::read(transaction.join("plan.yml")).unwrap(),
        b"version: [invalid"
    );
    assert!(!temp.path().join("unused-config").exists());
}
