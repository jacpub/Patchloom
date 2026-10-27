//! Patchloom Windows process tests, 2026-10-26; original Ajour GPLv3 applies.
//! Disposable subprocess tests; never compiled into an application build.
use super::*;
use std::{
    collections::BTreeMap,
    os::windows::{fs::OpenOptionsExt, process::CommandExt},
    path::PathBuf,
    process::{Child, Command},
    thread,
    time::{Duration, Instant},
};
use tempfile::{Builder, TempDir};

const GUARD: &str = "Patchloom disposable process fixture v1";
const CHILD_TEST: &str = "fs::install::process_tests::child_process_entry";
const TIMEOUT: Duration = Duration::from_secs(15);

fn guarded_root() -> PathBuf {
    let root = PathBuf::from(std::env::var_os("PATCHLOOM_PROCESS_ROOT").unwrap())
        .canonicalize()
        .unwrap();
    let parent = root.parent().unwrap();
    assert_eq!(root.file_name().unwrap(), "AddOns");
    assert!(parent.starts_with(std::env::temp_dir().canonicalize().unwrap()));
    assert!(parent
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .starts_with("patchloom-process-"));
    assert_eq!(
        fs::read_to_string(parent.join(".patchloom-process-fixture")).unwrap(),
        GUARD
    );
    root
}

pub(super) fn checkpoint(root: &Path, phase: &str) {
    let Ok(phases) = std::env::var("PATCHLOOM_PROCESS_PHASES") else {
        return;
    };
    if !phases.split(',').any(|wanted| wanted == phase) {
        return;
    }
    assert_eq!(root.canonicalize().unwrap(), guarded_root());
    let events = PathBuf::from(std::env::var_os("PATCHLOOM_PROCESS_EVENTS").unwrap());
    assert_eq!(
        events.parent().unwrap().canonicalize().unwrap(),
        root.parent().unwrap()
    );
    fs::write(events.join(phase), b"paused").unwrap();
    let start = Instant::now();
    while !events.join(format!("resume-{phase}")).exists() {
        assert!(
            start.elapsed() < TIMEOUT,
            "parent did not release checkpoint {}",
            phase
        );
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
#[ignore = "subprocess protocol entry; invoked exactly by the fixture parent"]
fn child_process_entry() {
    let root = guarded_root();
    let action = std::env::var("PATCHLOOM_PROCESS_ACTION").unwrap();
    if action == "restore" || action == "restore-blocked" {
        let result = restore_originals(&root, |_, _| true);
        if action == "restore-blocked" {
            assert!(result.is_err());
        } else {
            result.unwrap();
        }
        return;
    }
    if action.starts_with("recover") {
        let lock = File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(root.join(LOCK))
            .unwrap();
        lock.try_lock()
            .expect("terminated process must release its OS lock");
        let result = recover(&root);
        if action == "recover-blocked" {
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("explicit recovery"));
        } else {
            assert_eq!(action, "recover-ok");
            result.unwrap();
        }
        return;
    }
    let mut addon = Addon::empty("Foo");
    addon.folders.push(AddonFolder {
        id: "Foo".into(),
        path: root.join("Foo"),
        ..Default::default()
    });
    let downloads = root.parent().unwrap().join("downloads");
    let result = if action == "install-owned" {
        async_std::task::block_on(install_addon_checked(
            &addon,
            &downloads,
            &root,
            |staged, folders| {
                crate::fs::ownership::prepare(
                    staged,
                    &root,
                    serde_json::json!({"repository":"fixture/addon","product":"Retail"}),
                )?;
                Ok(folders)
            },
        ))
    } else {
        async_std::task::block_on(install_addon(&addon, &downloads, &root))
    };
    match action.as_str() {
        "install" | "install-owned" => {
            result.unwrap();
        }
        "install-busy" => assert!(result
            .unwrap_err()
            .to_string()
            .contains("Another install is active")),
        "install-fails" => {
            let error = result.unwrap_err();
            fs::write(
                root.parent().unwrap().join("expected-install-error.txt"),
                error.to_string(),
            )
            .unwrap();
        }
        _ => panic!("unknown child action"),
    }
}

fn fixture() -> (TempDir, PathBuf) {
    let temp = Builder::new()
        .prefix("patchloom-process-")
        .tempdir()
        .unwrap();
    fs::write(temp.path().join(".patchloom-process-fixture"), GUARD).unwrap();
    let root = temp.path().join("AddOns");
    fs::create_dir_all(root.join("Foo")).unwrap();
    fs::write(root.join("Foo/old.lua"), b"original").unwrap();
    fs::create_dir(root.join("Unrelated")).unwrap();
    fs::write(root.join("Unrelated/keep.lua"), b"keep").unwrap();
    let downloads = temp.path().join("downloads");
    fs::create_dir(&downloads).unwrap();
    super::tests::archive(
        &downloads.join("Foo"),
        &[("Foo/Foo.toc", b"## Title: Foo"), ("Foo/new.lua", b"new")],
    );
    (temp, root.canonicalize().unwrap())
}

struct Process {
    child: Child,
    events: PathBuf,
    log: PathBuf,
}

impl Process {
    fn start(root: &Path, name: &str, action: &str, phases: &str) -> Self {
        let events = root.parent().unwrap().join(format!("events-{name}"));
        fs::create_dir(&events).unwrap();
        let log = events.join("child.log");
        let output = File::create(&log).unwrap();
        let child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                CHILD_TEST,
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("PATCHLOOM_PROCESS_ROOT", root)
            .env("PATCHLOOM_PROCESS_EVENTS", &events)
            .env("PATCHLOOM_PROCESS_ACTION", action)
            .env("PATCHLOOM_PROCESS_PHASES", phases)
            .stdout(output.try_clone().unwrap())
            .stderr(output)
            .creation_flags(0x08000000)
            .spawn()
            .unwrap();
        Self { child, events, log }
    }

    fn paused(&mut self, phase: &str) {
        let start = Instant::now();
        while !self.events.join(phase).exists() {
            assert!(
                self.child.try_wait().unwrap().is_none(),
                "child exited before {}: {}",
                phase,
                self.logs()
            );
            assert!(
                start.elapsed() < TIMEOUT,
                "child checkpoint timed out: {}",
                self.logs()
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn resume(&self, phase: &str) {
        fs::write(self.events.join(format!("resume-{phase}")), b"resume").unwrap();
    }

    fn stop(&mut self) {
        self.child.kill().unwrap();
        assert!(
            !self.child.wait().unwrap().success(),
            "child was not forcibly terminated"
        );
    }

    fn success(&mut self) {
        let start = Instant::now();
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                assert!(status.success(), "child failed: {}", self.logs());
                return;
            }
            assert!(
                start.elapsed() < TIMEOUT,
                "child exit timed out: {}",
                self.logs()
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn logs(&self) -> String {
        fs::read_to_string(&self.log).unwrap_or_default()
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

fn snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    WalkDir::new(root)
        .into_iter()
        .map(|entry| entry.unwrap())
        .filter(|entry| {
            entry.file_type().is_file()
                && entry.path().strip_prefix(root).unwrap() != Path::new(LOCK)
        })
        .map(|entry| {
            (
                entry.path().strip_prefix(root).unwrap().to_path_buf(),
                fs::read(entry.path()).unwrap(),
            )
        })
        .collect()
}

#[test]
fn cross_process_contention_and_lock_release_after_forced_stop() {
    let (_temp, root) = fixture();
    let mut first = Process::start(&root, "first", "install", "locked");
    first.paused("locked");
    let before = snapshot(&root);
    Process::start(&root, "contender", "install-busy", "").success();
    assert_eq!(snapshot(&root), before);
    first.stop();
    Process::start(&root, "retry", "install", "").success();
    assert_eq!(fs::read(root.join("Foo/new.lua")).unwrap(), b"new");
    assert_eq!(fs::read(root.join("Unrelated/keep.lua")).unwrap(), b"keep");
    assert!(!root.join(TRANSACTION).exists());
}

#[test]
fn ownership_publication_survives_forced_stop_before_backup_cleanup() {
    for phase in ["committed", "ownership_published", "cleanup_backup"] {
        let (_temp, root) = fixture();
        let mut installer = Process::start(&root, "owned", "install-owned", phase);
        installer.paused(phase);
        let id = fs::read_to_string(root.join(TRANSACTION).join("receipt.id")).unwrap();
        if phase == "committed" {
            assert!(root.join(TRANSACTION).join("backup/Foo/old.lua").exists());
        }
        installer.stop();
        Process::start(&root, "recover-owned", "recover-ok", "").success();
        let preview = crate::fs::inspect_ownership(&root, &id).unwrap();
        assert_eq!(preview.folders, ["Foo"]);
        assert_eq!(fs::read(root.join("Foo/new.lua")).unwrap(), b"new");
        assert_eq!(fs::read(root.join("Unrelated/keep.lua")).unwrap(), b"keep");
        assert!(!root.join(TRANSACTION).exists());
    }
}

#[test]
fn interrupted_owned_preparation_or_backup_does_not_publish_a_receipt() {
    for phase in ["ready", "backup"] {
        let (_temp, root) = fixture();
        let mut installer = Process::start(&root, "owned-before-commit", "install-owned", phase);
        installer.paused(phase);
        let before = snapshot(&root);
        Process::start(&root, "contender", "install-busy", "").success();
        assert_eq!(snapshot(&root), before);
        installer.stop();
        Process::start(&root, "recover-original", "recover-ok", "").success();
        assert_eq!(fs::read(root.join("Foo/old.lua")).unwrap(), b"original");
        assert!(!root.join("Foo/new.lua").exists());
        assert!(!root.join(".patchloom-ownership").exists());
        assert!(!root.join(TRANSACTION).exists());
    }
}

#[test]
fn explicit_restore_resumes_after_forced_stop_without_losing_versions() {
    for phase in ["restore-retained", "restore-original"] {
        let (_temp, root) = fixture();
        let mut installer = Process::start(&root, "interrupted-install", "install", "published");
        installer.paused("published");
        installer.stop();
        let transaction = root.join(TRANSACTION);
        let mut plan = read_plan(&transaction).unwrap();
        plan.previous.push("Bar".into());
        plan.incoming.push("Bar".into());
        fs::write(
            transaction.join("plan.yml"),
            serde_yaml::to_string(&plan).unwrap(),
        )
        .unwrap();
        fs::create_dir(transaction.join("backup/Bar")).unwrap();
        fs::write(transaction.join("backup/Bar/old.lua"), b"original-bar").unwrap();
        fs::create_dir(root.join("Bar")).unwrap();
        fs::write(root.join("Bar/new.lua"), b"new-bar").unwrap();
        let mut restore = Process::start(&root, "interrupted-restore", "restore", phase);
        restore.paused(phase);
        restore.stop();
        Process::start(
            &root,
            "resume-restore",
            if phase == "restore-retained" {
                "restore"
            } else {
                "restore-blocked"
            },
            "",
        )
        .success();
        assert_eq!(fs::read(root.join("Foo/old.lua")).unwrap(), b"original");
        if phase == "restore-retained" {
            assert_eq!(fs::read(root.join("Bar/old.lua")).unwrap(), b"original-bar");
        } else {
            assert!(!root.join("Bar").exists());
            assert_eq!(
                fs::read(transaction.join("backup/Bar/old.lua")).unwrap(),
                b"original-bar"
            );
        }
        assert_eq!(
            fs::read(root.join(RESTORE).join("backup/Bar/new.lua")).unwrap(),
            b"new-bar"
        );
        assert_eq!(
            fs::read(root.join(RESTORE).join("backup/Foo/new.lua")).unwrap(),
            b"new"
        );
        assert_eq!(fs::read(root.join("Unrelated/keep.lua")).unwrap(), b"keep");
        assert!(recover(&root).is_err());
    }
}

#[test]
fn forced_stop_at_install_and_cleanup_boundaries() {
    for phase in [
        "ready",
        "backup",
        "published",
        "committed",
        "cleanup_staged",
        "cleanup_backup",
    ] {
        let (_temp, root) = fixture();
        let mut updater = Process::start(&root, "updater", "install", phase);
        updater.paused(phase);
        updater.stop();
        assert_eq!(fs::read(root.join("Unrelated/keep.lua")).unwrap(), b"keep");
        assert!(root.parent().unwrap().join("downloads/Foo").exists());
        let before = snapshot(&root);
        if phase == "published" {
            Process::start(&root, "recovery", "recover-blocked", "").success();
            assert_eq!(snapshot(&root), before);
            assert_eq!(
                fs::read(root.join(TRANSACTION).join("backup/Foo/old.lua")).unwrap(),
                b"original"
            );
            assert_eq!(fs::read(root.join("Foo/new.lua")).unwrap(), b"new");
        } else {
            Process::start(&root, "recovery", "recover-ok", "").success();
            let committed = matches!(phase, "committed" | "cleanup_staged" | "cleanup_backup");
            let (file, expected) = if committed {
                ("Foo/new.lua", b"new".as_slice())
            } else {
                ("Foo/old.lua", b"original".as_slice())
            };
            assert_eq!(
                fs::read(root.join(file)).unwrap(),
                expected,
                "phase {}",
                phase
            );
            assert!(!root.join(TRANSACTION).exists());
        }
        println!(
            "forced stop at {}: expected data retained; next process acquired lock",
            phase
        );
    }
}

// Holding ready without FILE_SHARE_DELETE makes the final rename fail on Windows.
fn block_commit(root: &Path, updater: &mut Process) -> File {
    updater.paused("before_commit");
    let ready = File::options()
        .read(true)
        .share_mode(0x1 | 0x2)
        .open(root.join(TRANSACTION).join("ready"))
        .unwrap();
    updater.resume("before_commit");
    ready
}

#[test]
fn forced_stop_during_rollback_preserves_originals() {
    for phase in ["rollback_removed", "rollback_restored"] {
        let (_temp, root) = fixture();
        let phases = format!("before_commit,{phase}");
        let mut updater = Process::start(&root, "updater", "install-fails", &phases);
        let ready = block_commit(&root, &mut updater);
        updater.paused(phase);
        updater.stop();
        drop(ready);
        if phase == "rollback_removed" {
            assert_eq!(
                fs::read(root.join(TRANSACTION).join("backup/Foo/old.lua")).unwrap(),
                b"original"
            );
            Process::start(&root, "recovery", "recover-ok", "").success();
        } else {
            let before = snapshot(&root);
            Process::start(&root, "recovery", "recover-blocked", "").success();
            assert_eq!(snapshot(&root), before);
        }
        assert_eq!(fs::read(root.join("Foo/old.lua")).unwrap(), b"original");
        assert_eq!(fs::read(root.join("Unrelated/keep.lua")).unwrap(), b"keep");
        println!("forced stop at {}: original survived; lock released", phase);
    }
}

#[test]
fn final_marker_rename_failure_restores_original_before_child_exit() {
    let (_temp, root) = fixture();
    let mut updater = Process::start(&root, "updater", "install-fails", "before_commit");
    let ready = block_commit(&root, &mut updater);
    updater.success();
    drop(ready);
    assert_eq!(fs::read(root.join("Foo/old.lua")).unwrap(), b"original");
    assert!(!root.join("Foo/new.lua").exists());
    assert!(root.parent().unwrap().join("downloads/Foo").exists());
    let error =
        fs::read_to_string(root.parent().unwrap().join("expected-install-error.txt")).unwrap();
    assert!(error.contains("Install failed"), "{}", error);
    assert!(root.join(TRANSACTION).exists()); // The held marker also prevented cleanup; no data guessed at.
}
