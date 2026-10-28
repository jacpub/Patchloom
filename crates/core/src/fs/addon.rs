// Patchloom lint maintenance, 2026-10-27; original license and attribution apply.
use super::Result;
use crate::addon::AddonFolder;
use std::fs::{remove_dir_all, remove_file};
use std::path::Path;
use walkdir::WalkDir;

/// Deletes an Addon and all dependencies from disk.
pub fn delete_addons(addon_folders: &[AddonFolder]) -> Result<()> {
    for folder in addon_folders {
        let path = &folder.path;
        if path.exists() {
            remove_dir_all(path)?;
        }
    }

    Ok(())
}

/// Deletes all saved varaible files correlating to `[AddonFolder]`.
pub fn delete_saved_variables(addon_folders: &[AddonFolder], wtf_path: &Path) -> Result<()> {
    for entry in WalkDir::new(wtf_path)
        .into_iter()
        .filter_map(std::result::Result::ok)
    {
        let path = entry.path();
        let parent_name = path
            .parent()
            .and_then(|a| a.file_name())
            .and_then(|a| a.to_str());

        if parent_name == Some("SavedVariables") {
            let file_name = path
                .file_name()
                .and_then(|a| a.to_str())
                .map(|a| a.trim_end_matches(".bak").trim_end_matches(".lua"));

            // NOTE: Will reject "Foobar_<invalid utf8>".
            if let Some(file_name_str) = file_name {
                for folder in addon_folders {
                    if file_name_str == folder.id {
                        remove_file(path)?;
                    }
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod test {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn test_delete_saved_variables() {
        let folders = vec![
            AddonFolder {
                id: "AddonA".to_string(),
                ..Default::default()
            },
            AddonFolder {
                id: "AddonB".to_string(),
                ..Default::default()
            },
            AddonFolder {
                id: "AddonC".to_string(),
                ..Default::default()
            },
            AddonFolder {
                id: "AddonD".to_string(),
                ..Default::default()
            },
            AddonFolder {
                id: "AddonE".to_string(),
                ..Default::default()
            },
            AddonFolder {
                id: "AddonF".to_string(),
                ..Default::default()
            },
            AddonFolder {
                id: "AddonG".to_string(),
                ..Default::default()
            },
        ];

        let tempdir = tempdir().unwrap();
        let root = tempdir.path();
        let sv = root.join("SavedVariables");

        fs::create_dir_all(&sv).unwrap();

        let mut files = vec![];
        for (idx, folder) in folders.iter().enumerate() {
            let mut name = if idx % 2 == 0 {
                format!("{}.lua", folder.id)
            } else {
                format!("{}.lua.bak", folder.id)
            };

            // change filename of last file, it shouldn't get removed
            if idx == folders.len() - 1 {
                name = format!("{}.nonlua", folder.id)
            }

            let path = sv.join(&name);
            fs::File::create(&path).unwrap();

            files.push(path);
        }

        delete_saved_variables(&folders, root).unwrap();

        let mut exists = 0;
        for file in files {
            if file.exists() {
                exists += 1;
            }
        }

        assert_eq!(exists, 1);
    }
}
