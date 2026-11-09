use super::Result;
use crate::backup::BackupFolder;
use crate::error::FilesystemError;
use chrono::{Datelike, Local, NaiveDateTime, Timelike};

use path_slash::PathExt;
use std::fs::File;
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;
use zip::{write::FileOptions, CompressionMethod, ZipWriter};

// Modified 2026-11-05; preserve ZIP's local calendar fields and DOS fallback
// without invoking time 0.1 through FileOptions::default().
fn zip_timestamp(now: NaiveDateTime) -> zip::DateTime {
    if !(1980..=2107).contains(&now.year()) {
        return zip::DateTime::default();
    }
    zip::DateTime::from_date_and_time(
        now.year() as u16,
        now.month() as u8,
        now.day() as u8,
        now.hour() as u8,
        now.minute() as u8,
        // Chrono represents a leap second with nanoseconds >= one billion.
        (now.second() + u32::from(now.nanosecond() >= 1_000_000_000)) as u8,
    )
    .unwrap_or_default()
}

/// A trait defining a way to back things up to the fs
pub trait Backup {
    fn backup(&self) -> Result<()>;
}

/// Back up folders to a zip archive and save on the fs
pub struct ZipBackup {
    src: Vec<BackupFolder>,
    dest: PathBuf,
}

impl ZipBackup {
    pub(crate) fn new(src: Vec<BackupFolder>, dest: impl AsRef<Path>) -> ZipBackup {
        ZipBackup {
            src,
            dest: dest.as_ref().to_owned(),
        }
    }
}

impl Backup for ZipBackup {
    fn backup(&self) -> Result<()> {
        let output = BufWriter::new(File::create(&self.dest)?);

        let mut zip_writer = ZipWriter::new(output);
        let options = FileOptions::default()
            .last_modified_time(zip_timestamp(Local::now().naive_local()))
            .compression_method(CompressionMethod::Deflated)
            .unix_permissions(0o755);

        let mut buffer = vec![];

        for folder in &self.src {
            let prefix = &folder.prefix;
            let path = &folder.path;

            zip_write(path, prefix, &mut buffer, &mut zip_writer, options)?;

            for entry in WalkDir::new(path)
                .into_iter()
                .filter_map(std::result::Result::ok)
            {
                let path = entry.path();

                zip_write(path, prefix, &mut buffer, &mut zip_writer, options)?;
            }
        }

        zip_writer.finish()?;

        Ok(())
    }
}

/// Write each path to the zip archive
fn zip_write(
    path: &Path,
    prefix: &Path,
    buffer: &mut Vec<u8>,
    writer: &mut ZipWriter<BufWriter<File>>,
    options: FileOptions,
) -> Result<()> {
    if !path.exists() {
        return Err(FilesystemError::FileDoesntExist {
            path: path.to_owned(),
        });
    }

    // On windows, convers `\` to `/`
    let normalized_path = path
        .to_slash()
        .ok_or(FilesystemError::NormalizingPathSlash {
            path: path.to_path_buf(),
        })?;
    let normalized_prefix = prefix
        .to_slash()
        .ok_or(FilesystemError::NormalizingPathSlash {
            path: prefix.to_path_buf(),
        })?;

    // Strip prefix from path name and remove leading slash
    let name = normalized_path
        .strip_prefix(&normalized_prefix)
        .ok_or(FilesystemError::StripPrefix {
            prefix: normalized_prefix,
            from: normalized_path.clone(),
        })?
        .trim_start_matches('/');

    if path.is_dir() {
        writer.add_directory(name, options)?;
    } else {
        writer.start_file(name, options)?;

        let mut file = File::open(path)?;
        file.read_to_end(buffer)?;

        writer.write_all(buffer)?;
        buffer.clear();
    }

    Ok(())
}

pub struct ZstdBackup {
    src: Vec<BackupFolder>,
    dest: PathBuf,
    level: i32,
}

impl ZstdBackup {
    pub(crate) fn new(src: Vec<BackupFolder>, dest: impl AsRef<Path>, level: i32) -> ZstdBackup {
        ZstdBackup {
            src,
            dest: dest.as_ref().to_owned(),
            level,
        }
    }
}

impl Backup for ZstdBackup {
    fn backup(&self) -> Result<()> {
        use zstd::stream::write::Encoder as ZstdEncoder;

        let output = File::create(&self.dest)?;
        let mut enc = ZstdEncoder::new(output, self.level)?;
        enc.multithread(num_cpus::get() as u32)?;
        let mut tar = tar::Builder::new(enc.auto_finish());

        for folder in &self.src {
            let path = folder.path.strip_prefix(&folder.prefix).unwrap();
            let src_path = folder.prefix.join(&folder.path);
            tar.append_dir_all(path, src_path)?;
        }
        tar.finish()?;

        Ok(())
    }
}

#[cfg(test)]
mod timestamp_tests {
    use super::*;

    #[test]
    fn zip_defaults_use_dos_epoch_before_explicit_timestamp() {
        let mut archive = ZipWriter::new(std::io::Cursor::new(Vec::new()));
        archive
            .start_file("fixture.txt", FileOptions::default())
            .unwrap();
        let bytes = archive.finish().unwrap().into_inner();
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        let timestamp = archive.by_index(0).unwrap().last_modified();
        assert_eq!(
            (timestamp.year(), timestamp.month(), timestamp.day()),
            (1980, 1, 1)
        );
        assert_eq!(
            (timestamp.hour(), timestamp.minute(), timestamp.second()),
            (0, 0, 0)
        );
    }

    #[test]
    fn explicit_zip_timestamp_preserves_bounds_and_dos_precision() {
        for (input, expected) in [
            ("1980-01-01 00:00:00", (1980, 1, 1, 0, 0, 0)),
            ("2107-12-31 23:59:59", (2107, 12, 31, 23, 59, 58)),
            ("1979-12-31 23:59:59", (1980, 1, 1, 0, 0, 0)),
            ("2108-01-01 00:00:00", (1980, 1, 1, 0, 0, 0)),
            ("2016-12-31 23:59:60", (2016, 12, 31, 23, 59, 60)),
        ] {
            let date = NaiveDateTime::parse_from_str(input, "%Y-%m-%d %H:%M:%S").unwrap();
            for compression in [CompressionMethod::Deflated, CompressionMethod::Bzip2] {
                let mut writer = ZipWriter::new(std::io::Cursor::new(Vec::new()));
                writer
                    .start_file(
                        "fixture.txt",
                        FileOptions::default()
                            .last_modified_time(zip_timestamp(date))
                            .compression_method(compression),
                    )
                    .unwrap();
                writer.write_all(b"preserved fixture").unwrap();
                let mut reader = zip::ZipArchive::new(std::io::Cursor::new(
                    writer.finish().unwrap().into_inner(),
                ))
                .unwrap();
                let mut entry = reader.by_index(0).unwrap();
                let stamp = entry.last_modified();
                assert_eq!(
                    (
                        stamp.year(),
                        stamp.month(),
                        stamp.day(),
                        stamp.hour(),
                        stamp.minute(),
                        stamp.second()
                    ),
                    expected,
                    "{input}"
                );
                assert_eq!(entry.compression(), compression);
                let mut bytes = Vec::new();
                entry.read_to_end(&mut bytes).unwrap();
                assert_eq!(bytes, b"preserved fixture");
            }
        }
        // A checked range must precede the narrowing year cast.
        for year in [-1, 67516] {
            let date = chrono::NaiveDate::from_ymd_opt(year, 1, 1)
                .unwrap()
                .and_hms_opt(0, 0, 0)
                .unwrap();
            assert_eq!(zip_timestamp(date).year(), 1980);
        }
    }

    #[test]
    fn explicit_zip_timestamp_keeps_local_wall_clock_fields() {
        let date = chrono::DateTime::parse_from_rfc3339("2024-03-01T00:30:45+02:00").unwrap();
        let stamp = zip_timestamp(date.naive_local());
        assert_eq!((stamp.year(), stamp.month(), stamp.day()), (2024, 3, 1));
        assert_eq!((stamp.hour(), stamp.minute(), stamp.second()), (0, 30, 45));
    }
}
