//! Content-bound, append-only ownership receipts, 2026-10-27; original GPLv3 applies.
use super::install::{component, folder_name, is_link};
use crate::addon::AddonFolder;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

const STORE: &str = ".patchloom-ownership";
const LIMIT: u64 = 1024 * 1024;
fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[derive(Debug, Serialize)]
pub struct OwnershipPreview {
    pub id: String,
    pub root: PathBuf,
    pub source: serde_json::Value,
    pub folders: Vec<String>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    format: u8,
    root: PathBuf,
    source: serde_json::Value,
    folders: Vec<String>,
    files: BTreeMap<String, String>,
}
fn plain(path: &Path, directory: bool) -> io::Result<()> {
    let m = fs::symlink_metadata(path)?;
    if is_link(&m) || (directory && !m.is_dir()) || (!directory && !m.is_file()) {
        return Err(invalid("Ownership path is a link or has the wrong type"));
    }
    Ok(())
}
fn read(path: &Path) -> io::Result<Vec<u8>> {
    plain(path, false)?;
    let mut bytes = vec![];
    File::open(path)?.take(LIMIT + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > LIMIT {
        return Err(invalid("Ownership metadata exceeds its read limit"));
    }
    Ok(bytes)
}
fn identifier(id: &str) -> io::Result<()> {
    if id.len() != 64
        || !id
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
    {
        return Err(invalid("Invalid ownership receipt identifier"));
    }
    Ok(())
}
fn root_path(root: &Path) -> io::Result<PathBuf> {
    plain(root, true)?;
    root.canonicalize()
}
fn folders(names: &[String]) -> io::Result<()> {
    if names.is_empty() || names.len() > 10_000 {
        return Err(invalid("Invalid ownership folder count"));
    }
    let mut seen = BTreeSet::new();
    for name in names {
        folder_name(name)?;
        if !seen.insert(name.to_lowercase()) {
            return Err(invalid("Ambiguous ownership folder names"));
        }
    }
    if names.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err(invalid("Ownership folders must be uniquely sorted"));
    }
    Ok(())
}
fn snapshot(root: &Path, names: &[String]) -> io::Result<BTreeMap<String, String>> {
    folders(names)?;
    let mut files = BTreeMap::new();
    let mut total = 0u64;
    for name in names {
        plain(&root.join(name), true)?;
        for entry in walkdir::WalkDir::new(root.join(name)).follow_links(false) {
            let entry = entry.map_err(io::Error::other)?;
            if files.len() >= 10_000 {
                return Err(invalid("Ownership snapshot has too many entries"));
            }
            let metadata = fs::symlink_metadata(entry.path())?;
            if is_link(&metadata) || (!metadata.is_file() && !metadata.is_dir()) {
                return Err(invalid(
                    "Ownership snapshot rejects links and special files",
                ));
            }
            let relative = entry
                .path()
                .strip_prefix(root)
                .map_err(|_| invalid("Ownership path escaped its root"))?;
            let parts: Vec<_> = relative
                .components()
                .map(|p| {
                    p.as_os_str()
                        .to_str()
                        .ok_or_else(|| invalid("Ownership path is not UTF-8"))
                })
                .collect::<io::Result<_>>()?;
            for part in &parts {
                component(part)?;
            }
            let spelling = parts.join("/");
            if spelling.len() > 1024 {
                return Err(invalid("Ownership path is too long"));
            }
            let value = if metadata.is_dir() {
                "directory".into()
            } else {
                let mut hasher = Sha256::new();
                let mut file = File::open(entry.path())?;
                let mut buffer = [0u8; 16 * 1024];
                let mut size = 0u64;
                loop {
                    let n = file.read(&mut buffer)?;
                    if n == 0 {
                        break;
                    }
                    size += n as u64;
                    total += n as u64;
                    if size > 128 * 1024 * 1024 || total > 512 * 1024 * 1024 {
                        return Err(invalid("Ownership snapshot exceeds byte limits"));
                    }
                    hasher.update(&buffer[..n]);
                }
                format!("sha256:{:x}", hasher.finalize())
            };
            files.insert(spelling, value);
        }
    }
    Ok(files)
}
fn decode(bytes: &[u8], root: &Path, id: &str) -> io::Result<Receipt> {
    identifier(id)?;
    if format!("{:x}", Sha256::digest(bytes)) != id {
        return Err(invalid("Ownership receipt checksum mismatch"));
    }
    let record: Receipt = serde_json::from_slice(bytes).map_err(io::Error::other)?;
    if record.format != 1 || record.root != root_path(root)? {
        return Err(invalid(
            "Ownership receipt version or installation mismatch",
        ));
    }
    folders(&record.folders)?;
    Ok(record)
}
fn load(root: &Path, id: &str) -> io::Result<Receipt> {
    identifier(id)?;
    plain(&root.join(STORE), true)?;
    decode(
        &read(&root.join(STORE).join(format!("{id}.json")))?,
        root,
        id,
    )
}
fn same_scope(record: &Receipt, source: &serde_json::Value) -> io::Result<()> {
    for key in ["repository", "product"] {
        if source[key].is_null() || record.source[key] != source[key] {
            return Err(invalid("Ownership source/product mismatch"));
        }
    }
    Ok(())
}
fn verify(record: &Receipt, root: &Path) -> io::Result<()> {
    if snapshot(root, &record.folders)? != record.files {
        return Err(invalid(
            "Installed folders changed since this ownership receipt",
        ));
    }
    Ok(())
}
fn preview(record: Receipt, id: &str) -> OwnershipPreview {
    OwnershipPreview {
        id: id.into(),
        root: record.root,
        source: record.source,
        folders: record.folders,
    }
}

/// Inspect a content-matching receipt. This does not authorize replacement or deletion.
pub fn inspect_ownership(root: &Path, id: &str) -> io::Result<OwnershipPreview> {
    let root = root_path(root)?;
    let record = load(&root, id)?;
    verify(&record, &root)?;
    Ok(preview(record, id))
}
pub(crate) fn hints(
    root: &Path,
    id: &str,
    source: &serde_json::Value,
) -> io::Result<Vec<AddonFolder>> {
    let root = root_path(root)?;
    let record = load(&root, id)?;
    same_scope(&record, source)?;
    Ok(record
        .folders
        .iter()
        .map(|name| AddonFolder {
            id: name.clone(),
            path: root.join(name),
            ..Default::default()
        })
        .collect())
}
// Called only from the staged-validation callback while the installer holds its OS lock.
pub(crate) fn authorize(
    root: &Path,
    id: &str,
    source: &serde_json::Value,
    confirm: impl FnOnce(&OwnershipPreview) -> bool,
) -> io::Result<()> {
    let record = load(root, id)?;
    same_scope(&record, source)?;
    verify(&record, root)?;
    if !confirm(&preview(record, id)) {
        return Err(invalid("Ownership update cancelled"));
    }
    let record = load(root, id)?;
    same_scope(&record, source)?;
    verify(&record, root)
}

// Prepare journal data before the ready marker; neither a receipt nor a hash grants authority.
pub(crate) fn prepare(staged: &Path, root: &Path, source: serde_json::Value) -> io::Result<String> {
    let mut names: Vec<_> = fs::read_dir(staged)?
        .map(|e| {
            e?.file_name()
                .into_string()
                .map_err(|_| invalid("Invalid staged folder name"))
        })
        .collect::<io::Result<_>>()?;
    names.sort();
    let record = Receipt {
        format: 1,
        root: root_path(root)?,
        source,
        files: snapshot(staged, &names)?,
        folders: names,
    };
    let bytes = serde_json::to_vec(&record).map_err(io::Error::other)?;
    if bytes.len() as u64 > LIMIT {
        return Err(invalid("Ownership receipt exceeds metadata limit"));
    }
    let id = format!("{:x}", Sha256::digest(&bytes));
    let transaction = staged
        .parent()
        .ok_or_else(|| invalid("Missing staging parent"))?;
    let mut output = File::create_new(transaction.join("receipt.json"))?;
    output.write_all(&bytes)?;
    output.sync_all()?;
    let mut output = File::create_new(transaction.join("receipt.id"))?;
    output.write_all(id.as_bytes())?;
    output.sync_all()?;
    Ok(id)
}
// Run after commit and before backup cleanup. Restart retries are idempotent and never overwrite.
pub(super) fn publish(root: &Path, transaction: &Path, incoming: &[String]) -> io::Result<()> {
    let id_path = transaction.join("receipt.id");
    let payload = transaction.join("receipt.json");
    match fs::symlink_metadata(&id_path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            if fs::symlink_metadata(&payload).is_ok() {
                return Err(invalid(
                    "Committed ownership journal lacks its receipt identifier",
                ));
            }
            return Ok(());
        }
        Err(e) => return Err(e),
        Ok(_) => (),
    }
    let id = String::from_utf8(read(&id_path)?)
        .map_err(|_| invalid("Ownership identifier is not UTF-8"))?;
    identifier(&id)?;
    let store = root.join(STORE);
    let destination = store.join(format!("{id}.json"));
    let bytes = match fs::symlink_metadata(&payload) {
        Ok(_) => read(&payload)?,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            plain(&store, true)?;
            read(&destination)?
        }
        Err(e) => return Err(e),
    };
    let record = decode(&bytes, root, &id)?;
    if record.folders != incoming {
        return Err(invalid("Receipt folders disagree with install journal"));
    }
    verify(&record, root)?;
    match fs::create_dir(&store) {
        Ok(()) => (),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => (),
        Err(e) => return Err(e),
    }
    plain(&store, true)?;
    match fs::symlink_metadata(&destination) {
        Ok(_) => {
            if read(&destination)? != bytes {
                return Err(invalid("Ownership publication destination conflict"));
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            let mut file = tempfile::Builder::new()
                .prefix(".receipt-")
                .tempfile_in(&store)?;
            file.write_all(&bytes)?;
            file.as_file().sync_all()?;
            file.persist_noclobber(destination).map_err(|e| e.error)?;
        }
        Err(e) => return Err(e),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf, String) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("AddOns");
        let transaction = root.join(".patchloom-install");
        fs::create_dir_all(transaction.join("staged/Foo")).unwrap();
        fs::write(transaction.join("staged/Foo/a.lua"), b"new").unwrap();
        let id = prepare(
            &transaction.join("staged"),
            &root,
            serde_json::json!({"repository":"fixture/addon","product":"Retail"}),
        )
        .unwrap();
        fs::rename(transaction.join("staged/Foo"), root.join("Foo")).unwrap();
        (temp, root, transaction, id)
    }
    #[test]
    fn publication_is_append_only_and_restart_can_use_published_payload() {
        let (_temp, root, transaction, id) = fixture();
        publish(&root, &transaction, &["Foo".into()]).unwrap();
        let path = root.join(STORE).join(format!("{id}.json"));
        let bytes = fs::read(&path).unwrap();
        fs::remove_file(transaction.join("receipt.json")).unwrap();
        publish(&root, &transaction, &["Foo".into()]).unwrap();
        assert_eq!(fs::read(&path).unwrap(), bytes);
        assert_eq!(inspect_ownership(&root, &id).unwrap().folders, ["Foo"]);
        fs::write(&path, b"tampered").unwrap();
        assert!(publish(&root, &transaction, &["Foo".into()]).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"tampered");
    }
    #[test]
    fn malformed_foreign_stale_and_unsupported_receipts_never_authorize() {
        let (temp, root, transaction, id) = fixture();
        publish(&root, &transaction, &["Foo".into()]).unwrap();
        assert!(hints(
            &root,
            &id,
            &serde_json::json!({"repository":"other/addon","product":"Retail"})
        )
        .is_err());
        assert!(inspect_ownership(&root, "../escape").is_err());
        let bytes = fs::read(root.join(STORE).join(format!("{id}.json"))).unwrap();
        let other = temp.path().join("other");
        fs::create_dir_all(other.join(STORE)).unwrap();
        fs::write(other.join(STORE).join(format!("{id}.json")), &bytes).unwrap();
        assert!(inspect_ownership(&other, &id).is_err());
        let mut unsupported: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        unsupported["format"] = 2.into();
        let bytes = serde_json::to_vec(&unsupported).unwrap();
        let forged = format!("{:x}", Sha256::digest(&bytes));
        fs::write(root.join(STORE).join(format!("{forged}.json")), bytes).unwrap();
        assert!(inspect_ownership(&root, &forged).is_err());
        fs::write(root.join("Foo/a.lua"), b"changed").unwrap();
        assert!(authorize(
            &root,
            &id,
            &serde_json::json!({"repository":"fixture/addon","product":"Retail"}),
            |_| panic!("stale data must not reach confirmation")
        )
        .is_err());
        assert_eq!(fs::read(root.join("Foo/a.lua")).unwrap(), b"changed");
    }
}
