//! Patchloom staged installer, modified 2026-10-26; original Ajour GPLv3 applies.
use super::Result;
use crate::{
    addon::{Addon, AddonFolder},
    parse::parse_toc_path,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeSet, HashMap, HashSet},
    fs::{self, File},
    io::{self, Read},
    path::Path,
};
use walkdir::WalkDir;

const TRANSACTION: &str = ".patchloom-install";
const LOCK: &str = ".patchloom-install.lock";
const RESTORE: &str = ".patchloom-restore";
const MAX_ENTRIES: usize = 10_000;
const MAX_FILE: u64 = 128 * 1024 * 1024;
const MAX_TOTAL: u64 = 512 * 1024 * 1024;

// No environment hooks or subprocess support are present in distributed builds.
#[cfg(all(test, windows, feature = "no-self-update"))]
mod process_tests;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Plan {
    version: u8,
    previous: Vec<String>,
    incoming: Vec<String>,
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

// Use the same portable spelling rules on all platforms, including Windows ADS/devices.
pub(crate) fn component(name: &str) -> io::Result<()> {
    let device = name
        .split('.')
        .next()
        .unwrap_or_default()
        .trim_end_matches(' ')
        .to_ascii_uppercase();
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.len() > 255
        || name.ends_with(['.', ' '])
        || name
            .chars()
            .any(|c| c.is_control() || "\\:<>\"|?*/".contains(c))
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
        return Err(invalid(format!("Unsafe addon path component: {name:?}")));
    }
    Ok(())
}

fn folder_name(name: &str) -> io::Result<()> {
    component(name)?;
    if name.to_lowercase().starts_with(".patchloom")
        || matches!(name.to_ascii_lowercase().as_str(), "wtf" | "savedvariables")
    {
        return Err(invalid(format!("Reserved addon folder: {name:?}")));
    }
    Ok(())
}

fn is_link(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return true;
        }
    }
    metadata.file_type().is_symlink()
}

fn exists(path: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn plain_tree(path: &Path) -> Result<()> {
    for entry in WalkDir::new(path).follow_links(false) {
        let entry = entry?;
        if is_link(&fs::symlink_metadata(entry.path())?) {
            return Err(invalid(format!(
                "Links/reparse points are not allowed: {:?}",
                entry.path()
            ))
            .into());
        }
    }
    Ok(())
}

fn remove_tree(path: &Path) -> Result<()> {
    plain_tree(path)?;
    fs::remove_dir_all(path)?;
    Ok(())
}

fn validate_archive(archive: &mut zip::ZipArchive<File>) -> Result<Vec<String>> {
    if archive.is_empty() || archive.len() > MAX_ENTRIES {
        return Err(invalid("Empty archive or too many archive entries").into());
    }
    let mut paths = HashMap::new();
    let mut explicit = HashSet::new();
    let mut folders = BTreeSet::new();
    let mut total = 0u64;
    for index in 0..archive.len() {
        let file = archive.by_index(index)?;
        let raw = std::str::from_utf8(file.name_raw())
            .map_err(|_| invalid("Archive path is not UTF-8"))?;
        let name = if file.is_dir() {
            raw.strip_suffix('/').unwrap_or(raw)
        } else {
            raw
        };
        let parts: Vec<_> = name.split('/').collect();
        if name.len() > 1024 || parts.len() > 64 || (!file.is_dir() && parts.len() < 2) {
            return Err(invalid(format!(
                "Archive entry must be inside an addon folder: {raw:?}"
            ))
            .into());
        }
        for part in &parts {
            component(part)?;
        }
        folder_name(parts[0])?;
        let kind = file.unix_mode().unwrap_or_default() & 0o170000;
        if !matches!(kind, 0 | 0o040000 | 0o100000) || (file.is_dir() && file.size() != 0) {
            return Err(invalid(format!("Unsupported archive entry type: {raw:?}")).into());
        }
        total = total
            .checked_add(file.size())
            .ok_or_else(|| invalid("Archive size overflow"))?;
        if file.size() > MAX_FILE || total > MAX_TOTAL {
            return Err(
                invalid("Archive exceeds extraction limits (128 MiB/file, 512 MiB total)").into(),
            );
        }
        if !explicit.insert(name.to_lowercase()) {
            return Err(invalid(format!("Duplicate archive entry: {raw:?}")).into());
        }
        for length in 1..=parts.len() {
            let path = parts[..length].join("/");
            let directory = length < parts.len() || file.is_dir();
            if let Some((old_path, old_directory)) =
                paths.insert(path.to_lowercase(), (path.clone(), directory))
            {
                if old_path != path || !old_directory || !directory {
                    return Err(
                        invalid(format!("Case or file/directory collision: {raw:?}")).into(),
                    );
                }
            }
        }
        folders.insert(parts[0].to_owned());
    }
    Ok(folders.into_iter().collect())
}

fn stage(
    archive: &mut zip::ZipArchive<File>,
    directory: &Path,
    folders: &[String],
) -> Result<Vec<AddonFolder>> {
    fs::create_dir(directory)?;
    for name in folders {
        fs::create_dir(directory.join(name))?; // Filesystem aliases must not merge top-level folders.
    }
    for index in 0..archive.len() {
        let mut file = archive.by_index(index)?;
        // Use the same UTF-8 spelling as validation, independent of the ZIP encoding flag.
        let path = directory.join(
            std::str::from_utf8(file.name_raw())
                .map_err(|_| invalid("Archive path is not UTF-8"))?,
        );
        if file.is_dir() {
            fs::create_dir_all(path)?;
        } else {
            fs::create_dir_all(
                path.parent()
                    .ok_or_else(|| invalid("Missing archive parent"))?,
            )?;
            let mut output = File::create_new(path)?;
            let expected = file.size();
            let copied = io::copy(&mut file.by_ref().take(MAX_FILE + 1), &mut output)?;
            if copied != expected {
                return Err(invalid("Archive size does not match extracted file").into());
            }
            output.sync_all()?;
        }
    }
    let mut addons = vec![];
    for name in folders {
        let mut parsed = None;
        for entry in fs::read_dir(directory.join(name))? {
            let path = entry?.path();
            if path.is_file()
                && path
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("toc"))
            {
                parsed = parse_toc_path(&path);
                if parsed.is_some() {
                    break;
                }
            }
        }
        addons.push(
            parsed.ok_or_else(|| invalid(format!("Addon folder has no readable TOC: {name}")))?,
        );
    }
    Ok(addons)
}

fn make_plan(addon: &Addon, root: &Path, incoming: Vec<String>) -> Result<Plan> {
    let mut owned = BTreeSet::new();
    let mut owned_case = HashSet::new();
    for folder in &addon.folders {
        folder_name(&folder.id)?;
        if folder.path.file_name() != Some(folder.id.as_ref())
            || folder
                .path
                .parent()
                .ok_or_else(|| invalid("Missing addon parent"))?
                .canonicalize()?
                != root
            || !owned_case.insert(folder.id.to_lowercase())
        {
            return Err(invalid(
                "Installed addon paths must be distinct direct children of AddOns",
            )
            .into());
        }
        owned.insert(folder.id.clone());
    }
    let relevant: HashSet<_> = incoming
        .iter()
        .chain(&owned)
        .map(|name| name.to_lowercase())
        .collect();
    let mut verified_owned = HashSet::new();
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| invalid("Non-UTF-8 addon folder"))?;
        if relevant.contains(&name.to_lowercase()) {
            if !owned.contains(&name)
                || (!incoming.contains(&name)
                    && incoming
                        .iter()
                        .any(|n| n.to_lowercase() == name.to_lowercase()))
            {
                return Err(invalid(format!(
                    "Addon folder conflicts with an existing folder: {name}"
                ))
                .into());
            }
            if !entry.file_type()?.is_dir() {
                return Err(invalid("Addon destination is not a directory").into());
            }
            plain_tree(&entry.path())?;
            verified_owned.insert(name);
        }
    }
    let mut previous = vec![];
    for name in owned {
        if exists(&root.join(&name))? {
            if !verified_owned.contains(&name) {
                return Err(invalid(
                    "Installed folder spelling does not match its directory entry",
                )
                .into());
            }
            previous.push(name);
        }
    }
    Ok(Plan {
        version: 1,
        previous,
        incoming,
    })
}

fn marker(path: &Path) -> Result<bool> {
    if !exists(path)? {
        return Ok(false);
    }
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() || is_link(&metadata) || metadata.len() != 0 {
        return Err(invalid(format!(
            "Invalid transaction marker; state retained: {path:?}"
        ))
        .into());
    }
    Ok(true)
}

fn validate_transaction(transaction: &Path, plan: &Plan) -> Result<()> {
    plain_tree(transaction)?;
    for entry in fs::read_dir(transaction)? {
        let entry = entry?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| invalid("Invalid transaction entry"))?;
        match name.as_str() {
            "plan.yml" => {
                let metadata = entry.metadata()?;
                if !metadata.is_file() || metadata.len() > 1024 * 1024 {
                    return Err(invalid("Invalid install journal file").into());
                }
            }
            "ready" | "committed" => {
                marker(&entry.path())?;
            }
            "staged" | "backup" => {
                if !entry.file_type()?.is_dir() {
                    return Err(invalid("Invalid transaction directory").into());
                }
                let allowed = if name == "staged" {
                    &plan.incoming
                } else {
                    &plan.previous
                };
                for folder in fs::read_dir(entry.path())? {
                    let folder = folder?;
                    let name = folder
                        .file_name()
                        .into_string()
                        .map_err(|_| invalid("Invalid transaction folder"))?;
                    if !folder.file_type()?.is_dir() || !allowed.contains(&name) {
                        return Err(
                            invalid("Unexpected transaction contents; backups retained").into()
                        );
                    }
                }
            }
            _ => return Err(invalid("Unknown transaction entry; state retained").into()),
        }
    }
    if marker(&transaction.join("ready"))? && marker(&transaction.join("committed"))? {
        return Err(invalid("Conflicting install journal state").into());
    }
    Ok(())
}

fn cleanup_transaction(transaction: &Path, plan: &Plan) -> Result<()> {
    validate_transaction(transaction, plan)?;
    // Keep the journal until all data is gone so interrupted cleanup can be retried.
    for child in ["staged", "backup"] {
        let path = transaction.join(child);
        if exists(&path)? {
            remove_tree(&path)?;
            #[cfg(all(test, windows, feature = "no-self-update"))]
            if let Some(root) = transaction.parent() {
                process_tests::checkpoint(
                    root,
                    if child == "staged" {
                        "cleanup_staged"
                    } else {
                        "cleanup_backup"
                    },
                );
            }
        }
    }
    for child in ["ready", "committed", "plan.yml"] {
        let path = transaction.join(child);
        if exists(&path)? {
            fs::remove_file(path)?;
        }
    }
    fs::remove_dir(transaction)?;
    Ok(())
}

// Only this process's recorded successful renames authorize deleting published destinations.
fn rollback(root: &Path, plan: &Plan, published: &[String]) -> Result<()> {
    let transaction = root.join(TRANSACTION);
    validate_transaction(&transaction, plan)?;
    for name in plan.previous.iter().chain(published) {
        if exists(&root.join(name))? {
            plain_tree(&root.join(name))?;
        }
    }
    for name in &plan.previous {
        if exists(&transaction.join("backup").join(name))?
            && exists(&root.join(name))?
            && !published.contains(name)
        {
            return Err(invalid(format!(
                "Recovery destination occupied; backup retained: {name}"
            ))
            .into());
        }
    }
    for name in published {
        if exists(&root.join(name))? {
            remove_tree(&root.join(name))?;
            #[cfg(all(test, windows, feature = "no-self-update"))]
            process_tests::checkpoint(root, "rollback_removed");
        }
    }
    for name in &plan.previous {
        let backup = transaction.join("backup").join(name);
        if exists(&backup)? {
            fs::rename(backup, root.join(name))?;
            #[cfg(all(test, windows, feature = "no-self-update"))]
            process_tests::checkpoint(root, "rollback_restored");
        }
    }
    cleanup_transaction(&transaction, plan)
}

/// A snapshot for inspection only. Journal claims never authorize deleting files.
#[derive(Debug, Serialize)]
pub struct RecoveryInspection {
    pub root: std::path::PathBuf,
    pub phase: String,
    pub previous: Vec<String>,
    pub incoming: Vec<String>,
    pub backups: Vec<String>,
    pub staged: Vec<String>,
    pub destinations: Vec<String>,
    pub restore_phase: Option<String>,
    pub retained_replacements: Vec<String>,
}

/// Inspect retained installation state without creating or changing any files.
/// Cooperating installers are excluded when their existing lock file is present.
pub fn inspect_recovery(directory: &Path) -> Result<RecoveryInspection> {
    if is_link(&fs::symlink_metadata(directory)?) || !directory.is_dir() {
        return Err(invalid("AddOns root must be an existing directory, not a link").into());
    }
    let root = directory.canonicalize()?;
    let lock_path = root.join(LOCK);
    let _lock = if exists(&lock_path)? {
        let metadata = fs::symlink_metadata(&lock_path)?;
        if is_link(&metadata) || !metadata.is_file() {
            return Err(invalid("Invalid install lock file").into());
        }
        let file = File::open(lock_path)?;
        file.try_lock()
            .map_err(|_| invalid("Addon installation is busy; retry inspection later"))?;
        Some(file)
    } else {
        None
    };
    let mut report = RecoveryInspection {
        root: root.clone(),
        phase: "none".into(),
        previous: vec![],
        incoming: vec![],
        backups: vec![],
        staged: vec![],
        destinations: vec![],
        restore_phase: None,
        retained_replacements: vec![],
    };
    let transaction = root.join(TRANSACTION);
    if !exists(&transaction)? {
        if exists(&root.join(RESTORE))? {
            return Err(invalid(
                "Orphaned recovery workspace retained; install journal is missing",
            )
            .into());
        }
        return Ok(report);
    }
    plain_tree(&transaction)?;
    let plan = read_plan(&transaction)?;
    validate_transaction(&transaction, &plan)?;
    report.phase = if marker(&transaction.join("committed"))? {
        "committed-marker"
    } else if marker(&transaction.join("ready"))? {
        "ready-marker"
    } else {
        "unmarked"
    }
    .into();
    for name in plan
        .previous
        .iter()
        .chain(&plan.incoming)
        .collect::<BTreeSet<_>>()
    {
        for (base, names) in [
            (transaction.join("backup"), &mut report.backups),
            (transaction.join("staged"), &mut report.staged),
            (root.clone(), &mut report.destinations),
        ] {
            let path = base.join(name);
            if exists(&path)? {
                let metadata = fs::symlink_metadata(path)?;
                if is_link(&metadata) || !metadata.is_dir() {
                    return Err(invalid("Unexpected recovery folder type; state retained").into());
                }
                names.push(name.clone());
            }
        }
    }
    report.previous = plan.previous;
    report.incoming = plan.incoming;
    let workspace = root.join(RESTORE);
    if exists(&workspace)? {
        plain_tree(&workspace)?;
        let restore = read_plan(&workspace)?;
        validate_transaction(&workspace, &restore)?;
        report.restore_phase = Some(
            if marker(&workspace.join("committed"))? {
                "restoring-or-restored"
            } else if marker(&workspace.join("ready"))? {
                "retaining-current"
            } else {
                "incomplete-preparation"
            }
            .into(),
        );
        for name in restore.previous {
            if exists(&workspace.join("backup").join(&name))? {
                report.retained_replacements.push(name);
            }
        }
    }
    Ok(report)
}

fn read_plan(transaction: &Path) -> Result<Plan> {
    let plan_path = transaction.join("plan.yml");
    let file = File::open(plan_path)?;
    if file.metadata()?.len() > 1024 * 1024 {
        return Err(invalid("Oversized install journal").into());
    }
    let plan: Plan = serde_yaml::from_reader(file)?;
    for names in [&plan.previous, &plan.incoming] {
        let mut seen = HashSet::new();
        for name in names {
            folder_name(name)?;
            if !seen.insert(name.to_lowercase()) {
                return Err(invalid("Duplicate folder in install journal").into());
            }
        }
    }
    if plan.version != 1
        || plan.incoming.is_empty()
        || plan.incoming.len() > MAX_ENTRIES
        || plan.previous.len() > MAX_ENTRIES
    {
        return Err(invalid("Invalid or unsupported install journal").into());
    }
    for name in &plan.incoming {
        if plan
            .previous
            .iter()
            .any(|old| old != name && old.to_lowercase() == name.to_lowercase())
        {
            return Err(invalid("Case collision in install journal").into());
        }
    }
    Ok(plan)
}

fn validate_restore_state(
    root: &Path,
    transaction: &Path,
    workspace: &Path,
    original: &Plan,
    plan: &Plan,
    resuming: bool,
    restoring: bool,
) -> Result<()> {
    if is_link(&fs::symlink_metadata(root)?) {
        return Err(invalid("Recovery root changed; all versions retained").into());
    }
    plain_tree(transaction)?;
    if read_plan(transaction)? != *original {
        return Err(invalid("Install journal changed; review recovery again").into());
    }
    validate_transaction(transaction, original)?;
    if !marker(&transaction.join("ready"))? || exists(workspace)? != resuming {
        return Err(invalid("Recovery state changed; review recovery again").into());
    }
    if resuming {
        plain_tree(workspace)?;
        if read_plan(workspace)? != *plan {
            return Err(invalid("Recovery journal changed; all versions retained").into());
        }
        validate_transaction(workspace, plan)?;
        if marker(&workspace.join("committed"))? != restoring
            || marker(&workspace.join("ready"))? == restoring
        {
            return Err(invalid("Recovery phase changed; all versions retained").into());
        }
    }
    // Validate every source and destination before any move. Never guess at missing originals.
    for name in &plan.previous {
        for path in [root.join(name), workspace.join("backup").join(name)] {
            if exists(&path)? {
                if !path.is_dir() {
                    return Err(invalid("Recovery path is not a folder").into());
                }
                plain_tree(&path)?;
            }
        }
        if !restoring && exists(&root.join(name))? && exists(&workspace.join("backup").join(name))?
        {
            return Err(invalid("Recovery destination conflict; all versions retained").into());
        }
    }
    for name in &plan.incoming {
        let backup = transaction.join("backup").join(name);
        if exists(&backup)? {
            if !backup.is_dir() {
                return Err(invalid("Original backup is not a folder").into());
            }
            plain_tree(&backup)?;
            if restoring && exists(&root.join(name))? {
                return Err(
                    invalid("Restore destination occupied; original backup retained").into(),
                );
            }
        } else {
            return Err(invalid("Original backup missing; all versions retained").into());
        }
    }
    Ok(())
}

/// Restore the complete original bundle after an explicit, locked preview.
/// Current destinations are moved aside, never deleted. Retained state blocks updates.
pub fn restore_originals(
    directory: &Path,
    confirm: impl FnOnce(&[String], &[String]) -> bool,
) -> Result<bool> {
    if is_link(&fs::symlink_metadata(directory)?) || !directory.is_dir() {
        return Err(invalid("AddOns root must be an existing directory, not a link").into());
    }
    let root = directory.canonicalize()?;
    let lock_path = root.join(LOCK);
    let metadata = fs::symlink_metadata(&lock_path)?;
    if is_link(&metadata) || !metadata.is_file() {
        return Err(invalid("Invalid install lock file").into());
    }
    let lock = File::options().read(true).write(true).open(lock_path)?;
    lock.try_lock()
        .map_err(|_| invalid("Installation is busy; retry recovery later"))?;
    let transaction = root.join(TRANSACTION);
    plain_tree(&transaction)?;
    let original = read_plan(&transaction)?;
    validate_transaction(&transaction, &original)?;
    if original.previous.is_empty() || !marker(&transaction.join("ready"))? {
        return Err(invalid("No interrupted original bundle is available to restore").into());
    }
    let workspace = root.join(RESTORE);
    let names: Vec<String> = original
        .previous
        .iter()
        .chain(&original.incoming)
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let plan = Plan {
        version: 1,
        previous: names,
        incoming: original.previous.clone(),
    };
    let resuming = exists(&workspace)?;
    let restoring = if resuming {
        plain_tree(&workspace)?;
        let saved = read_plan(&workspace)?;
        if saved.previous != plan.previous || saved.incoming != plan.incoming {
            return Err(invalid("Recovery plan changed; all versions retained").into());
        }
        validate_transaction(&workspace, &plan)?;
        if !marker(&workspace.join("ready"))? && !marker(&workspace.join("committed"))? {
            return Err(invalid("Incomplete recovery preparation; all versions retained").into());
        }
        marker(&workspace.join("committed"))?
    } else {
        false
    };
    validate_restore_state(
        &root,
        &transaction,
        &workspace,
        &original,
        &plan,
        resuming,
        restoring,
    )?;
    if !confirm(&plan.incoming, &plan.previous) {
        return Ok(false);
    }
    validate_restore_state(
        &root,
        &transaction,
        &workspace,
        &original,
        &plan,
        resuming,
        restoring,
    )?;
    if !resuming {
        fs::create_dir(&workspace)?;
        let file = File::create_new(workspace.join("plan.yml"))?;
        serde_yaml::to_writer(&file, &plan)?;
        file.sync_all()?;
        fs::create_dir(workspace.join("backup"))?;
        fs::create_dir(workspace.join("staged"))?;
        File::create_new(workspace.join("ready"))?.sync_all()?;
    }
    if !restoring {
        for name in &plan.previous {
            let current = root.join(name);
            if exists(&current)? {
                if !current.is_dir() {
                    return Err(
                        invalid("Current folder type changed; all versions retained").into(),
                    );
                }
                let retained = workspace.join("backup").join(name);
                if exists(&retained)? {
                    return Err(
                        invalid("Retention destination changed; all versions retained").into(),
                    );
                }
                plain_tree(&current)?;
                fs::rename(current, retained)?;
                #[cfg(all(test, windows, feature = "no-self-update"))]
                process_tests::checkpoint(&root, "restore-retained");
            }
        }
        fs::rename(workspace.join("ready"), workspace.join("committed"))?;
    }
    for name in &plan.incoming {
        let backup = transaction.join("backup").join(name);
        if !exists(&backup)? {
            return Err(invalid(
                "Original source changed; inspect retained versions before continuing",
            )
            .into());
        }
        if !backup.is_dir() {
            return Err(invalid("Original folder type changed; all versions retained").into());
        }
        {
            if exists(&root.join(name))? {
                return Err(
                    invalid("Restore destination occupied; original backup retained").into(),
                );
            }
            plain_tree(&backup)?;
            fs::rename(backup, root.join(name))?;
            #[cfg(all(test, windows, feature = "no-self-update"))]
            process_tests::checkpoint(&root, "restore-original");
        }
    }
    // Deliberately retain the journal and replacement folders until cleanup is explicitly reviewed.
    Ok(true)
}

// Restart recovery never deletes an AddOns destination based on journal claims alone.
fn recover(root: &Path) -> Result<()> {
    if exists(&root.join(RESTORE))? {
        return Err(
            invalid("Restored versions retained; explicit recovery cleanup is required").into(),
        );
    }
    let transaction = root.join(TRANSACTION);
    if !exists(&transaction)? {
        return Ok(());
    }
    plain_tree(&transaction)?;
    let plan_path = transaction.join("plan.yml");
    if !exists(&plan_path)? {
        fs::remove_dir(&transaction)?; // Only an empty, interrupted directory creation is safe.
        return Ok(());
    }
    let plan = read_plan(&transaction)?;
    let staging = transaction.join("staged");
    let backups = transaction.join("backup");
    validate_transaction(&transaction, &plan)?;
    let ready = marker(&transaction.join("ready"))?;
    let committed = marker(&transaction.join("committed"))?;
    if ready {
        for name in &plan.incoming {
            if !exists(&staging.join(name))? && exists(&root.join(name))? {
                return Err(invalid("Interrupted publication needs explicit recovery; destinations and backups retained").into());
            }
        }
        for name in &plan.previous {
            if !exists(&backups.join(name))? && !exists(&root.join(name))? {
                return Err(invalid(
                    "Original missing from destination and backup; state retained",
                )
                .into());
            }
        }
        return rollback(root, &plan, &[]);
    } else if committed {
        if exists(&staging)? && fs::read_dir(&staging)?.next().is_some() {
            return Err(invalid(
                "Committed transaction still has staged folders; backups retained",
            )
            .into());
        }
        for name in &plan.incoming {
            if !root.join(name).is_dir() {
                return Err(invalid("Committed addon is missing; backups retained").into());
            }
            plain_tree(&root.join(name))?;
        }
        for name in &plan.previous {
            if !plan.incoming.contains(name) && exists(&root.join(name))? {
                return Err(invalid(
                    "Committed transaction has an obsolete destination; backups retained",
                )
                .into());
            }
        }
    } else if exists(&backups)? && fs::read_dir(&backups)?.next().is_some() {
        return Err(invalid("Unrecognized install state; backups retained").into());
    }
    cleanup_transaction(&transaction, &plan)
}

fn commit(root: &Path, plan: &Plan, published: &mut Vec<String>) -> Result<()> {
    let transaction = root.join(TRANSACTION);
    for name in &plan.previous {
        fs::rename(root.join(name), transaction.join("backup").join(name))?;
        #[cfg(all(test, windows, feature = "no-self-update"))]
        process_tests::checkpoint(root, "backup");
    }
    for name in &plan.incoming {
        if exists(&root.join(name))? {
            return Err(invalid(format!("Install destination occupied: {name}")).into());
        }
        fs::rename(transaction.join("staged").join(name), root.join(name))?;
        published.push(name.clone());
        #[cfg(all(test, windows, feature = "no-self-update"))]
        process_tests::checkpoint(root, "published");
    }
    // Atomic marker rename separates rollback from cleanup after process interruption.
    // ponytail: filesystem power-loss guarantees and hostile concurrent writers need platform-specific work.
    #[cfg(all(test, windows, feature = "no-self-update"))]
    process_tests::checkpoint(root, "before_commit");
    fs::rename(transaction.join("ready"), transaction.join("committed"))?;
    #[cfg(all(test, windows, feature = "no-self-update"))]
    process_tests::checkpoint(root, "committed");
    Ok(())
}

fn finish(root: &Path, plan: &Plan) -> Result<()> {
    let mut published = vec![];
    if let Err(error) = commit(root, plan, &mut published) {
        if let Err(recovery) = rollback(root, plan, &published) {
            return Err(invalid(format!(
                "Install failed: {error}; recovery failed: {recovery}. Inspect retained transaction at {:?}",
                root.join(TRANSACTION)
            ))
            .into());
        }
        return Err(error);
    }
    if let Err(error) = recover(root) {
        log::warn!("Addon installed; transaction cleanup pending: {}", error);
    }
    Ok(())
}

/// Validate and stage a complete ZIP, then replace only this addon's owned folders.
/// Failures roll back known writes; ambiguous interrupted publication preserves both versions.
pub async fn install_addon(
    addon: &Addon,
    from_directory: &Path,
    to_directory: &Path,
) -> Result<Vec<AddonFolder>> {
    install_addon_checked(addon, from_directory, to_directory, |_, addons| Ok(addons)).await
}

pub(crate) async fn install_addon_checked(
    addon: &Addon,
    from_directory: &Path,
    to_directory: &Path,
    validate_staged: impl FnOnce(&Path, Vec<AddonFolder>) -> Result<Vec<AddonFolder>>,
) -> Result<Vec<AddonFolder>> {
    component(&addon.primary_folder_id)?;
    let zip_path = from_directory.join(&addon.primary_folder_id);
    let zip_file = File::open(&zip_path)?;
    if zip_file.metadata()?.len() > MAX_TOTAL {
        return Err(invalid("Archive exceeds the 512 MiB input limit").into());
    }
    let mut archive = zip::ZipArchive::new(zip_file)?;
    let incoming = validate_archive(&mut archive)?; // Reject unsafe paths before any writes.
    if is_link(&fs::symlink_metadata(to_directory)?) {
        return Err(invalid("AddOns root is a link/reparse point").into());
    }
    let root = to_directory.canonicalize()?;
    let lock_path = root.join(LOCK);
    if exists(&lock_path)? && is_link(&fs::symlink_metadata(&lock_path)?) {
        return Err(invalid("Install lock is a link/reparse point").into());
    }
    let lock = File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path)?;
    lock.try_lock().map_err(|error| {
        io::Error::other(format!(
            "Another install is active or the filesystem cannot lock: {error}"
        ))
    })?;
    #[cfg(all(test, windows, feature = "no-self-update"))]
    process_tests::checkpoint(&root, "locked");
    recover(&root)?;
    let plan = make_plan(addon, &root, incoming)?;
    let transaction = root.join(TRANSACTION);
    fs::create_dir(&transaction)?;
    let prepared = (|| -> Result<Vec<AddonFolder>> {
        let journal = File::create_new(transaction.join("plan.yml"))?;
        serde_yaml::to_writer(&journal, &plan)?;
        journal.sync_all()?;
        fs::create_dir(transaction.join("backup"))?;
        let addons = stage(&mut archive, &transaction.join("staged"), &plan.incoming)?;
        let addons = validate_staged(&transaction.join("staged"), addons)?;
        File::create_new(transaction.join("ready"))?.sync_all()?;
        Ok(addons)
    })();
    let mut addons = match prepared {
        Ok(addons) => addons,
        Err(error) => {
            if let Err(cleanup) = cleanup_transaction(&transaction, &plan) {
                log::warn!("Staging cleanup pending: {}", cleanup);
            }
            return Err(error);
        }
    };
    #[cfg(all(test, windows, feature = "no-self-update"))]
    process_tests::checkpoint(&root, "ready");
    finish(&root, &plan)?;
    for addon in &mut addons {
        addon.path = root.join(&addon.id);
    }
    drop(archive);
    if let Err(error) = fs::remove_file(&zip_path) {
        log::warn!("Addon installed; archive cleanup pending: {}", error);
    }
    Ok(addons)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::Write, path::PathBuf};
    use tempfile::{tempdir, TempDir};
    use zip::{write::FileOptions, CompressionMethod, ZipWriter};

    fn fixture() -> (TempDir, Addon, PathBuf, PathBuf) {
        let temp = tempdir().unwrap();
        let root = temp.path().join("AddOns");
        let downloads = temp.path().join("downloads");
        fs::create_dir_all(root.join("Foo")).unwrap();
        fs::create_dir(&downloads).unwrap();
        fs::write(root.join("Foo/old.lua"), b"original").unwrap();
        let mut addon = Addon::empty("Foo");
        addon.folders.push(AddonFolder {
            id: "Foo".into(),
            path: root.join("Foo"),
            ..Default::default()
        });
        (temp, addon, downloads, root)
    }

    #[test]
    fn inspection_preserves_clean_and_ambiguous_state() {
        let (_temp, _addon, _downloads, root) = fixture();
        assert_eq!(inspect_recovery(&root).unwrap().phase, "none");
        assert!(!root.join(LOCK).exists());
        let transaction = pending(
            &root,
            &Plan {
                version: 1,
                previous: vec!["Foo".into()],
                incoming: vec!["Foo".into()],
            },
        );
        fs::create_dir(transaction.join("backup/Foo")).unwrap();
        fs::write(transaction.join("backup/Foo/old.lua"), b"backup").unwrap();
        let report = inspect_recovery(&root).unwrap();
        assert_eq!(report.phase, "ready-marker");
        assert_eq!(report.backups, ["Foo"]);
        assert_eq!(report.destinations, ["Foo"]);
        assert!(report.staged.is_empty());
        assert_eq!(fs::read(root.join("Foo/old.lua")).unwrap(), b"original");
        assert_eq!(
            fs::read(transaction.join("backup/Foo/old.lua")).unwrap(),
            b"backup"
        );
        fs::remove_file(transaction.join("ready")).unwrap();
        fs::create_dir(transaction.join("committed")).unwrap();
        assert!(inspect_recovery(&root).is_err());
        assert!(transaction.join("backup/Foo/old.lua").exists());
    }

    fn restore_fixture() -> (TempDir, PathBuf, PathBuf) {
        let (temp, _addon, _downloads, root) = fixture();
        File::create(root.join(LOCK)).unwrap();
        let transaction = pending(
            &root,
            &Plan {
                version: 1,
                previous: vec!["Foo".into()],
                incoming: vec!["Foo".into()],
            },
        );
        fs::rename(root.join("Foo"), transaction.join("backup/Foo")).unwrap();
        fs::create_dir(root.join("Foo")).unwrap();
        fs::write(root.join("Foo/new.lua"), b"replacement").unwrap();
        (temp, root, transaction)
    }

    #[test]
    fn restore_revalidates_source_and_journal_after_confirmation() {
        for change_journal in [false, true] {
            let (temp, root, transaction) = restore_fixture();
            let result = restore_originals(&root, |_, _| {
                if change_journal {
                    fs::write(
                        transaction.join("plan.yml"),
                        b"version: 99\nprevious: [Foo]\nincoming: [Foo]\n",
                    )
                    .unwrap();
                } else {
                    fs::rename(
                        transaction.join("backup/Foo"),
                        temp.path().join("moved-original"),
                    )
                    .unwrap();
                }
                true
            });
            assert!(result.is_err());
            assert_eq!(fs::read(root.join("Foo/new.lua")).unwrap(), b"replacement");
            assert!(!root.join(RESTORE).exists());
            let original = if change_journal {
                transaction.join("backup/Foo/old.lua")
            } else {
                temp.path().join("moved-original/old.lua")
            };
            assert_eq!(fs::read(original).unwrap(), b"original");
        }
    }

    #[test]
    fn restore_never_accepts_unverified_destination_as_original() {
        let (temp, root, transaction) = restore_fixture();
        fs::rename(
            transaction.join("backup/Foo"),
            temp.path().join("moved-original"),
        )
        .unwrap();
        let workspace = root.join(RESTORE);
        fs::create_dir_all(workspace.join("backup/Foo")).unwrap();
        fs::create_dir(workspace.join("staged")).unwrap();
        fs::copy(transaction.join("plan.yml"), workspace.join("plan.yml")).unwrap();
        fs::write(workspace.join("committed"), b"").unwrap();
        fs::write(workspace.join("backup/Foo/new.lua"), b"retained").unwrap();
        assert!(restore_originals(&root, |_, _| panic!(
            "missing originals must not be offered"
        ))
        .is_err());
        assert_eq!(fs::read(root.join("Foo/new.lua")).unwrap(), b"replacement");
        assert_eq!(
            fs::read(workspace.join("backup/Foo/new.lua")).unwrap(),
            b"retained"
        );
    }

    #[test]
    fn inspection_reports_orphaned_restore_workspace_as_error() {
        let (_temp, _addon, _downloads, root) = fixture();
        let retained = root.join(RESTORE).join("backup/Foo");
        fs::create_dir_all(&retained).unwrap();
        fs::write(retained.join("new.lua"), b"retained").unwrap();
        assert!(inspect_recovery(&root).is_err());
        assert_eq!(fs::read(retained.join("new.lua")).unwrap(), b"retained");
    }

    #[test]
    fn explicit_restore_preserves_versions_and_can_resume() {
        let (_temp, _addon, _downloads, root) = fixture();
        File::create(root.join(LOCK)).unwrap();
        let transaction = pending(
            &root,
            &Plan {
                version: 1,
                previous: vec!["Foo".into()],
                incoming: vec!["Foo".into(), "New".into()],
            },
        );
        fs::rename(root.join("Foo"), transaction.join("backup/Foo")).unwrap();
        fs::create_dir(root.join("Foo")).unwrap();
        fs::write(root.join("Foo/new.lua"), b"replacement").unwrap();
        fs::create_dir(root.join("New")).unwrap();
        fs::write(root.join("New/new.lua"), b"additional").unwrap();
        assert!(!restore_originals(&root, |_, _| false).unwrap());
        assert!(!root.join(RESTORE).exists());
        assert!(root.join("Foo/new.lua").exists());
        assert!(restore_originals(&root, |originals, _| originals == ["Foo"]).unwrap());
        assert_eq!(fs::read(root.join("Foo/old.lua")).unwrap(), b"original");
        assert_eq!(
            fs::read(root.join(RESTORE).join("backup/Foo/new.lua")).unwrap(),
            b"replacement"
        );
        assert_eq!(
            fs::read(root.join(RESTORE).join("backup/New/new.lua")).unwrap(),
            b"additional"
        );
        assert!(!root.join("New").exists());
        assert!(restore_originals(&root, |_, _| true).is_err());
        let inspection = inspect_recovery(&root).unwrap();
        assert_eq!(
            inspection.restore_phase.as_deref(),
            Some("restoring-or-restored")
        );
        assert_eq!(inspection.retained_replacements, ["Foo", "New"]);
        assert!(recover(&root).is_err());
    }

    #[test]
    fn explicit_restore_refuses_missing_originals_before_confirmation() {
        let (_temp, _addon, _downloads, root) = fixture();
        File::create(root.join(LOCK)).unwrap();
        pending(
            &root,
            &Plan {
                version: 1,
                previous: vec!["Foo".into()],
                incoming: vec!["Foo".into()],
            },
        );
        assert!(
            restore_originals(&root, |_, _| panic!("invalid state must not be offered")).is_err()
        );
        assert!(!root.join(RESTORE).exists());
        assert_eq!(fs::read(root.join("Foo/old.lua")).unwrap(), b"original");
    }

    #[test]
    fn explicit_restore_rejects_busy_and_conflicting_state_without_overwrite() {
        let (_temp, _addon, _downloads, root) = fixture();
        let lock = File::create(root.join(LOCK)).unwrap();
        lock.lock().unwrap();
        assert!(restore_originals(&root, |_, _| panic!("busy state")).is_err());
        drop(lock);
        let plan = Plan {
            version: 1,
            previous: vec!["Foo".into()],
            incoming: vec!["Foo".into()],
        };
        let transaction = pending(&root, &plan);
        fs::create_dir(transaction.join("backup/Foo")).unwrap();
        fs::write(transaction.join("backup/Foo/old.lua"), b"backup").unwrap();
        let workspace = root.join(RESTORE);
        fs::create_dir_all(workspace.join("backup/Foo")).unwrap();
        fs::create_dir(workspace.join("staged")).unwrap();
        fs::write(
            workspace.join("plan.yml"),
            serde_yaml::to_string(&plan).unwrap(),
        )
        .unwrap();
        fs::write(workspace.join("ready"), b"").unwrap();
        fs::write(workspace.join("backup/Foo/retained.lua"), b"retained").unwrap();
        assert!(restore_originals(&root, |_, _| panic!("conflict must not be offered")).is_err());
        assert_eq!(fs::read(root.join("Foo/old.lua")).unwrap(), b"original");
        assert_eq!(
            fs::read(transaction.join("backup/Foo/old.lua")).unwrap(),
            b"backup"
        );
        assert_eq!(
            fs::read(workspace.join("backup/Foo/retained.lua")).unwrap(),
            b"retained"
        );
    }

    #[test]
    fn inspection_respects_existing_lock_without_creating_state() {
        let (_temp, _addon, _downloads, root) = fixture();
        let file = File::create(root.join(LOCK)).unwrap();
        file.lock().unwrap();
        assert!(inspect_recovery(&root).is_err());
        drop(file);
        assert!(inspect_recovery(&root).is_ok());
        assert_eq!(fs::read(root.join(LOCK)).unwrap(), b"");
    }

    pub(super) fn archive(path: &Path, entries: &[(&str, &[u8])]) {
        let mut writer = ZipWriter::new(File::create(path).unwrap());
        for (name, data) in entries {
            writer
                .start_file(
                    *name,
                    FileOptions::default().compression_method(CompressionMethod::Stored),
                )
                .unwrap();
            writer.write_all(data).unwrap();
        }
        writer.finish().unwrap();
    }

    fn install(addon: &Addon, downloads: &Path, root: &Path) -> Result<Vec<AddonFolder>> {
        async_std::task::block_on(install_addon(addon, downloads, root))
    }

    fn pending(root: &Path, plan: &Plan) -> PathBuf {
        let transaction = root.join(TRANSACTION);
        fs::create_dir(&transaction).unwrap();
        fs::create_dir(transaction.join("backup")).unwrap();
        fs::create_dir(transaction.join("staged")).unwrap();
        fs::write(
            transaction.join("plan.yml"),
            serde_yaml::to_string(plan).unwrap(),
        )
        .unwrap();
        File::create_new(transaction.join("ready")).unwrap();
        transaction
    }

    #[test]
    fn replaces_owned_bundle_and_preserves_unrelated_data() {
        let (temp, mut addon, downloads, root) = fixture();
        fs::create_dir(root.join("Obsolete")).unwrap();
        fs::write(root.join("Obsolete/old.lua"), b"obsolete").unwrap();
        addon.folders.push(AddonFolder {
            id: "Obsolete".into(),
            path: root.join("Obsolete"),
            ..Default::default()
        });
        fs::create_dir(root.join("Unrelated")).unwrap();
        fs::write(root.join("Unrelated/keep.lua"), b"keep").unwrap();
        fs::create_dir(temp.path().join("WTF")).unwrap();
        fs::write(temp.path().join("WTF/saved.lua"), b"settings").unwrap();
        archive(
            &downloads.join("Foo"),
            &[
                ("Foo/Foo.toc", b"## Title: Foo\n## Version: 2"),
                ("Foo/new.lua", b"new"),
                ("Extra/Extra.toc", b"## Title: Extra"),
            ],
        );
        let installed = install(&addon, &downloads, &root).unwrap();
        assert_eq!(installed.len(), 2);
        assert!(installed
            .iter()
            .all(|folder| folder.path == root.canonicalize().unwrap().join(&folder.id)));
        assert_eq!(fs::read(root.join("Foo/new.lua")).unwrap(), b"new");
        assert!(!root.join("Foo/old.lua").exists());
        assert!(!root.join("Obsolete").exists());
        assert_eq!(fs::read(root.join("Unrelated/keep.lua")).unwrap(), b"keep");
        assert_eq!(
            fs::read(temp.path().join("WTF/saved.lua")).unwrap(),
            b"settings"
        );
        assert!(!root.join(TRANSACTION).exists());
        assert!(!downloads.join("Foo").exists());
    }

    #[test]
    fn rejects_unsafe_paths_before_any_destination_write() {
        for name in [
            "../escape.lua",
            "/escape.lua",
            "C:/escape.lua",
            "//host/share.lua",
            "Foo/../escape.lua",
            "Foo\\escape.lua",
            "Foo/x:stream",
            "Foo/NUL.txt",
            "Foo/NUL .txt",
            "Foo/CONIN$",
            "Foo/COM1",
            "Foo/trailing.",
            "Foo/trailing ",
            "Foo//file.lua",
            ".patchloom-install/file.lua",
            "WTF/file.lua",
            "SavedVariables/file.lua",
            "loose.lua",
        ] {
            let (_temp, addon, downloads, root) = fixture();
            archive(
                &downloads.join("Foo"),
                &[("Foo/Foo.toc", b"valid"), (name, b"bad")],
            );
            assert!(
                install(&addon, &downloads, &root).is_err(),
                "accepted {}",
                name
            );
            assert_eq!(fs::read(root.join("Foo/old.lua")).unwrap(), b"original");
            assert!(!root.join(LOCK).exists(), "wrote lock for {}", name);
            assert!(!root.join(TRANSACTION).exists());
        }
    }

    #[test]
    fn rejects_duplicate_case_and_file_directory_collisions() {
        for entries in [
            vec![
                ("Foo/Foo.toc", b"toc".as_slice()),
                ("foo/other.lua", b"other"),
            ],
            vec![("Foo/Foo.toc", b"one".as_slice()), ("Foo/Foo.toc", b"two")],
            vec![
                ("Foo/path", b"file".as_slice()),
                ("Foo/path/child", b"child"),
            ],
        ] {
            let (_temp, addon, downloads, root) = fixture();
            archive(&downloads.join("Foo"), &entries);
            assert!(install(&addon, &downloads, &root).is_err());
            assert!(!root.join(LOCK).exists());
            assert_eq!(fs::read(root.join("Foo/old.lua")).unwrap(), b"original");
        }
    }

    #[test]
    fn corrupt_payload_and_missing_toc_preserve_originals() {
        let (_temp, addon, downloads, root) = fixture();
        let path = downloads.join("Foo");
        archive(
            &path,
            &[
                ("Foo/Foo.toc", b"## Title: Foo"),
                ("Foo/new.lua", b"CORRUPT_ME_UNIQUE"),
            ],
        );
        let mut bytes = fs::read(&path).unwrap();
        let index = bytes
            .windows(17)
            .position(|data| data == b"CORRUPT_ME_UNIQUE")
            .unwrap();
        bytes[index] ^= 1;
        fs::write(&path, bytes).unwrap();
        assert!(install(&addon, &downloads, &root).is_err());
        assert_eq!(fs::read(root.join("Foo/old.lua")).unwrap(), b"original");
        assert!(!root.join(TRANSACTION).exists());
        assert!(path.exists());
        archive(&path, &[("Foo/new.lua", b"no TOC")]);
        assert!(install(&addon, &downloads, &root).is_err());
        assert_eq!(fs::read(root.join("Foo/old.lua")).unwrap(), b"original");
        fs::write(&path, b"not a ZIP").unwrap();
        assert!(install(&addon, &downloads, &root).is_err());
        assert_eq!(fs::read(root.join("Foo/old.lua")).unwrap(), b"original");
    }

    #[test]
    fn rejects_archive_links_and_declared_size_limit() {
        for symlink in [true, false] {
            let (_temp, addon, downloads, root) = fixture();
            let path = downloads.join("Foo");
            archive(&path, &[("Foo/Foo.toc", b"## Title: Foo")]);
            let mut bytes = fs::read(&path).unwrap();
            let header = bytes
                .windows(4)
                .position(|data| data == b"PK\x01\x02")
                .unwrap();
            if symlink {
                bytes[header + 5] = 3; // Unix symlink mode in central-directory attributes.
                bytes[header + 38..header + 42].copy_from_slice(&(0o120777u32 << 16).to_le_bytes());
            } else {
                bytes[header + 24..header + 28]
                    .copy_from_slice(&((MAX_FILE + 1) as u32).to_le_bytes());
            }
            fs::write(&path, bytes).unwrap();
            assert!(install(&addon, &downloads, &root).is_err());
            assert!(!root.join(LOCK).exists());
            assert_eq!(fs::read(root.join("Foo/old.lua")).unwrap(), b"original");
        }
    }

    #[test]
    fn refuses_unowned_conflicts_and_outside_owned_paths() {
        let (temp, mut addon, downloads, root) = fixture();
        fs::create_dir(root.join("Other")).unwrap();
        fs::write(root.join("Other/keep.lua"), b"keep").unwrap();
        archive(
            &downloads.join("Foo"),
            &[("Foo/Foo.toc", b"toc"), ("Other/Other.toc", b"toc")],
        );
        assert!(install(&addon, &downloads, &root).is_err());
        assert_eq!(fs::read(root.join("Other/keep.lua")).unwrap(), b"keep");
        addon.folders[0].path = temp.path().join("Foo");
        assert!(install(&addon, &downloads, &root).is_err());
        assert_eq!(fs::read(root.join("Foo/old.lua")).unwrap(), b"original");
    }

    #[test]
    fn failed_second_publish_rolls_back_the_whole_bundle() {
        let (_temp, _addon, _downloads, root) = fixture();
        let plan = Plan {
            version: 1,
            previous: vec!["Foo".into()],
            incoming: vec!["Foo".into(), "New".into()],
        };
        let transaction = pending(&root, &plan);
        fs::create_dir(transaction.join("staged/Foo")).unwrap();
        fs::write(transaction.join("staged/Foo/new.lua"), b"new").unwrap();
        // A missing second staged directory forces an IO error after Foo was replaced.
        assert!(finish(&root, &plan).is_err());
        assert_eq!(fs::read(root.join("Foo/old.lua")).unwrap(), b"original");
        assert!(!root.join("Foo/new.lua").exists());
        assert!(!root.join("New").exists());
        assert!(!transaction.exists());
    }

    #[test]
    fn interrupted_publication_preserves_both_versions_for_explicit_recovery() {
        let (_temp, addon, downloads, root) = fixture();
        let plan = Plan {
            version: 1,
            previous: vec!["Foo".into()],
            incoming: vec!["Foo".into(), "New".into()],
        };
        let transaction = pending(&root, &plan);
        for name in &plan.incoming {
            fs::create_dir(transaction.join("staged").join(name)).unwrap();
        }
        fs::rename(root.join("Foo"), transaction.join("backup/Foo")).unwrap();
        fs::rename(transaction.join("staged/Foo"), root.join("Foo")).unwrap();
        fs::rename(transaction.join("staged/New"), root.join("New")).unwrap();
        // Simulate process termination before the commit marker: no destructor rollback.
        archive(&downloads.join("Foo"), &[("Foo/Foo.toc", b"toc")]);
        assert!(install(&addon, &downloads, &root).is_err());
        assert_eq!(
            fs::read(transaction.join("backup/Foo/old.lua")).unwrap(),
            b"original"
        );
        assert!(root.join("Foo").is_dir());
        assert!(root.join("New").is_dir());
        assert!(downloads.join("Foo").exists());
        assert!(recover(&root).is_err());
    }

    #[test]
    fn committed_install_keeps_new_version_during_cleanup_recovery() {
        let (_temp, _addon, _downloads, root) = fixture();
        let plan = Plan {
            version: 1,
            previous: vec!["Foo".into()],
            incoming: vec!["Foo".into()],
        };
        let transaction = pending(&root, &plan);
        fs::create_dir(transaction.join("staged/Foo")).unwrap();
        fs::write(transaction.join("staged/Foo/new.lua"), b"new").unwrap();
        commit(&root, &plan, &mut vec![]).unwrap();
        fs::remove_dir(transaction.join("staged")).unwrap();
        fs::remove_file(transaction.join("backup/Foo/old.lua")).unwrap();
        recover(&root).unwrap();
        assert_eq!(fs::read(root.join("Foo/new.lua")).unwrap(), b"new");
        assert!(!root.join("Foo/old.lua").exists());
        assert!(!transaction.exists());
    }

    #[test]
    fn recovery_handles_each_interrupted_backup_publish_boundary() {
        for phase in 0..4 {
            let (_temp, _addon, _downloads, root) = fixture();
            let plan = Plan {
                version: 1,
                previous: vec!["Foo".into()],
                incoming: vec!["Foo".into()],
            };
            let transaction = pending(&root, &plan);
            fs::create_dir(transaction.join("staged/Foo")).unwrap();
            if phase == 0 {
                fs::remove_file(transaction.join("ready")).unwrap();
            }
            if phase >= 2 {
                fs::rename(root.join("Foo"), transaction.join("backup/Foo")).unwrap();
            }
            if phase >= 3 {
                fs::rename(transaction.join("staged/Foo"), root.join("Foo")).unwrap();
                assert!(recover(&root).is_err());
                assert_eq!(
                    fs::read(transaction.join("backup/Foo/old.lua")).unwrap(),
                    b"original"
                );
                assert!(root.join("Foo").is_dir());
                continue;
            }
            recover(&root).unwrap();
            recover(&root).unwrap();
            assert_eq!(fs::read(root.join("Foo/old.lua")).unwrap(), b"original");
            assert!(!transaction.exists());
        }
    }

    #[test]
    fn recovery_conflict_keeps_original_backup_and_can_be_retried() {
        let (_temp, _addon, _downloads, root) = fixture();
        let plan = Plan {
            version: 1,
            previous: vec!["Foo".into()],
            incoming: vec!["Foo".into()],
        };
        let transaction = pending(&root, &plan);
        fs::create_dir(transaction.join("staged/Foo")).unwrap();
        fs::rename(root.join("Foo"), transaction.join("backup/Foo")).unwrap();
        fs::create_dir(root.join("Foo")).unwrap(); // An external writer occupied the restore path.
        fs::write(root.join("Foo/keep.lua"), b"external").unwrap();
        assert!(recover(&root).is_err());
        assert_eq!(
            fs::read(transaction.join("backup/Foo/old.lua")).unwrap(),
            b"original"
        );
        assert_eq!(fs::read(root.join("Foo/keep.lua")).unwrap(), b"external");
        fs::remove_file(root.join("Foo/keep.lua")).unwrap();
        fs::remove_dir(root.join("Foo")).unwrap();
        recover(&root).unwrap();
        assert_eq!(fs::read(root.join("Foo/old.lua")).unwrap(), b"original");
    }

    #[test]
    fn rejects_linked_destination_root() {
        let (temp, addon, downloads, root) = fixture();
        let alias = temp.path().join("Alias");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&root, &alias).unwrap();
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            let output = std::process::Command::new("cmd.exe")
                .args(["/d", "/c", "mklink", "/J"])
                .arg(&alias)
                .arg(&root)
                .creation_flags(0x08000000)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        archive(&downloads.join("Foo"), &[("Foo/Foo.toc", b"toc")]);
        assert!(install(&addon, &downloads, &alias).is_err());
        assert_eq!(fs::read(root.join("Foo/old.lua")).unwrap(), b"original");
        assert!(!root.join(LOCK).exists());
    }

    #[test]
    fn lock_conflict_prevents_a_second_installer() {
        let (_temp, addon, downloads, root) = fixture();
        archive(&downloads.join("Foo"), &[("Foo/Foo.toc", b"toc")]);
        let lock = File::create(root.join(LOCK)).unwrap();
        lock.lock().unwrap();
        assert!(install(&addon, &downloads, &root).is_err());
        assert_eq!(fs::read(root.join("Foo/old.lua")).unwrap(), b"original");
        assert!(!root.join(TRANSACTION).exists());
        drop(lock);
        install(&addon, &downloads, &root).unwrap();
    }

    #[test]
    fn invalid_recovery_journal_preserves_all_data() {
        let (temp, _addon, _downloads, root) = fixture();
        let plan = Plan {
            version: 1,
            previous: vec!["../outside".into()],
            incoming: vec![],
        };
        let transaction = pending(&root, &plan);
        fs::write(temp.path().join("outside"), b"keep").unwrap();
        assert!(recover(&root).is_err());
        assert_eq!(fs::read(temp.path().join("outside")).unwrap(), b"keep");
        assert_eq!(fs::read(root.join("Foo/old.lua")).unwrap(), b"original");
        assert!(transaction.exists());
    }

    #[test]
    fn review_ready_journal_cannot_authorize_unrelated_deletion() {
        let (_temp, _addon, _downloads, root) = fixture();
        fs::create_dir(root.join("Unrelated")).unwrap();
        fs::write(root.join("Unrelated/Unrelated.toc"), b"toc").unwrap();
        fs::write(root.join("Unrelated/keep.lua"), b"keep").unwrap();
        let plan = Plan {
            version: 1,
            previous: vec![],
            incoming: vec!["Unrelated".into()],
        };
        let transaction = pending(&root, &plan);
        let result = recover(&root);
        assert!(result.is_err(), "ambiguous journal was accepted");
        assert_eq!(fs::read(root.join("Unrelated/keep.lua")).unwrap(), b"keep");
        assert!(transaction.exists());
    }

    #[test]
    fn review_invalid_committed_marker_preserves_original_backup() {
        let (_temp, _addon, _downloads, root) = fixture();
        let plan = Plan {
            version: 1,
            previous: vec!["Foo".into()],
            incoming: vec!["Foo".into()],
        };
        let transaction = pending(&root, &plan);
        fs::create_dir(transaction.join("staged/Foo")).unwrap();
        fs::write(transaction.join("staged/Foo/new.lua"), b"new").unwrap();
        fs::rename(root.join("Foo"), transaction.join("backup/Foo")).unwrap();
        fs::remove_file(transaction.join("ready")).unwrap();
        fs::create_dir(transaction.join("committed")).unwrap();
        assert!(recover(&root).is_err());
        assert_eq!(
            fs::read(transaction.join("backup/Foo/old.lua")).unwrap(),
            b"original"
        );
        assert!(transaction.join("staged/Foo/new.lua").exists());
    }

    #[test]
    fn review_committed_marker_requires_complete_publication() {
        let (_temp, _addon, _downloads, root) = fixture();
        let plan = Plan {
            version: 1,
            previous: vec!["Foo".into()],
            incoming: vec!["Foo".into()],
        };
        let transaction = pending(&root, &plan);
        fs::create_dir(transaction.join("staged/Foo")).unwrap();
        fs::rename(root.join("Foo"), transaction.join("backup/Foo")).unwrap();
        fs::remove_file(transaction.join("ready")).unwrap();
        File::create_new(transaction.join("committed")).unwrap();
        assert!(recover(&root).is_err());
        assert_eq!(
            fs::read(transaction.join("backup/Foo/old.lua")).unwrap(),
            b"original"
        );
    }

    #[test]
    fn review_unknown_backup_entry_is_retained_before_any_cleanup() {
        let (_temp, _addon, _downloads, root) = fixture();
        let plan = Plan {
            version: 1,
            previous: vec!["Foo".into()],
            incoming: vec!["Foo".into()],
        };
        let transaction = pending(&root, &plan);
        fs::create_dir(transaction.join("staged/Foo")).unwrap();
        fs::create_dir(transaction.join("backup/Unexpected")).unwrap();
        fs::write(transaction.join("backup/Unexpected/keep.lua"), b"keep").unwrap();
        assert!(recover(&root).is_err());
        assert_eq!(
            fs::read(transaction.join("backup/Unexpected/keep.lua")).unwrap(),
            b"keep"
        );
        assert_eq!(fs::read(root.join("Foo/old.lua")).unwrap(), b"original");
        assert!(transaction.join("staged/Foo").exists());
    }

    #[test]
    fn in_process_failure_removes_only_positively_recorded_new_publications() {
        let (_temp, _addon, _downloads, root) = fixture();
        let plan = Plan {
            version: 1,
            previous: vec!["Foo".into()],
            incoming: vec!["New".into(), "Missing".into()],
        };
        let transaction = pending(&root, &plan);
        fs::create_dir(transaction.join("staged/New")).unwrap();
        fs::write(transaction.join("staged/New/new.lua"), b"new").unwrap();
        assert!(finish(&root, &plan).is_err());
        assert!(!root.join("New").exists());
        assert_eq!(fs::read(root.join("Foo/old.lua")).unwrap(), b"original");
        assert!(!transaction.exists());
    }
}
