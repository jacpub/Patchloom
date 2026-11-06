//! Bounded BZIP2 addon ZIP regression, 2026-11-03; original Ajour GPLv3 applies.
use ajour_core::{
    addon::{Addon, AddonFolder},
    fs::install_addon,
};
use std::{
    fs,
    io::Write,
    path::Path,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};
use zip::{write::FileOptions, CompressionMethod, ZipArchive, ZipWriter};

struct OwnedChild(Child);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
            let deadline = Instant::now() + Duration::from_secs(2);
            while self.0.try_wait().ok().flatten().is_none() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(10));
            }
        }
    }
}

#[test]
fn bzip2_zip_decode_is_bounded_and_preserves_originals_on_error() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("fixture-marker"), b"patchloom-bzip2-v1").unwrap();
    let log = fs::File::create(temp.path().join("child.log")).unwrap();
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--ignored", "--exact", "bzip2_fixture_child", "--nocapture"])
        .env("PATCHLOOM_BZIP_FIXTURE", temp.path())
        .env("APPDATA", temp.path().join("unused-config"))
        .env("XDG_CONFIG_HOME", temp.path().join("unused-config"))
        .stdin(Stdio::null())
        .stdout(log.try_clone().unwrap())
        .stderr(log);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = OwnedChild(command.spawn().unwrap());
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            assert!(
                status.success(),
                "{}",
                fs::read_to_string(temp.path().join("child.log")).unwrap()
            );
            assert_eq!(
                fs::read(temp.path().join("finished")).unwrap(),
                b"3 cases checked"
            );
            assert!(!temp.path().join("unused-config").exists());
            break;
        }
        assert!(
            Instant::now() < deadline,
            "BZIP2 installer exceeded child deadline"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
#[ignore = "subprocess protocol entry; invoked by bounded fixture parent"]
fn bzip2_fixture_child() {
    let fixture = std::env::var_os("PATCHLOOM_BZIP_FIXTURE").expect("fixture parent required");
    let fixture = Path::new(&fixture);
    assert_eq!(
        fs::read(fixture.join("fixture-marker")).unwrap(),
        b"patchloom-bzip2-v1"
    );
    for case in ["truncated", "bad-header", "valid"] {
        let base = fixture.join(case);
        let root = base.join("AddOns");
        let downloads = base.join("downloads");
        fs::create_dir_all(root.join("Foo")).unwrap();
        fs::create_dir_all(root.join("Unrelated")).unwrap();
        fs::create_dir_all(base.join("WTF")).unwrap();
        fs::create_dir(&downloads).unwrap();
        fs::write(root.join("Foo/old.lua"), b"original").unwrap();
        fs::write(root.join("Unrelated/keep.lua"), b"unrelated").unwrap();
        fs::write(base.join("WTF/SavedVariables.lua"), b"settings").unwrap();
        let toc = b"## Interface: 120105\n## Title: BZIP2 Fixture\n";
        let mut writer = ZipWriter::new(std::io::Cursor::new(Vec::new()));
        writer
            .start_file(
                "Foo/Foo.toc",
                FileOptions::default().compression_method(CompressionMethod::Bzip2),
            )
            .unwrap();
        writer.write_all(toc).unwrap();
        let mut bytes = writer.finish().unwrap().into_inner();
        let mut archive = ZipArchive::new(std::io::Cursor::new(&bytes)).unwrap();
        let entry = archive.by_index(0).unwrap();
        assert_eq!(entry.compression(), CompressionMethod::Bzip2);
        let offset = entry.data_start() as usize;
        assert_eq!(&bytes[offset..offset + 3], b"BZh");
        drop(entry);
        drop(archive);
        match case {
            "truncated" => {
                // Preserve ZIP structure while limiting decoder input to BZhN.
                // Exercise unfinished-stream rejection; this is not a reproducer of
                // the security fix for FFI buffer-length truncation.
                let central = bytes.windows(4).rposition(|b| b == b"PK\x01\x02").unwrap();
                bytes[18..22].copy_from_slice(&4_u32.to_le_bytes());
                bytes[central + 20..central + 24].copy_from_slice(&4_u32.to_le_bytes());
            }
            "bad-header" => bytes[offset] = 0,
            "valid" => {}
            _ => unreachable!(),
        }
        fs::write(downloads.join("Foo"), bytes).unwrap();
        let mut addon = Addon::empty("Foo");
        addon.folders.push(AddonFolder {
            id: "Foo".into(),
            path: root.join("Foo"),
            ..Default::default()
        });
        let result = async_std::task::block_on(install_addon(&addon, &downloads, &root));
        if case == "valid" {
            result.unwrap();
            assert_eq!(fs::read(root.join("Foo/Foo.toc")).unwrap(), toc);
            assert!(!root.join("Foo/old.lua").exists());
        } else {
            let error = result.unwrap_err().to_string();
            if case == "truncated" {
                assert!(error.contains("decompression not finished"), "{}", error);
            }
            assert_eq!(fs::read(root.join("Foo/old.lua")).unwrap(), b"original");
            assert_eq!(fs::read_dir(root.join("Foo")).unwrap().count(), 1);
            assert!(!root.join(".patchloom-install").exists());
            assert!(!root.join(".patchloom-restore").exists());
            assert!(downloads.join("Foo").is_file());
        }
        assert_eq!(
            fs::read(root.join("Unrelated/keep.lua")).unwrap(),
            b"unrelated"
        );
        assert_eq!(
            fs::read(base.join("WTF/SavedVariables.lua")).unwrap(),
            b"settings"
        );
    }
    fs::write(fixture.join("finished"), b"3 cases checked").unwrap();
}
