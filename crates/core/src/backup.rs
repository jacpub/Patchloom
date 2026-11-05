use crate::error::FilesystemError;
use crate::fs::backup::{Backup, ZipBackup, ZstdBackup};
use crate::repository::CompressionFormat;

use chrono::{Local, NaiveDateTime};
use std::convert::TryFrom;
use std::path::{Path, PathBuf};

/// Creates a .zip archive from the list of source folders and
/// saves it to the dest folder.
pub async fn backup_folders(
    src_folders: Vec<BackupFolder>,
    mut dest: PathBuf,
    compression: CompressionFormat,
    zstd_level: i32,
) -> Result<NaiveDateTime, FilesystemError> {
    let now = Local::now();

    dest.push(format!(
        "ajour_backup_{}.{}",
        now.format("%Y-%m-%d_%H-%M-%S"),
        compression.file_ext(),
    ));

    match compression {
        CompressionFormat::Zip => ZipBackup::new(src_folders, &dest).backup()?,
        CompressionFormat::Zstd => ZstdBackup::new(src_folders, &dest, zstd_level).backup()?,
    }

    // Won't fail since we pass it the correct format
    let as_of = Archive::try_from(dest).unwrap().as_of;

    Ok(as_of)
}

/// Finds the latest archive in the supplied backup folder and returns
/// the datetime it was saved
pub async fn latest_backup(backup_dir: PathBuf) -> Option<NaiveDateTime> {
    let zip_pattern = format!("{}/ajour_backup_[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]_[0-9][0-9]-[0-9][0-9]-[0-9][0-9].zip", backup_dir.display());
    let zstd_pattern = format!("{}/ajour_backup_[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]_[0-9][0-9]-[0-9][0-9]-[0-9][0-9].tar.zst", backup_dir.display());

    let mut backups = vec![];

    for path in glob::glob(&zip_pattern)
        .unwrap()
        .chain(glob::glob(&zstd_pattern).unwrap())
        .flatten()
    {
        if let Ok(archive) = Archive::try_from(path) {
            backups.push(archive.as_of);
        }
    }

    // Apparently NaiveDateTime sorts in Desc order by default, no need to reverse
    backups.sort();
    backups.pop()
}

/// Specifies a folder that we want backed up. `prefix` will get stripped out of
/// the path of each entry in the archive.
pub struct BackupFolder {
    pub path: PathBuf,
    pub prefix: PathBuf,
}

impl BackupFolder {
    pub fn new(path: impl AsRef<Path>, prefix: impl AsRef<Path>) -> BackupFolder {
        BackupFolder {
            path: path.as_ref().to_owned(),
            prefix: prefix.as_ref().to_owned(),
        }
    }
}

/// Metadata for our archive saved on the filesystem. Converted from a `PathBuf` with
/// the correct naming convention
struct Archive {
    pub as_of: NaiveDateTime,
}

impl TryFrom<PathBuf> for Archive {
    type Error = chrono::ParseError;

    fn try_from(path: PathBuf) -> Result<Archive, chrono::ParseError> {
        let mut file_stem = path.file_stem().unwrap().to_str().unwrap();

        // in the case of "file.tar.zst" path.file_stem() will return "file.tar", we still need to
        // drop the extension
        if let Some(i) = file_stem.find('.') {
            file_stem = file_stem.split_at(i).0;
        }

        let date_str = format!(
            "{} {}",
            file_stem.split('_').nth(2).unwrap_or_default(),
            file_stem.split('_').nth(3).unwrap_or_default()
        );

        let as_of = NaiveDateTime::parse_from_str(&date_str, "%Y-%m-%d %H-%M-%S")?;

        Ok(Archive { as_of })
    }
}

#[cfg(test)]
mod chrono_compatibility_tests {
    use super::*;
    use chrono::{FixedOffset, TimeZone, Utc};
    use std::io::Read;

    #[test]
    fn chrono_upgrade_preserves_offset_format_and_serialization() {
        let utc = Utc.ymd(2024, 2, 29).and_hms(23, 30, 45);
        let local = utc.with_timezone(&FixedOffset::east(2 * 60 * 60));
        assert_eq!(
            local.format("%Y-%m-%d_%H-%M-%S").to_string(),
            "2024-03-01_01-30-45"
        );
        assert_eq!(local.with_timezone(&Utc), utc);
        let serialized = serde_json::to_string(&local).unwrap();
        assert_eq!(serialized, "\"2024-03-01T01:30:45+02:00\"");
        let decoded: chrono::DateTime<FixedOffset> = serde_json::from_str(&serialized).unwrap();
        assert_eq!(decoded, local);
    }

    #[test]
    fn chrono_upgrade_preserves_backup_filename_parsing() {
        let expected =
            NaiveDateTime::parse_from_str("2024-02-29 23:30:45", "%Y-%m-%d %H:%M:%S").unwrap();
        for extension in ["zip", "tar.zst"] {
            let path = PathBuf::from(format!("ajour_backup_2024-02-29_23-30-45.{extension}"));
            assert_eq!(Archive::try_from(path).unwrap().as_of, expected);
            let invalid = PathBuf::from(format!("ajour_backup_2023-02-29_23-30-45.{extension}"));
            assert!(Archive::try_from(invalid).is_err());
        }
    }

    #[test]
    fn chrono_upgrade_preserves_zip_backup_contents_and_timestamp() {
        async_std::task::block_on(async {
            let fixture = tempfile::tempdir().unwrap();
            let source = fixture.path().join("Addon");
            let output = fixture.path().join("Backups");
            std::fs::create_dir(&source).unwrap();
            std::fs::create_dir(&output).unwrap();
            let original = b"fixture addon bytes\n";
            std::fs::write(source.join("Fixture.lua"), original).unwrap();
            let before = Local::now().naive_local() - chrono::Duration::seconds(2);
            let as_of = backup_folders(
                vec![BackupFolder::new(&source, fixture.path())],
                output.clone(),
                CompressionFormat::Zip,
                1,
            )
            .await
            .unwrap();
            let after = Local::now().naive_local();
            assert_eq!(latest_backup(output.clone()).await, Some(as_of));
            let archive_path = std::fs::read_dir(&output)
                .unwrap()
                .next()
                .unwrap()
                .unwrap()
                .path();
            assert_eq!(
                Archive::try_from(archive_path.clone()).unwrap().as_of,
                as_of
            );
            let mut archive =
                zip::ZipArchive::new(std::fs::File::open(archive_path).unwrap()).unwrap();
            let mut entry = archive.by_name("Addon/Fixture.lua").unwrap();
            let timestamp = entry.last_modified();
            assert!(chrono::NaiveDate::from_ymd_opt(
                timestamp.year().into(),
                timestamp.month().into(),
                timestamp.day().into()
            )
            .is_some());
            let calendar = chrono::NaiveDate::from_ymd_opt(
                timestamp.year().into(),
                timestamp.month().into(),
                timestamp.day().into(),
            )
            .unwrap()
            .and_hms_opt(
                timestamp.hour().into(),
                timestamp.minute().into(),
                timestamp.second().into(),
            )
            .unwrap();
            assert!(
                calendar >= before && calendar <= after,
                "ZIP must retain the current local backup time"
            );
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).unwrap();
            assert_eq!(bytes, original);
            assert_eq!(std::fs::read(source.join("Fixture.lua")).unwrap(), original);
        });
    }
}
