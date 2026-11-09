//! Disposable CLI recovery inspection checks; original Ajour GPLv3 applies.
//! Modified 2026-11-03; Router proposal integrated locally.
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Child, Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
};

const PROMPT: &str = "Type RESTORE to proceed, or press Enter to cancel: ";

struct Cli {
    child: Child,
    logs: tempfile::TempDir,
}

impl Cli {
    fn start(executable: &str, root: &Path, temp: &Path) -> Self {
        let logs = tempfile::tempdir().unwrap();
        let stdout = fs::File::create(logs.path().join("stdout")).unwrap();
        let stderr = fs::File::create(logs.path().join("stderr")).unwrap();
        let config = temp.join("unused-config");
        let child = Command::new(executable)
            .arg(root)
            .current_dir(temp)
            .stdin(Stdio::piped())
            .stdout(stdout)
            .stderr(stderr)
            .env("HOME", &config)
            .env("USERPROFILE", &config)
            .env("APPDATA", &config)
            .env("LOCALAPPDATA", &config)
            .env("XDG_CONFIG_HOME", &config)
            .spawn()
            .unwrap();
        Self { child, logs }
    }

    fn stdout(&self) -> String {
        fs::read_to_string(self.logs.path().join("stdout")).unwrap()
    }

    fn prompt(&mut self) -> String {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let text = self.stdout();
            assert!(
                self.child.try_wait().unwrap().is_none(),
                "CLI exited before prompt: {}",
                text
            );
            if text.ends_with(PROMPT) {
                return text;
            }
            assert!(Instant::now() < deadline, "CLI prompt timed out: {}", text);
            thread::sleep(Duration::from_millis(5));
        }
    }

    fn answer(&mut self, answer: &[u8]) {
        let mut stdin = self.child.stdin.take().unwrap();
        stdin.write_all(answer).unwrap();
        stdin.flush().unwrap();
    }

    fn finish(&mut self) -> Output {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return Output {
                    status,
                    stdout: fs::read(self.logs.path().join("stdout")).unwrap(),
                    stderr: fs::read(self.logs.path().join("stderr")).unwrap(),
                };
            }
            assert!(
                Instant::now() < deadline,
                "CLI exit timed out: {}",
                self.stdout()
            );
            thread::sleep(Duration::from_millis(5));
        }
    }
}

impl Drop for Cli {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_some() {
            return;
        }
        let _ = self.child.kill();
        let deadline = Instant::now() + Duration::from_secs(5);
        while self.child.try_wait().ok().flatten().is_none() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
    }
}

fn restore_cli(root: &Path, temp: &Path) -> Cli {
    Cli::start(env!("CARGO_BIN_EXE_restore_originals"), root, temp)
}

fn fixture() -> (tempfile::TempDir, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("AddOns");
    let transaction = root.join(".patchloom-install");
    fs::create_dir_all(transaction.join("backup/Foo")).unwrap();
    fs::create_dir_all(transaction.join("backup/Bar")).unwrap();
    fs::create_dir(transaction.join("staged")).unwrap();
    for (name, bytes) in [
        ("Foo", b"replacement".as_slice()),
        ("Bar", b"replacement-bar"),
        ("New", b"additional"),
    ] {
        fs::create_dir(root.join(name)).unwrap();
        fs::write(root.join(name).join("new.lua"), bytes).unwrap();
    }
    fs::write(root.join(".patchloom-install.lock"), b"").unwrap();
    fs::write(
        transaction.join("plan.yml"),
        b"version: 1\nprevious: [Foo, Bar]\nincoming: [Foo, New]\n",
    )
    .unwrap();
    fs::write(transaction.join("ready"), b"").unwrap();
    fs::write(transaction.join("backup/Foo/old.lua"), b"original").unwrap();
    fs::write(transaction.join("backup/Bar/old.lua"), b"original-bar").unwrap();
    (temp, root)
}

fn snapshot(root: &Path) -> BTreeMap<PathBuf, (bool, Vec<u8>)> {
    walkdir::WalkDir::new(root)
        .into_iter()
        .map(|entry| {
            let entry = entry.unwrap();
            let directory = entry.file_type().is_dir();
            (
                entry.path().strip_prefix(root).unwrap().to_path_buf(),
                (
                    directory,
                    if directory {
                        vec![]
                    } else {
                        fs::read(entry.path()).unwrap()
                    },
                ),
            )
        })
        .collect()
}

fn assert_preview(text: &str, root: &Path) {
    assert!(text.contains(&format!("AddOns root: {:?}", root.canonicalize().unwrap())));
    assert!(text.contains("Restore originals: [\"Foo\", \"Bar\"]"));
    assert!(text.contains("Current folder names checked by this plan: [\"Bar\", \"Foo\", \"New\"]"));
}

#[test]
fn restore_command_requires_explicit_choice_and_preserves_replacement() {
    let (temp, root) = fixture();
    for answer in [b"\n".as_slice(), b"restore\n", b"RESTORE later\n", b""] {
        let before = snapshot(temp.path());
        let mut child = restore_cli(&root, temp.path());
        assert_preview(&child.prompt(), &root);
        let lock = fs::File::options()
            .read(true)
            .write(true)
            .open(root.join(".patchloom-install.lock"))
            .unwrap();
        lock.try_lock().unwrap();
        child.answer(answer);
        let output = child.finish();
        drop(lock);
        assert!(
            output.status.success(),
            "cancellation must not attempt a locked restore"
        );
        assert!(String::from_utf8_lossy(&output.stdout)
            .contains("Cancelled. No addon folders changed."));
        assert_eq!(snapshot(temp.path()), before);
    }

    let journal = fs::read(root.join(".patchloom-install/plan.yml")).unwrap();
    let mut child = restore_cli(&root, temp.path());
    assert_preview(&child.prompt(), &root);
    child.answer(b"RESTORE\n");
    let output = child.finish();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("Originals restored."));
    assert!(text.contains("Replacement versions and recovery journals are retained; updates remain blocked pending cleanup."));
    assert_eq!(fs::read(root.join("Foo/old.lua")).unwrap(), b"original");
    assert_eq!(fs::read(root.join("Bar/old.lua")).unwrap(), b"original-bar");
    for (name, bytes) in [
        ("Foo", b"replacement".as_slice()),
        ("Bar", b"replacement-bar"),
        ("New", b"additional"),
    ] {
        assert_eq!(
            fs::read(
                root.join(".patchloom-restore/backup")
                    .join(name)
                    .join("new.lua")
            )
            .unwrap(),
            bytes
        );
    }
    assert!(!root.join("New").exists());
    assert_eq!(
        fs::read(root.join(".patchloom-install/plan.yml")).unwrap(),
        journal
    );
    assert!(root.join(".patchloom-install/ready").is_file());
    assert!(root.join(".patchloom-restore/committed").is_file());
    assert!(!temp.path().join("unused-config").exists());
}

#[test]
fn restore_command_refuses_content_changes_after_preview() {
    for location in 0..3 {
        let (temp, root) = fixture();
        let mut child = restore_cli(&root, temp.path());
        assert_preview(&child.prompt(), &root);
        let path = match location {
            0 => root.join(".patchloom-install/backup/Foo/old.lua"),
            1 => root.join("Foo/new.lua"),
            2 => root.join(".patchloom-install/plan.yml"),
            _ => unreachable!(),
        };
        let original_plan: serde_yaml::Value =
            serde_yaml::from_slice(&fs::read(root.join(".patchloom-install/plan.yml")).unwrap())
                .unwrap();
        if location == 2 {
            fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .unwrap()
                .write_all(b"\n# same parsed plan, changed raw bytes\n")
                .unwrap();
            let current_plan: serde_yaml::Value =
                serde_yaml::from_slice(&fs::read(&path).unwrap()).unwrap();
            assert_eq!(current_plan, original_plan);
        } else {
            let mut bytes = fs::read(&path).unwrap();
            bytes[0] ^= 1;
            fs::write(&path, bytes).unwrap();
        }
        let before = snapshot(temp.path());
        child.answer(b"RESTORE\n");
        let output = child.finish();
        assert!(!output.status.success());
        let text = String::from_utf8_lossy(&output.stderr);
        assert!(text.contains("Recovery changed since preview."));
        assert!(text.contains("fresh preview"));
        assert!(!String::from_utf8_lossy(&output.stdout).contains("Originals restored."));
        assert!(!root.join(".patchloom-restore").exists());
        assert_eq!(snapshot(temp.path()), before);
        assert_eq!(
            fs::read(root.join(".patchloom-install/backup/Bar/old.lua")).unwrap(),
            b"original-bar"
        );
    }
}

#[test]
fn restore_command_fails_before_prompt_for_missing_journal_or_busy_lock() {
    for state in 0..3 {
        let (temp, root) = fixture();
        match state {
            0 => fs::rename(
                root.join(".patchloom-install"),
                temp.path().join("retained-journal"),
            )
            .unwrap(),
            1 => fs::remove_file(root.join(".patchloom-install.lock")).unwrap(),
            2 => {}
            _ => unreachable!(),
        }
        let before = snapshot(temp.path());
        let lock = if state == 2 {
            let file = fs::File::options()
                .read(true)
                .write(true)
                .open(root.join(".patchloom-install.lock"))
                .unwrap();
            file.try_lock().unwrap();
            Some(file)
        } else {
            None
        };
        let output = restore_cli(&root, temp.path()).finish();
        drop(lock);
        assert!(!output.status.success());
        assert!(!String::from_utf8_lossy(&output.stdout).contains(PROMPT));
        assert!(!String::from_utf8_lossy(&output.stdout).contains("Originals restored."));
        assert_eq!(snapshot(temp.path()), before);
    }
}

#[test]
fn restore_prompt_releases_lock_and_checked_restore_refuses_held_lock() {
    let (temp, root) = fixture();
    let before = snapshot(temp.path());
    let mut child = restore_cli(&root, temp.path());
    assert_preview(&child.prompt(), &root);
    let lock = fs::File::options()
        .read(true)
        .write(true)
        .open(root.join(".patchloom-install.lock"))
        .unwrap();
    lock.try_lock()
        .expect("CLI must release its OS lock before waiting for input");
    child.answer(b"RESTORE\n");
    let output = child.finish();
    drop(lock);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("busy"));
    assert!(stderr.contains("a failed restore may have moved some folders"));
    assert!(!String::from_utf8_lossy(&output.stdout).contains("Originals restored."));
    assert!(!root.join(".patchloom-restore").exists());
    assert_eq!(snapshot(temp.path()), before);
}

#[test]
fn command_reports_retained_versions_and_rejects_bad_journal_without_writes() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("AddOns");
    fs::create_dir_all(root.join("Foo")).unwrap();
    fs::write(root.join("Foo/new.lua"), b"replacement").unwrap();
    let inspect = |path: &Path| {
        Cli::start(env!("CARGO_BIN_EXE_inspect_recovery"), path, temp.path()).finish()
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
