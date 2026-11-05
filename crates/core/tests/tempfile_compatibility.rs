//! Temporary publication and cleanup compatibility, 2026-11-05.
//! Original Ajour GPLv3 attribution applies. All paths belong to disposable fixtures.
use std::io::{Read, Seek, SeekFrom, Write};

#[test]
fn publication_conflict_preserves_both_versions() {
    let root = tempfile::tempdir().unwrap();
    let destination = root.path().join("receipt.json");
    std::fs::write(&destination, b"original receipt").unwrap();
    let mut incoming = tempfile::Builder::new()
        .prefix(".receipt-")
        .tempfile_in(root.path())
        .unwrap();
    incoming.write_all(b"incoming receipt").unwrap();
    incoming.as_file().sync_all().unwrap();
    let temporary = incoming.path().to_owned();
    let mut retained = incoming.persist_noclobber(&destination).unwrap_err().file;
    retained.seek(SeekFrom::Start(0)).unwrap();
    let mut bytes = Vec::new();
    retained.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, b"incoming receipt");
    assert_eq!(std::fs::read(&destination).unwrap(), b"original receipt");
    drop(retained);
    assert!(!temporary.exists());
    assert_eq!(std::fs::read(destination).unwrap(), b"original receipt");
}

#[test]
fn publication_survives_temporary_handle_drop() {
    let root = tempfile::tempdir().unwrap();
    let destination = root.path().join("download.zip");
    let mut incoming = tempfile::Builder::new()
        .prefix(".patchloom-download-")
        .tempfile_in(root.path())
        .unwrap();
    incoming.write_all(b"verified fixture payload").unwrap();
    incoming.as_file().sync_all().unwrap();
    let published = incoming.persist_noclobber(&destination).unwrap();
    drop(published);
    assert_eq!(
        std::fs::read(destination).unwrap(),
        b"verified fixture payload"
    );
}

#[test]
fn directory_cleanup_preserves_sibling_source() {
    let root = tempfile::tempdir().unwrap();
    let original = root.path().join("original.txt");
    std::fs::write(&original, b"preserved source").unwrap();
    let staging = tempfile::Builder::new()
        .prefix("staging-")
        .tempdir_in(root.path())
        .unwrap();
    let staging_path = staging.path().to_owned();
    std::fs::create_dir(staging.path().join("nested")).unwrap();
    std::fs::write(staging.path().join("nested/incoming.txt"), b"incoming").unwrap();
    staging.close().unwrap();
    assert!(!staging_path.exists());
    assert_eq!(std::fs::read(original).unwrap(), b"preserved source");
}

#[cfg(unix)]
#[test]
fn directory_cleanup_does_not_follow_static_symlink() {
    let root = tempfile::tempdir().unwrap();
    let outside = root.path().join("outside");
    std::fs::create_dir(&outside).unwrap();
    std::fs::write(outside.join("original.txt"), b"outside source").unwrap();
    let staging = tempfile::Builder::new().tempdir_in(root.path()).unwrap();
    std::os::unix::fs::symlink(&outside, staging.path().join("link")).unwrap();
    staging.close().unwrap();
    assert_eq!(
        std::fs::read(outside.join("original.txt")).unwrap(),
        b"outside source"
    );
}
