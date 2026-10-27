//! Config-free ownership inspection fixtures; original GPLv3 applies.
use sha2::{Digest, Sha256};
use std::{fs, path::Path, process::Command};
fn snapshot(root: &Path) -> Vec<(std::path::PathBuf, Vec<u8>)> {
    let mut files: Vec<_> = walkdir::WalkDir::new(root)
        .into_iter()
        .map(|e| e.unwrap())
        .filter(|e| e.file_type().is_file())
        .map(|e| {
            (
                e.path().strip_prefix(root).unwrap().to_path_buf(),
                fs::read(e.path()).unwrap(),
            )
        })
        .collect();
    files.sort();
    files
}
#[test]
fn inspect_command_is_read_only_and_rejects_changed_content() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("AddOns");
    fs::create_dir_all(root.join("Foo")).unwrap();
    fs::create_dir(root.join(".patchloom-ownership")).unwrap();
    fs::write(root.join("Foo/a.lua"), b"fixture").unwrap();
    let hash = format!("sha256:{:x}", Sha256::digest(b"fixture"));
    let record = serde_json::json!({"format":1,"root":root.canonicalize().unwrap(),"source":{"repository":"fixture/addon","product":"Retail"},"folders":["Foo"],"files":{"Foo":"directory","Foo/a.lua":hash}});
    let bytes = serde_json::to_vec(&record).unwrap();
    let id = format!("{:x}", Sha256::digest(&bytes));
    fs::write(
        root.join(".patchloom-ownership").join(format!("{id}.json")),
        bytes,
    )
    .unwrap();
    let config = temp.path().join("absent-config");
    let run = || {
        Command::new(env!("CARGO_BIN_EXE_inspect_ownership"))
            .arg(&root)
            .arg(&id)
            .env("APPDATA", &config)
            .env("XDG_CONFIG_HOME", &config)
            .env("HOME", &config)
            .output()
            .unwrap()
    };
    let before = snapshot(temp.path());
    let output = run();
    assert!(output.status.success(), "{:?}", output);
    let preview: serde_yaml::Value = serde_yaml::from_slice(&output.stdout).unwrap();
    assert_eq!(preview["folders"][0].as_str(), Some("Foo"));
    assert_eq!(snapshot(temp.path()), before);
    assert!(!config.exists());
    fs::write(root.join("Foo/a.lua"), b"user-change").unwrap();
    let before = snapshot(temp.path());
    assert!(!run().status.success());
    assert_eq!(snapshot(temp.path()), before);
    assert!(!config.exists());
}
