//! Bounded in-memory PAX and backup compatibility fixtures, 2026-11-05.
//! Original Ajour GPLv3 attribution applies.
use std::io::Read;

fn append_block(archive: &mut Vec<u8>, header: tar::Header, body: &[u8]) {
    archive.extend_from_slice(header.as_bytes());
    archive.extend_from_slice(body);
    archive.resize(archive.len().div_ceil(512) * 512, 0);
}

#[test]
fn pax_size_overrides_nonzero_base_header_size() {
    for base_size in [0, 1] {
        let mut bytes = Vec::new();
        let mut pax = tar::Header::new_ustar();
        pax.set_path("PaxHeader").unwrap();
        pax.set_entry_type(tar::EntryType::XHeader);
        pax.set_size(10);
        pax.set_cksum();
        append_block(&mut bytes, pax, b"10 size=3\n");
        let mut file = tar::Header::new_ustar();
        file.set_path("Fixture.txt").unwrap();
        file.set_entry_type(tar::EntryType::Regular);
        file.set_size(base_size);
        file.set_cksum();
        append_block(&mut bytes, file, b"abc");
        bytes.extend_from_slice(&[0; 1024]);
        let mut archive = tar::Archive::new(bytes.as_slice());
        let mut entries = archive.entries().unwrap();
        let mut entry = entries.next().unwrap().unwrap();
        let mut body = Vec::new();
        entry.by_ref().take(8).read_to_end(&mut body).unwrap();
        assert_eq!(
            body, b"abc",
            "PAX must take precedence over base size {base_size}"
        );
        drop(entry);
        assert!(entries.next().is_none());
    }
}

#[test]
fn zstd_tar_backup_roundtrip_preserves_contents_and_sources() {
    use ajour_core::{
        backup::{backup_folders, BackupFolder},
        repository::CompressionFormat,
    };
    async_std::task::block_on(async {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("Fixture");
        let backups = root.path().join("Backups");
        std::fs::create_dir(&source).unwrap();
        std::fs::create_dir(&backups).unwrap();
        std::fs::write(source.join("Payload.txt"), b"archive fixture bytes").unwrap();
        backup_folders(
            vec![BackupFolder::new(&source, root.path())],
            backups.clone(),
            CompressionFormat::Zstd,
            3,
        )
        .await
        .unwrap();
        let path = std::fs::read_dir(backups)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let decoder = zstd::stream::read::Decoder::new(std::fs::File::open(path).unwrap()).unwrap();
        let mut archive = tar::Archive::new(decoder);
        let mut payloads = 0;
        for entry in archive.entries().unwrap() {
            let mut entry = entry.unwrap();
            if entry.path().unwrap().as_ref() == std::path::Path::new("Fixture/Payload.txt") {
                let mut body = Vec::new();
                entry.by_ref().take(64).read_to_end(&mut body).unwrap();
                assert_eq!(body, b"archive fixture bytes");
                payloads += 1;
            }
        }
        assert_eq!(payloads, 1);
        assert_eq!(
            std::fs::read(source.join("Payload.txt")).unwrap(),
            b"archive fixture bytes"
        );
    });
}
