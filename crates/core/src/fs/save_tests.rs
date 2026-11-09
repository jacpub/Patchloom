// Disposable configuration preservation regressions, 2026-11-09; GPLv3 applies.
use super::*;
use crate::config::Config;
use std::process::Command;

const MALFORMED: &[u8] = b"theme: [unterminated\nwow: {\n";

fn fixture() -> tempfile::TempDir {
    let fixture = tempfile::Builder::new()
        .prefix("patchloom-config-preservation-")
        .tempdir()
        .unwrap();
    fs::write(
        fixture.path().join("fixture-marker"),
        b"disposable config fixture",
    )
    .unwrap();
    fixture
}

fn assert_child(root: &Path, scenario: &str) {
    let result = fixture_child(root, scenario);
    assert!(
        result.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
}

fn fixture_child(root: &std::path::Path, scenario: &str) -> std::process::Output {
    // Resolve the OS's temporary-directory aliases (e.g. macOS /var) before
    // testing links deliberately created inside the owned fixture.
    let root = root.canonicalize().unwrap();
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--ignored",
            "--exact",
            "fs::save::tests::configuration_child",
            "--nocapture",
        ])
        .env("PATCHLOOM_CONFIG_TEST_ROOT", root)
        .env("PATCHLOOM_CONFIG_TEST_CASE", scenario);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    command.output().unwrap()
}

#[test]
fn malformed_configuration_is_preserved_instead_of_replaced_with_defaults() {
    let fixture = fixture();
    fs::create_dir(fixture.path().join("settings")).unwrap();
    let path = fixture.path().join("settings/ajour.yml");
    fs::write(&path, MALFORMED).unwrap();
    let result = fixture_child(fixture.path(), "malformed");
    assert_eq!(
        fs::read(&path).unwrap(),
        MALFORMED,
        "existing configuration was overwritten"
    );
    assert!(
        result.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
fn empty_truncated_and_invalid_field_configurations_are_preserved() {
    for bytes in [
        b"".as_slice(),
        b"wow: { directories: [\n".as_slice(),
        b"auto_update: not-a-boolean\n".as_slice(),
    ] {
        let fixture = fixture();
        fs::create_dir(fixture.path().join("settings")).unwrap();
        let path = fixture.path().join("settings/ajour.yml");
        fs::write(&path, bytes).unwrap();
        assert_child(fixture.path(), "malformed");
        assert_eq!(fs::read(path).unwrap(), bytes);
    }
}

#[test]
fn valid_configuration_loads_without_reserialization_or_directory_discovery() {
    let fixture = fixture();
    fs::create_dir(fixture.path().join("settings")).unwrap();
    let bytes = b"# preserve this comment and formatting\ntheme: 'fixture theme'\nbackup_wtf: true\nwow:\n  directories:\n    Retail: 'not-a-real-game/_retail_'\nfuture_unknown_field: untouched\n";
    let path = fixture.path().join("settings/ajour.yml");
    fs::write(&path, bytes).unwrap();
    assert_child(fixture.path(), "valid");
    assert_eq!(fs::read(path).unwrap(), bytes);
    assert!(!fixture.path().join("not-a-real-game").exists());
}

#[test]
fn absent_configuration_is_initialized_then_loaded_without_replacement() {
    let fixture = fixture();
    assert!(!fixture.path().join("settings").exists());
    assert_child(fixture.path(), "absent");
    let path = fixture.path().join("settings/ajour.yml");
    let loaded: Config = serde_yaml::from_slice(&fs::read(path).unwrap()).unwrap();
    assert_eq!(loaded, Config::default());
}

#[test]
fn non_file_configuration_is_refused_without_changing_its_contents() {
    let fixture = fixture();
    let path = fixture.path().join("settings/ajour.yml");
    fs::create_dir_all(&path).unwrap();
    fs::write(path.join("keep"), b"preserved directory").unwrap();
    assert_child(fixture.path(), "error");
    assert_eq!(fs::read(path.join("keep")).unwrap(), b"preserved directory");
}

#[test]
fn exclusive_initialization_refuses_an_already_appeared_file() {
    let fixture = fixture();
    let path = fixture.path().canonicalize().unwrap().join("ajour.yml");
    assert!(inspect_config_path(&path).unwrap().is_none());
    // Deterministic creation-race primitive: another writer publishes after the
    // observed absence, before our initializer. No timing/sleep is involved.
    fs::write(&path, b"another writer's settings").unwrap();
    let result = create_preserving::<Config>(&path);
    assert!(
        matches!(result, Err(FilesystemError::Io(ref e)) if e.kind() == io::ErrorKind::AlreadyExists)
    );
    assert_eq!(fs::read(path).unwrap(), b"another writer's settings");
}

#[test]
fn relative_config_root_is_supported_but_parent_components_fail_closed() {
    let fixture = fixture();
    fs::create_dir(fixture.path().join("settings")).unwrap();
    let path = fixture.path().join("settings/ajour.yml");
    let bytes = b"theme: relative-fixture\n";
    fs::write(&path, bytes).unwrap();
    assert_child(fixture.path(), "relative");
    assert_child(fixture.path(), "parent-components");
    assert_eq!(fs::read(path).unwrap(), bytes);
}

#[test]
fn regenerable_cache_fallback_keeps_its_existing_policy_without_touching_config() {
    let fixture = fixture();
    let root = fixture.path().join("settings");
    fs::create_dir_all(root.join("cache")).unwrap();
    fs::write(root.join("ajour.yml"), MALFORMED).unwrap();
    for name in ["fingerprints.yml", "addons.yml"] {
        fs::write(root.join("cache").join(name), MALFORMED).unwrap();
    }
    assert_child(fixture.path(), "cache");
    assert_eq!(fs::read(root.join("ajour.yml")).unwrap(), MALFORMED);
    for name in ["fingerprints.yml", "addons.yml"] {
        assert_ne!(fs::read(root.join("cache").join(name)).unwrap(), MALFORMED);
    }
}

#[cfg(windows)]
#[test]
fn read_sharing_failure_preserves_existing_configuration() {
    let fixture = fixture();
    fs::create_dir(fixture.path().join("settings")).unwrap();
    let path = fixture.path().join("settings/ajour.yml");
    fs::write(&path, MALFORMED).unwrap();
    assert_child(fixture.path(), "locked");
    assert_eq!(fs::read(path).unwrap(), MALFORMED);
}

#[cfg(windows)]
#[test]
fn junction_configuration_parent_is_refused_without_touching_target() {
    use std::os::windows::process::CommandExt;
    let fixture = fixture();
    let target = fixture.path().join("other-directory");
    fs::create_dir(&target).unwrap();
    fs::write(target.join("ajour.yml"), b"theme: preserved\n").unwrap();
    let link = fixture.path().join("settings");
    let result = Command::new("cmd.exe")
        .args(["/d", "/c", "mklink", "/J"])
        .arg(&link)
        .arg(&target)
        .creation_flags(0x08000000)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_child(fixture.path(), "error");
    assert_eq!(
        fs::read(target.join("ajour.yml")).unwrap(),
        b"theme: preserved\n"
    );
}

#[cfg(unix)]
#[test]
fn linked_and_dangling_configuration_files_and_parent_are_refused() {
    use std::os::unix::fs::symlink;
    for scenario in ["file", "dangling", "parent"] {
        let fixture = fixture();
        let outside = fixture.path().join("other-directory");
        fs::create_dir(&outside).unwrap();
        let target = outside.join("ajour.yml");
        if scenario != "dangling" {
            fs::write(&target, b"theme: preserved\n").unwrap();
        }
        if scenario == "parent" {
            symlink(&outside, fixture.path().join("settings")).unwrap();
        } else {
            fs::create_dir(fixture.path().join("settings")).unwrap();
            symlink(&target, fixture.path().join("settings/ajour.yml")).unwrap();
        }
        assert_child(fixture.path(), "error");
        if scenario == "dangling" {
            assert!(!target.exists());
        } else {
            assert_eq!(fs::read(target).unwrap(), b"theme: preserved\n");
        }
    }
}

#[test]
#[ignore = "isolated child protocol; executed by preservation parent only"]
fn configuration_child() {
    let Some(root) = std::env::var_os("PATCHLOOM_CONFIG_TEST_ROOT") else {
        return;
    };
    let root = PathBuf::from(root);
    assert!(root.is_absolute());
    assert!(root
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with("patchloom-config-preservation-"));
    assert_eq!(
        fs::read(root.join("fixture-marker")).unwrap(),
        b"disposable config fixture"
    );
    std::env::set_current_dir(&root).unwrap();
    let scenario = std::env::var("PATCHLOOM_CONFIG_TEST_CASE").unwrap();
    let config_root = match scenario.as_str() {
        "relative" | "parent-components" => PathBuf::from(if scenario == "relative" {
            "settings"
        } else {
            "settings/../settings"
        }),
        _ => root.join("settings"),
    };
    *super::super::CONFIG_DIR.lock().unwrap() = config_root;
    let path = root.join("settings/ajour.yml");
    match scenario.as_str() {
        "malformed" | "error" | "parent-components" => {
            let result: Result<Config> = Config::load_or_default();
            assert!(
                result.is_err(),
                "invalid existing config must surface an error"
            );
            assert!(async_std::task::block_on(crate::config::load_config()).is_err());
        }
        "valid" => {
            let config: Config = Config::load_or_default().unwrap();
            assert_eq!(config.theme.as_deref(), Some("fixture theme"));
            assert!(config.backup_wtf);
            assert_eq!(
                config.wow.directories[&crate::config::Flavor::Retail],
                PathBuf::from("not-a-real-game/_retail_")
            );
        }
        "relative" => {
            let config: Config = Config::load_or_default().unwrap();
            assert_eq!(config.theme.as_deref(), Some("relative-fixture"));
        }
        "absent" => {
            assert_eq!(
                Config::load_or_default::<Config>().unwrap(),
                Config::default()
            );
            let bytes = fs::read(&path).unwrap();
            assert_eq!(
                async_std::task::block_on(crate::config::load_config()).unwrap(),
                Config::default()
            );
            assert_eq!(fs::read(&path).unwrap(), bytes);
        }
        "cache" => {
            async_std::task::block_on(crate::cache::load_fingerprint_cache()).unwrap();
            async_std::task::block_on(crate::cache::load_addon_cache()).unwrap();
        }
        #[cfg(windows)]
        "locked" => {
            use std::os::windows::fs::OpenOptionsExt;
            let lock = fs::OpenOptions::new()
                .read(true)
                .share_mode(0)
                .open(&path)
                .unwrap();
            let result: Result<Config> = Config::load_or_default();
            assert!(result.is_err());
            assert!(async_std::task::block_on(crate::config::load_config()).is_err());
            drop(lock);
        }
        _ => panic!("unexpected child scenario"),
    }
}
