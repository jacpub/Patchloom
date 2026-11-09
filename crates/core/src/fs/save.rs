use super::{config_dir, FilesystemError, Result};
use serde::{de::DeserializeOwned, Serialize};

use std::fs;
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};

/// Defines a serializable struct that should persist on the filesystem inside the
/// Ajour config directory.
pub trait PersistentData: DeserializeOwned + Serialize {
    /// User configuration must opt out of the legacy regenerable-cache fallback.
    const PRESERVE_LOAD_ERRORS: bool = false;

    /// Only method required to implement PersistentData on an object. Always relative to
    /// the config folder for Ajour.
    fn relative_path() -> PathBuf;

    /// Returns the full file path. Will create any parent directories that don't
    /// exist.
    fn path() -> Result<PathBuf> {
        let path = config_dir().join(Self::relative_path());

        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }

        Ok(path)
    }

    /// Load from `PersistentData::path()`.
    fn load() -> Result<Self> {
        let path = Self::path()?;

        if path.exists() {
            let file = fs::File::open(&path)?;

            Ok(serde_yaml::from_reader(&file)?)
        } else {
            Err(FilesystemError::FileDoesntExist { path })
        }
    }

    /// Load from `PersistentData::path()`. If file doesn't exist, save it to the filesystem as `Default`
    /// and return that object.
    fn load_or_default<T: PersistentData + Default>() -> Result<T> {
        // Modified 2026-11-09; original GPLv3 applies. Do not use path() here:
        // it creates directories before we can inspect unsafe ancestor paths.
        if T::PRESERVE_LOAD_ERRORS {
            let root = super::CONFIG_DIR.lock().unwrap().clone();
            // Check before joining: Windows verbatim-prefix joins can normalize
            // away parent components before the later path inspection.
            if root.components().any(|p| matches!(p, Component::ParentDir)) {
                return Err(invalid_config_path(&root));
            }
            let path = root.join(T::relative_path());
            return load_preserving::<T>(&path);
        }
        let load_result = <T as PersistentData>::load();

        match load_result {
            Ok(deser) => Ok(deser),
            _ => Ok(get_default_and_save()?),
        }
    }

    /// Save to `PersistentData::path()`
    fn save(&self) -> Result<()> {
        let contents = serde_yaml::to_string(&self)?;

        fs::write(Self::path()?, contents)?;

        Ok(())
    }
}

fn invalid_config_path(path: &Path) -> FilesystemError {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        format!("Unsafe configuration path {path:?}; existing data was not replaced"),
    )
    .into()
}

fn is_link(metadata: &fs::Metadata) -> bool {
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

// Point-in-time checks, not protection against hostile concurrent ancestor
// replacement. Exclusive file creation below never truncates an existing file.
fn inspect_config_path(path: &Path) -> Result<Option<fs::Metadata>> {
    if !path.is_absolute() || path.components().any(|p| matches!(p, Component::ParentDir)) {
        return Err(invalid_config_path(path));
    }
    let mut target = None;
    for ancestor in path.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(metadata) => {
                if is_link(&metadata)
                    || if ancestor == path {
                        !metadata.is_file()
                    } else {
                        !metadata.is_dir()
                    }
                {
                    return Err(invalid_config_path(ancestor));
                }
                if ancestor == path {
                    target = Some(metadata);
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(target)
}

fn load_preserving<T: PersistentData + Default>(path: &Path) -> Result<T> {
    let path = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()?.join(path)
    };
    if inspect_config_path(&path)?.is_some() {
        let file = fs::File::open(&path)?;
        let metadata = file.metadata()?;
        if !metadata.is_file() || is_link(&metadata) {
            return Err(invalid_config_path(&path));
        }
        // Parse/read failures are returned; this branch never opens for writing.
        return Ok(serde_yaml::from_reader(file)?);
    }
    create_preserving::<T>(&path)
}

fn create_preserving<T: PersistentData + Default>(path: &Path) -> Result<T> {
    inspect_config_path(path)?;
    let data = T::default();
    let contents = serde_yaml::to_string(&data)?;
    let parent = path.parent().ok_or_else(|| invalid_config_path(path))?;
    fs::create_dir_all(parent)?;
    inspect_config_path(path)?;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    file.write_all(contents.as_bytes())?;
    file.sync_all()?;
    Ok(data)
}

/// Get `Default` and save it.
fn get_default_and_save<T: PersistentData + Default>() -> Result<T> {
    let data = Default::default();

    <T as PersistentData>::save(&data)?;

    Ok(data)
}

#[cfg(test)]
#[path = "save_tests.rs"]
mod tests;
