//! Explicit copied-settings inspection, 2026-11-09; original GPLv3 applies.
use ajour_core::config::legacy_import::{inspect_copy, MAX_INPUT_BYTES};
use std::{
    fs::{self, Metadata, OpenOptions},
    io::{Read, Write},
    path::{Component, Path},
};

pub fn run(opts: &crate::cli::Opts, copy: &Path) -> crate::Result<()> {
    if opts.command.is_some()
        || opts.preview.is_some()
        || opts.preview_check
        || opts.preview_window_check
        || opts.startup_check
        || opts.data_directory.is_some()
        || opts.antialiasing.is_some()
        || opts.self_update_temp.is_some()
    {
        anyhow::bail!("Use --inspect-legacy-config COPY without other startup options or commands");
    }
    let bytes = read_copy(copy)?;
    let report = inspect_copy(Some(&bytes))?;
    let mut output = std::io::stdout().lock();
    serde_json::to_writer(&mut output, &report)?;
    writeln!(output)?;
    Ok(())
}

fn is_link(metadata: &Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

fn check_path(path: &Path) -> crate::Result<Metadata> {
    let mut target = None;
    for ancestor in path.ancestors() {
        let metadata = fs::symlink_metadata(ancestor)
            .map_err(|e| anyhow::anyhow!("Cannot inspect supplied copy ({})", e.kind()))?;
        if is_link(&metadata)
            || if ancestor == path {
                !metadata.is_file()
            } else {
                !metadata.is_dir()
            }
        {
            anyhow::bail!("The supplied copy must be a regular file without linked ancestors");
        }
        if ancestor == path {
            target = Some(metadata);
        }
    }
    target.ok_or_else(|| anyhow::anyhow!("No source copy was supplied"))
}

// Point-in-time ancestry checks, not a sandbox against hostile concurrent
// ancestor replacement. Never open for writing; never follow embedded hints.
fn read_copy(copy: &Path) -> crate::Result<Vec<u8>> {
    if copy.as_os_str().is_empty() || copy.components().any(|p| matches!(p, Component::ParentDir)) {
        anyhow::bail!("Supply a copy path without parent-directory components");
    }
    #[cfg(windows)]
    check_windows_spelling(copy)?;
    let path = std::env::current_dir()?.join(copy);
    let before = check_path(&path)?;
    if before.len() > MAX_INPUT_BYTES as u64 {
        anyhow::bail!("Settings copy exceeds the 64 KiB read limit");
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // OPEN_REPARSE_POINT, with read sharing only: no target-link following,
        // and cooperating writers/deleters cannot change the open file.
        options.custom_flags(0x0020_0000).share_mode(1);
    }
    let file = options
        .open(&path)
        .map_err(|e| anyhow::anyhow!("Cannot read supplied copy ({})", e.kind()))?;
    let opened = file.metadata()?;
    if !opened.is_file() || is_link(&opened) {
        anyhow::bail!("The supplied copy is not a regular file");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if (before.dev(), before.ino()) != (opened.dev(), opened.ino()) {
            anyhow::bail!("The supplied copy changed while opening it");
        }
    }
    check_path(&path)?;
    let mut bytes = Vec::new();
    (&file)
        .take(MAX_INPUT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| anyhow::anyhow!("Cannot read supplied copy ({})", e.kind()))?;
    if bytes.len() > MAX_INPUT_BYTES {
        anyhow::bail!("Settings copy exceeds the 64 KiB read limit");
    }
    let after = file.metadata()?;
    check_path(&path)?;
    if opened.len() != after.len() || opened.modified()? != after.modified()? {
        anyhow::bail!("The supplied copy changed while reading it");
    }
    Ok(bytes)
}

#[cfg(windows)]
fn check_windows_spelling(copy: &Path) -> crate::Result<()> {
    if !copy.is_absolute()
        && (copy.has_root() || copy.components().any(|p| matches!(p, Component::Prefix(_))))
    {
        anyhow::bail!("Drive-relative and rooted-without-drive copy paths are not supported");
    }
    for component in copy.components() {
        match component {
            Component::Prefix(prefix)
                if !matches!(
                    prefix.kind(),
                    std::path::Prefix::Disk(_) | std::path::Prefix::VerbatimDisk(_)
                ) =>
            {
                anyhow::bail!("Network and device paths are not supported for settings copies");
            }
            Component::Normal(part) => {
                let name = part
                    .to_str()
                    .ok_or_else(|| anyhow::anyhow!("Unsupported settings copy path spelling"))?;
                // Same DOS-device spellings refused by the existing archive policy.
                let device = name
                    .split('.')
                    .next()
                    .unwrap_or_default()
                    .trim_end_matches(' ')
                    .to_ascii_uppercase();
                if name.ends_with(['.', ' '])
                    || name
                        .chars()
                        .any(|c| c.is_control() || ":<>\"|?*".contains(c))
                    || matches!(
                        device.as_str(),
                        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$" | "CLOCK$"
                    )
                    || ["COM", "LPT"].iter().any(|prefix| {
                        device.strip_prefix(prefix).is_some_and(|suffix| {
                            matches!(
                                suffix,
                                "1" | "2"
                                    | "3"
                                    | "4"
                                    | "5"
                                    | "6"
                                    | "7"
                                    | "8"
                                    | "9"
                                    | "\u{b9}"
                                    | "\u{b2}"
                                    | "\u{b3}"
                            )
                        })
                    })
                {
                    anyhow::bail!("Device names, alternate streams and ambiguous copy path spellings are not supported");
                }
            }
            _ => {}
        }
    }
    Ok(())
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn ambiguous_drive_and_dos_device_spellings_are_rejected_before_any_io() {
        for text in [
            "C:copy.yml",
            "\\copy.yml",
            "NUL",
            "NUL.yml",
            "CON",
            "conin$",
            "CONOUT$",
            "CLOCK$",
            "AUX.txt",
            "PRN",
            "COM1",
            "LPT9.yml",
            "COM\u{b9}.yml",
            "copy.yml:stream",
            "copy.yml.",
            "copy.yml ",
            "\\\\server\\share\\copy.yml",
            "\\\\.\\NUL",
        ] {
            assert!(check_windows_spelling(Path::new(text)).is_err(), "{}", text);
        }
        assert!(check_windows_spelling(Path::new("C:\\copies\\settings.yml")).is_ok());
        assert!(check_windows_spelling(Path::new("copies/settings.yml")).is_ok());
    }
}
