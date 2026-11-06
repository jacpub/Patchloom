//! Strict public GitHub release pilot, modified 2026-10-27; original GPLv3 applies.
use crate::{
    addon::{Addon, AddonFolder},
    discovery::{self, Installation, Product},
    network,
};
use serde::{Deserialize, Serialize};
use std::{io, path::Path};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[derive(Deserialize)]
struct Release {
    id: u64,
    tag_name: String,
    draft: bool,
    prerelease: bool,
    assets: Vec<Asset>,
}
#[derive(Deserialize)]
struct Asset {
    id: u64,
    name: String,
    state: String,
    size: u64,
    digest: Option<String>,
    browser_download_url: String,
}
#[derive(Deserialize)]
struct Manifest {
    releases: Vec<Package>,
}
#[derive(Deserialize)]
struct Package {
    filename: String,
    nolib: bool,
    metadata: Vec<Metadata>,
}
#[derive(Deserialize)]
struct Metadata {
    flavor: String,
    interface: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct Candidate {
    repository: String,
    release_id: u64,
    asset_id: u64,
    pub version: String,
    product: Product,
    interface: u32,
    url: String,
    size: u64,
    digest: String,
}
pub struct Installed {
    candidate: Candidate,
    pub folders: Vec<AddonFolder>,
    pub receipt_id: String,
}
impl Installed {
    pub fn matches_release(&self, candidate: &Candidate) -> bool {
        self.candidate.repository == candidate.repository
            && self.candidate.product == candidate.product
            && self.candidate.interface == candidate.interface
            && self.candidate.release_id == candidate.release_id
            && self.candidate.asset_id == candidate.asset_id
            && self.candidate.digest == candidate.digest
            && self.candidate.version == candidate.version
    }
}

/// Exact folder effects of one validated artifact, not authority to adopt addons.
pub struct InstallPreview {
    added: Vec<String>,
    replaced: Vec<String>,
    removed: Vec<String>,
}
impl InstallPreview {
    pub fn added(&self) -> &[String] {
        &self.added
    }
    pub fn replaced(&self) -> &[String] {
        &self.replaced
    }
    pub fn removed(&self) -> &[String] {
        &self.removed
    }
}

/// Owns temporary verified bytes and an immutable content-bound preview.
/// Dropping this object cancels without touching installed folders.
pub struct PreparedInstall {
    candidate: Candidate,
    installation: Installation,
    temporary: tempfile::TempDir,
    archive: crate::fs::PreparedArchive,
    addon: Addon,
    ownership: Option<crate::fs::OwnershipPreview>,
    preview: InstallPreview,
}

/// Single-use approval of exactly one prepared artifact and folder snapshot.
/// Approval and application both consume their input; neither type is Clone.
/// ```compile_fail
/// use ajour_core::github_releases::PreparedInstall;
/// fn duplicate(prepared: PreparedInstall) {
///     let first = prepared.approve();
///     let second = prepared.approve();
/// }
/// ```
/// ```compile_fail
/// use ajour_core::github_releases::InstallApproval;
/// async fn replay(approval: InstallApproval) {
///     let first = approval.apply().await;
///     let second = approval.apply().await;
/// }
/// ```
pub struct InstallApproval(PreparedInstall);

impl PreparedInstall {
    pub fn preview(&self) -> &InstallPreview {
        &self.preview
    }
    pub fn candidate(&self) -> &Candidate {
        &self.candidate
    }
    /// Call only after the user explicitly confirms this exact preview.
    pub fn approve(self) -> InstallApproval {
        InstallApproval(self)
    }

    fn validate(&self, root: &Path, incoming: &[String]) -> crate::fs::Result<()> {
        crate::fs::require_clean_install(root)?;
        self.validate_contents(root, incoming)
    }

    fn validate_contents(&self, root: &Path, incoming: &[String]) -> crate::fs::Result<()> {
        validate_installation(&self.candidate, &self.installation)
            .map_err(|e| invalid(&e.to_string()))?;
        network::plain_download_directory(root).map_err(|e| invalid(&e.to_string()))?;
        network::plain_download_directory(self.temporary.path())
            .map_err(|e| invalid(&e.to_string()))?;
        self.archive
            .validate_archive(&self.temporary.path().join("archive.zip"))?;
        self.archive
            .validate_staged(&self.temporary.path().join("preview"))?;
        self.archive
            .validate_destinations(&self.addon, root, incoming)?;
        if let Some(expected) = &self.ownership {
            let current = crate::fs::inspect_ownership(root, &expected.id)?;
            if current.root != expected.root
                || current.source != expected.source
                || current.folders != expected.folders
            {
                return Err(invalid("Prepared ownership changed; review again").into());
            }
        }
        Ok(())
    }

    // Called only while the actual installer holds its writer lock.
    fn authorize_ownership(&self, root: &Path) -> crate::fs::Result<()> {
        if let Some(expected) = &self.ownership {
            let source = serde_json::to_value(&self.candidate).map_err(io::Error::other)?;
            crate::fs::ownership::authorize(root, &expected.id, &source, |current| {
                current.id == expected.id
                    && current.root == expected.root
                    && current.source == expected.source
                    && current.folders == expected.folders
            })?;
        }
        Ok(())
    }
}

impl InstallApproval {
    /// Revalidate under the existing installer mutex, then use its transaction.
    /// Errors after commit can report pending ownership publication/cleanup;
    /// inspect retained state before retrying rather than assuming rollback.
    pub async fn apply(self) -> Result<Installed> {
        let prepared = self.0;
        let root = prepared.installation.directory.join("Interface/AddOns");
        // Check plain paths before the shared installer opens the cached ZIP;
        // repeat all authoritative checks under its actual writer lock below.
        network::plain_download_directory(prepared.temporary.path())?;
        prepared
            .archive
            .validate_archive(&prepared.temporary.path().join("archive.zip"))?;
        let source = serde_json::to_value(&prepared.candidate)?;
        let mut receipt_id = None;
        let folders = crate::fs::install_addon_guarded(
            &prepared.addon,
            prepared.temporary.path(),
            &root,
            |root, incoming| {
                prepared.validate(root, incoming)?;
                prepared.authorize_ownership(root)
            },
            |staged, _| {
                prepared.validate_contents(&root, prepared.archive.incoming())?;
                prepared.authorize_ownership(&root)?;
                prepared.archive.validate_staged(staged)?;
                let folders = discovery::parse_product_tocs(
                    staged,
                    prepared.candidate.product,
                    prepared.candidate.interface,
                )?;
                receipt_id = Some(crate::fs::ownership::prepare(staged, &root, source)?);
                Ok(folders)
            },
        )
        .await?;
        Ok(Installed {
            candidate: prepared.candidate,
            folders,
            receipt_id: receipt_id.ok_or_else(|| invalid("Ownership receipt was not prepared"))?,
        })
    }
}

fn validate_installation(
    candidate: &Candidate,
    expected: &Installation,
) -> io::Result<Installation> {
    network::plain_download_directory(&expected.directory).map_err(|e| invalid(&e.to_string()))?;
    if expected.product != candidate.product || expected.interface != candidate.interface {
        return Err(invalid("Candidate/product mismatch"));
    }
    let root = expected
        .directory
        .parent()
        .ok_or_else(|| invalid("Missing installation root"))?;
    let fresh = discovery::discover(root)?;
    if !fresh.issues.is_empty() {
        return Err(invalid("Installation discovery is ambiguous"));
    }
    fresh
        .installations
        .into_iter()
        .find(|i| {
            i.product == expected.product
                && i.product_id == expected.product_id
                && i.version == expected.version
                && i.interface == expected.interface
                && i.directory == expected.directory
        })
        .ok_or_else(|| invalid("Installation identity changed; rediscover before updating"))
}

fn repository_name(repository: &str) -> Result<()> {
    let parts: Vec<_> = repository.split('/').collect();
    if parts.len() != 2
        || parts.iter().any(|v| {
            v.is_empty()
                || v.len() > 100
                || *v == "."
                || *v == ".."
                || !v
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))
        })
    {
        return Err(invalid("Expected a public GitHub owner/repository name").into());
    }
    Ok(())
}
fn asset<'a>(release: &'a Release, name: &str, repository: &str) -> Result<&'a Asset> {
    let matches: Vec<_> = release.assets.iter().filter(|a| a.name == name).collect();
    if matches.len() != 1 {
        return Err(invalid("Missing or ambiguous release asset").into());
    }
    let asset = matches[0];
    let prefix = format!("https://github.com/{repository}/releases/download/");
    if asset.id == 0
        || asset.state != "uploaded"
        || asset.size == 0
        || asset.size > 128 * 1024 * 1024
        || !asset.browser_download_url.starts_with(&prefix)
        || asset.browser_download_url.contains(['#', '?'])
    {
        return Err(invalid("Invalid GitHub release asset metadata").into());
    }
    Ok(asset)
}
fn flavor(product: Product) -> &'static str {
    match product {
        Product::Retail => "mainline",
        Product::ClassicProgression => "mists",
        Product::ClassicEra => "classic",
        Product::Anniversary => "bcc",
    }
}
fn resolve(
    repository: &str,
    installation: &Installation,
    release: &Release,
    manifest: &[u8],
) -> Result<Candidate> {
    if release.draft || release.prerelease || release.id == 0 || release.tag_name.is_empty() {
        return Err(invalid("No stable published release").into());
    }
    let manifest: Manifest = serde_json::from_slice(manifest)?;
    let matches: Vec<_> = manifest
        .releases
        .iter()
        .filter(|p| {
            !p.nolib
                && p.metadata.iter().any(|m| {
                    m.flavor == flavor(installation.product)
                        && m.interface == installation.interface
                })
        })
        .collect();
    if matches.len() != 1 {
        return Err(
            invalid("No unique product/interface manifest match; no filename fallback").into(),
        );
    }
    let asset = asset(release, &matches[0].filename, repository)?;
    if !asset.name.ends_with(".zip") {
        return Err(invalid("Expected a packaged addon ZIP").into());
    }
    let digest = asset
        .digest
        .clone()
        .ok_or_else(|| invalid("GitHub asset digest required for this pilot"))?;
    if !digest
        .strip_prefix("sha256:")
        .is_some_and(|d| d.len() == 64 && d.bytes().all(|c| c.is_ascii_hexdigit()))
    {
        return Err(invalid("Invalid asset SHA-256").into());
    }
    Ok(Candidate {
        repository: repository.into(),
        release_id: release.id,
        asset_id: asset.id,
        version: release.tag_name.clone(),
        product: installation.product,
        interface: installation.interface,
        url: asset.browser_download_url.clone(),
        size: asset.size,
        digest,
    })
}

/// Check one explicitly selected public repository; failures do not change local inventory.
pub async fn check(repository: &str, installation: &Installation) -> Result<Candidate> {
    repository_name(repository)?;
    let bytes = network::metadata(&format!(
        "https://api.github.com/repos/{repository}/releases/latest"
    ))
    .await?;
    let release: Release = serde_json::from_slice(&bytes)?;
    let manifest_asset = asset(&release, "release.json", repository)?;
    if manifest_asset.size > 1024 * 1024 {
        return Err(invalid("Release manifest exceeds limit").into());
    }
    let manifest = network::metadata(&manifest_asset.browser_download_url).await?;
    if manifest.len() as u64 != manifest_asset.size {
        return Err(invalid("Manifest size mismatch").into());
    }
    // Manifest bytes are bound to the API-provided digest as well as their origin.
    use sha2::{Digest, Sha256};
    if manifest_asset.digest.as_deref() != Some(&format!("sha256:{:x}", Sha256::digest(&manifest)))
    {
        return Err(invalid("Manifest SHA-256 mismatch or absent").into());
    }
    resolve(repository, installation, &release, &manifest)
}

impl Candidate {
    /// Prepare a nonconflicting fresh install (`None`) or content-matching
    /// receipt-backed update. This creates only owned temporary cache files.
    /// It neither adopts unmanaged folders nor recovers an older transaction.
    pub async fn prepare(
        &self,
        installation: &Installation,
        receipt_id: Option<&str>,
        cache: &Path,
    ) -> Result<PreparedInstall> {
        let installation = validate_installation(self, installation)?;
        let root = installation.directory.join("Interface/AddOns");
        network::plain_download_directory(&root)?;
        network::plain_download_directory(cache)?;
        if cache.canonicalize()?.starts_with(
            installation
                .directory
                .parent()
                .ok_or_else(|| invalid("Missing installation root"))?,
        ) {
            return Err(invalid("Preparation cache must be outside the WoW installation").into());
        }
        crate::fs::require_clean_install(&root)?;
        let source = serde_json::to_value(self)?;
        let mut addon = Addon::empty("archive.zip");
        let ownership = if let Some(id) = receipt_id {
            addon.folders = crate::fs::ownership::hints(&root, id, &source)?;
            Some(crate::fs::inspect_ownership(&root, id)?)
        } else {
            None
        };
        let temporary = tempfile::Builder::new()
            .prefix(".patchloom-provider-")
            .tempdir_in(cache)?;
        network::download_verified(
            &self.url,
            &temporary.path().join("archive.zip"),
            Some(self.size),
            Some(&self.digest),
        )
        .await?;
        let archive = crate::fs::prepare_archive(&addon, temporary.path(), &root)?;
        archive.validate_digest(&self.digest)?;
        discovery::parse_product_tocs(
            &temporary.path().join("preview"),
            self.product,
            self.interface,
        )?;
        let preview = InstallPreview {
            added: archive
                .incoming()
                .iter()
                .filter(|n| !archive.previous().contains(n))
                .cloned()
                .collect(),
            replaced: archive
                .incoming()
                .iter()
                .filter(|n| archive.previous().contains(n))
                .cloned()
                .collect(),
            removed: archive
                .previous()
                .iter()
                .filter(|n| !archive.incoming().contains(n))
                .cloned()
                .collect(),
        };
        let prepared = PreparedInstall {
            candidate: self.clone(),
            installation,
            temporary,
            archive,
            addon,
            ownership,
            preview,
        };
        // Validate again after download/staging, without creating a live mutex.
        prepared.validate(&root, prepared.archive.incoming())?;
        Ok(prepared)
    }

    /// Caller explicitly supplies previously owned folders; discovery alone never proves ownership.
    /// The returned in-memory receipt is produced only after the installer commits.
    pub async fn install(
        &self,
        installation: &Installation,
        previous: &[AddonFolder],
        cache: &Path,
    ) -> Result<Installed> {
        self.install_validated(installation, previous, cache, |_| Ok(()))
            .await
    }

    /// Receipt-based replacement always needs explicit confirmation under the installer lock.
    pub async fn install_from_receipt(
        &self,
        installation: &Installation,
        receipt_id: &str,
        cache: &Path,
        confirm: impl FnOnce(&crate::fs::OwnershipPreview) -> bool,
    ) -> Result<Installed> {
        let source = serde_json::to_value(self)?;
        let previous = crate::fs::ownership::hints(
            &installation.directory.join("Interface/AddOns"),
            receipt_id,
            &source,
        )?;
        self.install_validated(installation, &previous, cache, |root| {
            crate::fs::ownership::authorize(root, receipt_id, &source, confirm)
        })
        .await
    }

    async fn install_validated(
        &self,
        installation: &Installation,
        previous: &[AddonFolder],
        cache: &Path,
        validate_previous: impl FnOnce(&Path) -> io::Result<()>,
    ) -> Result<Installed> {
        if self.product != installation.product || self.interface != installation.interface {
            return Err(invalid("Candidate/product mismatch").into());
        }
        let root = installation
            .directory
            .parent()
            .ok_or_else(|| invalid("Missing copied install root"))?;
        let fresh = discovery::discover(root)?;
        if !fresh.issues.is_empty()
            || !fresh.installations.iter().any(|i| {
                i.product == self.product
                    && i.product_id == installation.product_id
                    && i.version == installation.version
                    && i.interface == self.interface
                    && i.directory == installation.directory
            })
        {
            return Err(
                invalid("Installation identity changed; rediscover before updating").into(),
            );
        }
        let addons = installation.directory.join("Interface/AddOns");
        network::plain_download_directory(cache)?;
        let temporary = tempfile::Builder::new()
            .prefix(".patchloom-provider-")
            .tempdir_in(cache)?;
        let mut addon = Addon::empty("archive.zip");
        addon.folders = previous.to_vec();
        network::download_verified(
            &self.url,
            &temporary.path().join("archive.zip"),
            Some(self.size),
            Some(&self.digest),
        )
        .await?;
        let after_download = discovery::discover(root)?;
        if !after_download.issues.is_empty()
            || !after_download.installations.iter().any(|i| {
                i.product == self.product
                    && i.product_id == installation.product_id
                    && i.version == installation.version
                    && i.interface == self.interface
                    && i.directory == installation.directory
            })
        {
            return Err(invalid("Installation changed during download; update cancelled").into());
        }
        let mut receipt_id = None;
        let source = serde_json::to_value(self)?;
        let folders =
            crate::fs::install_addon_checked(&addon, temporary.path(), &addons, |staged, _| {
                validate_previous(&addons)?;
                let folders = discovery::parse_product_tocs(staged, self.product, self.interface)?;
                receipt_id = Some(crate::fs::ownership::prepare(staged, &addons, source)?);
                Ok(folders)
            })
            .await?;
        Ok(Installed {
            candidate: self.clone(),
            folders,
            receipt_id: receipt_id.ok_or_else(|| invalid("Ownership receipt was not prepared"))?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    use std::{
        fs,
        io::{Read, Write},
        net::TcpListener,
    };
    fn zip(entries: &[(&str, &str)]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(io::Cursor::new(vec![]));
        for (name, content) in entries {
            writer
                .start_file(*name, zip::write::FileOptions::default())
                .unwrap();
            writer.write_all(content.as_bytes()).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }
    fn server(bytes: Vec<u8>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; 4096];
            let _ = stream.read(&mut request);
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n",
                bytes.len()
            )
            .unwrap();
            stream.write_all(&bytes).unwrap();
        });
        url
    }
    fn fixture() -> (tempfile::TempDir, Installation, Vec<AddonFolder>) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("wow");
        let addon = root.join("_retail_/Interface/AddOns/Foo");
        fs::create_dir_all(&addon).unwrap();
        fs::write(
            root.join(".build.info"),
            "Active!DEC:1|Product!STRING:0|Version!STRING:0\n1|wow|12.1.5.70575\n",
        )
        .unwrap();
        fs::write(
            addon.join("Foo.toc"),
            "## Interface: 120105\n## Version: old\n",
        )
        .unwrap();
        fs::write(addon.join("old.lua"), "preserve").unwrap();
        let install = discovery::discover(&root).unwrap().installations.remove(0);
        let previous = vec![AddonFolder {
            id: "Foo".into(),
            path: addon,
            ..Default::default()
        }];
        (temp, install, previous)
    }
    fn candidate(installation: &Installation, bytes: &[u8]) -> Candidate {
        let release: Release=serde_json::from_value(serde_json::json!({"id":1,"tag_name":"v2","draft":false,"prerelease":false,"assets":[{"id":2,"name":"Foo.zip","state":"uploaded","size":bytes.len(),"digest":format!("sha256:{:x}",Sha256::digest(bytes)),"browser_download_url":"https://github.com/fixture/addon/releases/download/v2/Foo.zip"}]})).unwrap();
        resolve("fixture/addon",installation,&release,br#"{"releases":[{"filename":"Foo.zip","nolib":false,"metadata":[{"flavor":"mainline","interface":120105}]}]}"#).unwrap()
    }
    #[test]
    fn prepared_fresh_preview_is_read_only_and_apply_requires_consumed_approval() {
        async_std::task::block_on(async {
            let (temp, installation, previous) = fixture();
            let root = installation.directory.join("Interface/AddOns");
            let bytes = zip(&[
                ("Bar/Bar.toc", "## Interface: 120105\n## Version: v2\n"),
                ("Bar/new.lua", "new"),
            ]);
            let mut selected = candidate(&installation, &bytes);
            selected.url = server(bytes.clone());
            let prepared = selected
                .prepare(&installation, None, temp.path())
                .await
                .unwrap();
            assert_eq!(prepared.preview().added(), ["Bar"]);
            assert!(prepared.preview().replaced().is_empty());
            assert!(prepared.preview().removed().is_empty());
            assert!(!root.join("Bar").exists());
            assert!(!root.join(".patchloom-install.lock").exists());
            assert!(!root.join(".patchloom-install").exists());
            drop(prepared);
            assert!(!root.join("Bar").exists());
            assert_eq!(
                fs::read(previous[0].path.join("old.lua")).unwrap(),
                b"preserve"
            );
            selected.url = server(bytes);
            let installed = selected
                .prepare(&installation, None, temp.path())
                .await
                .unwrap()
                .approve()
                .apply()
                .await
                .unwrap();
            assert!(installed.matches_release(&selected));
            assert_eq!(fs::read(root.join("Bar/new.lua")).unwrap(), b"new");
            let receipt = crate::fs::inspect_ownership(&root, &installed.receipt_id).unwrap();
            assert_eq!(receipt.folders, ["Bar"]);
            assert_eq!(
                fs::read(previous[0].path.join("old.lua")).unwrap(),
                b"preserve"
            );
        });
    }

    #[test]
    fn prepared_update_shows_exact_replacements_additions_and_removals() {
        async_std::task::block_on(async {
            let (temp, installation, previous) = fixture();
            let root = installation.directory.join("Interface/AddOns");
            let bytes = zip(&[
                ("Foo/Foo.toc", "## Interface: 120105\n"),
                ("Gone/Gone.toc", "## Interface: 120105\n"),
            ]);
            let mut first = candidate(&installation, &bytes);
            first.url = server(bytes);
            let old = first
                .install(&installation, &previous, temp.path())
                .await
                .unwrap();
            let old_receipt = fs::read(
                root.join(".patchloom-ownership")
                    .join(format!("{}.json", old.receipt_id)),
            )
            .unwrap();
            let bytes = zip(&[
                ("Foo/Foo.toc", "## Interface: 120105\n## Version: v3\n"),
                ("Added/Added.toc", "## Interface: 120105\n"),
            ]);
            let mut next = candidate(&installation, &bytes);
            next.version = "v3".into();
            next.url = server(bytes);
            let prepared = next
                .prepare(&installation, Some(&old.receipt_id), temp.path())
                .await
                .unwrap();
            assert_eq!(prepared.preview().added(), ["Added"]);
            assert_eq!(prepared.preview().replaced(), ["Foo"]);
            assert_eq!(prepared.preview().removed(), ["Gone"]);
            assert!(root.join("Gone").is_dir());
            assert!(!root.join("Added").exists());
            let installed = prepared.approve().apply().await.unwrap();
            assert!(root.join("Added").is_dir());
            assert!(!root.join("Gone").exists());
            assert_eq!(
                installed
                    .folders
                    .iter()
                    .find(|f| f.id == "Foo")
                    .unwrap()
                    .version
                    .as_deref(),
                Some("v3")
            );
            assert_eq!(
                fs::read(
                    root.join(".patchloom-ownership")
                        .join(format!("{}.json", old.receipt_id))
                )
                .unwrap(),
                old_receipt
            );
            assert_eq!(
                crate::fs::inspect_ownership(&root, &installed.receipt_id)
                    .unwrap()
                    .folders,
                ["Added", "Foo"]
            );
        });
    }

    // Byte/tree preservation oracle excludes only the cooperative mutex created
    // by an explicit apply; payload files with that name remain included.
    fn prepared_tree(root: &Path) -> std::collections::BTreeMap<String, String> {
        walkdir::WalkDir::new(root)
            .follow_links(false)
            .into_iter()
            .map(|e| e.unwrap())
            .filter(|e| e.path() != root && e.path() != root.join(".patchloom-install.lock"))
            .map(|e| {
                let name = e
                    .path()
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                let m = fs::symlink_metadata(e.path()).unwrap();
                let value = if m.file_type().is_symlink() {
                    "link".into()
                } else if m.is_dir() {
                    "directory".into()
                } else {
                    format!("{:x}", Sha256::digest(fs::read(e.path()).unwrap()))
                };
                (name, value)
            })
            .collect()
    }

    #[test]
    fn prepared_fresh_install_rejects_unmanaged_owned_looking_and_case_collisions() {
        async_std::task::block_on(async {
            for name in ["Foo", "foo"] {
                let (temp, installation, _) = fixture();
                let root = installation.directory.join("Interface/AddOns");
                let bytes = zip(&[(
                    if name == "Foo" {
                        "Foo/Foo.toc"
                    } else {
                        "foo/foo.toc"
                    },
                    "## Interface: 120105\n",
                )]);
                let mut selected = candidate(&installation, &bytes);
                selected.url = server(bytes);
                let before = prepared_tree(&root);
                assert!(selected
                    .prepare(&installation, None, temp.path())
                    .await
                    .is_err());
                assert_eq!(prepared_tree(&root), before);
                assert!(!root.join(".patchloom-install.lock").exists());
            }
        });
    }

    #[test]
    fn prepared_rejects_unsafe_or_incompatible_archives_without_live_state() {
        async_std::task::block_on(async {
            for entries in [
                vec![("../escape", "bad")],
                vec![("Bar/Bar.toc", "## Interface: 11509\n")],
                vec![("Bar/no-toc.lua", "bad")],
            ] {
                let (temp, installation, _) = fixture();
                let root = installation.directory.join("Interface/AddOns");
                let before = prepared_tree(&root);
                let bytes = zip(&entries);
                let mut selected = candidate(&installation, &bytes);
                selected.url = server(bytes);
                assert!(selected
                    .prepare(&installation, None, temp.path())
                    .await
                    .is_err());
                assert_eq!(prepared_tree(&root), before);
                assert!(!root.join(".patchloom-install.lock").exists());
                assert!(!temp.path().join("escape").exists());
            }
        });
    }

    #[test]
    fn prepared_mutations_invalidate_approval_before_replacement() {
        async_std::task::block_on(async {
            for mutation in 0..7 {
                let (temp, installation, previous) = fixture();
                let root = installation.directory.join("Interface/AddOns");
                let bytes = zip(&[
                    ("Foo/Foo.toc", "## Interface: 120105\n"),
                    ("Foo/value.lua", "old"),
                ]);
                let mut first = candidate(&installation, &bytes);
                first.url = server(bytes);
                let old = first
                    .install(&installation, &previous, temp.path())
                    .await
                    .unwrap();
                let bytes = zip(&[
                    ("Foo/Foo.toc", "## Interface: 120105\n"),
                    ("Foo/value.lua", "new"),
                    ("Bar/Bar.toc", "## Interface: 120105\n"),
                ]);
                let mut next = candidate(&installation, &bytes);
                next.url = server(bytes);
                let prepared = next
                    .prepare(&installation, Some(&old.receipt_id), temp.path())
                    .await
                    .unwrap();
                let receipt = root
                    .join(".patchloom-ownership")
                    .join(format!("{}.json", old.receipt_id));
                match mutation {
                    0 => fs::OpenOptions::new()
                        .append(true)
                        .open(prepared.temporary.path().join("archive.zip"))
                        .unwrap()
                        .write_all(b"changed")
                        .unwrap(),
                    1 => fs::write(
                        prepared.temporary.path().join("preview/Foo/value.lua"),
                        b"changed",
                    )
                    .unwrap(),
                    2 => fs::write(root.join("Foo/value.lua"), b"user-edited").unwrap(),
                    3 => {
                        fs::create_dir(root.join("Bar")).unwrap();
                        fs::write(root.join("Bar/keep"), b"unmanaged").unwrap();
                    }
                    4 => fs::write(
                        installation.directory.parent().unwrap().join(".build.info"),
                        "Active!DEC:1|Product!STRING:0|Version!STRING:0\n1|wow|12.2.0.70575\n",
                    )
                    .unwrap(),
                    5 => fs::OpenOptions::new()
                        .append(true)
                        .open(&receipt)
                        .unwrap()
                        .write_all(b" ")
                        .unwrap(),
                    6 => fs::rename(&receipt, temp.path().join("preserved-receipt.json")).unwrap(),
                    _ => unreachable!(),
                }
                let before = prepared_tree(&root);
                assert!(
                    prepared.approve().apply().await.is_err(),
                    "mutation {} was accepted",
                    mutation
                );
                assert_eq!(
                    prepared_tree(&root),
                    before,
                    "mutation {mutation} lost data"
                );
                assert!(!root.join(".patchloom-install").exists());
                assert_eq!(
                    fs::read(root.join("Foo/value.lua")).unwrap(),
                    if mutation == 2 {
                        b"user-edited".as_slice()
                    } else {
                        b"old".as_slice()
                    }
                );
            }
        });
    }

    #[test]
    fn prepared_pending_recovery_is_never_implicitly_recovered() {
        async_std::task::block_on(async {
            for pending in [".patchloom-install", ".patchloom-restore"] {
                for before_prepare in [false, true] {
                    let (temp, installation, _) = fixture();
                    let root = installation.directory.join("Interface/AddOns");
                    let bytes = zip(&[("Bar/Bar.toc", "## Interface: 120105\n")]);
                    let mut selected = candidate(&installation, &bytes);
                    if before_prepare {
                        fs::create_dir(root.join(pending)).unwrap();
                        let before = prepared_tree(&root);
                        selected.url = "http://127.0.0.1:1/no-request".into();
                        let error = selected
                            .prepare(&installation, None, temp.path())
                            .await
                            .err()
                            .unwrap();
                        assert!(error.to_string().contains("Pending installation"));
                        assert_eq!(prepared_tree(&root), before);
                        assert!(!root.join(".patchloom-install.lock").exists());
                    } else {
                        selected.url = server(bytes);
                        let prepared = selected
                            .prepare(&installation, None, temp.path())
                            .await
                            .unwrap();
                        fs::create_dir(root.join(pending)).unwrap();
                        let before = prepared_tree(&root);
                        assert!(prepared.approve().apply().await.is_err());
                        assert_eq!(prepared_tree(&root), before);
                        assert!(
                            root.join(pending).is_dir(),
                            "empty pending journal must not be recovered/removed"
                        );
                    }
                }
            }
        });
    }

    #[test]
    fn prepared_busy_install_lock_blocks_preparation_and_apply() {
        async_std::task::block_on(async {
            for before_prepare in [false, true] {
                let (temp, installation, _) = fixture();
                let root = installation.directory.join("Interface/AddOns");
                let bytes = zip(&[("Bar/Bar.toc", "## Interface: 120105\n")]);
                let mut selected = candidate(&installation, &bytes);
                selected.url = server(bytes);
                let prepared = if !before_prepare {
                    Some(
                        selected
                            .prepare(&installation, None, temp.path())
                            .await
                            .unwrap(),
                    )
                } else {
                    None
                };
                let lock = fs::File::create(root.join(".patchloom-install.lock")).unwrap();
                lock.try_lock().unwrap();
                let before = prepared_tree(&root);
                if let Some(prepared) = prepared {
                    assert!(prepared.approve().apply().await.is_err());
                } else {
                    assert!(selected
                        .prepare(&installation, None, temp.path())
                        .await
                        .is_err());
                }
                assert_eq!(prepared_tree(&root), before);
                assert!(!root.join(".patchloom-install").exists());
            }
        });
    }

    #[test]
    fn prepared_cache_inside_installation_is_rejected_without_download_or_writes() {
        async_std::task::block_on(async {
            let (_temp, installation, _) = fixture();
            let root = installation.directory.join("Interface/AddOns");
            let before = prepared_tree(&root);
            let bytes = zip(&[("Bar/Bar.toc", "## Interface: 120105\n")]);
            let mut selected = candidate(&installation, &bytes);
            selected.url = "http://127.0.0.1:1/no-request".into();
            let error = selected
                .prepare(&installation, None, &root)
                .await
                .err()
                .unwrap();
            assert!(error.to_string().contains("cache must be outside"));
            assert_eq!(prepared_tree(&root), before);
        });
    }

    #[test]
    fn prepared_root_replacement_invalidates_a_fresh_install_even_when_affected_names_are_absent() {
        async_std::task::block_on(async {
            let (temp, installation, _) = fixture();
            let root = installation.directory.join("Interface/AddOns");
            let replacement = temp.path().join("different-addons-root");
            fs::create_dir(&replacement).unwrap();
            let bytes = zip(&[("Bar/Bar.toc", "## Interface: 120105\n")]);
            let mut selected = candidate(&installation, &bytes);
            selected.url = server(bytes);
            let prepared = selected
                .prepare(&installation, None, temp.path())
                .await
                .unwrap();
            let preserved = temp.path().join("preserved-addons-root");
            let originals = prepared_tree(&root);
            fs::rename(&root, &preserved).unwrap();
            fs::rename(&replacement, &root).unwrap();
            let before = prepared_tree(&root);
            assert!(prepared.approve().apply().await.is_err());
            assert_eq!(prepared_tree(&root), before);
            assert_eq!(prepared_tree(&preserved), originals);
            assert!(!root.join("Bar").exists());
        });
    }

    #[test]
    fn prepared_receipt_missing_foreign_or_modified_is_not_an_ownership_grant() {
        async_std::task::block_on(async {
            for mutation in 0..3 {
                let (temp, installation, previous) = fixture();
                let root = installation.directory.join("Interface/AddOns");
                let bytes = zip(&[("Foo/Foo.toc", "## Interface: 120105\n")]);
                let mut first = candidate(&installation, &bytes);
                first.url = server(bytes.clone());
                let old = first
                    .install(&installation, &previous, temp.path())
                    .await
                    .unwrap();
                let mut next = candidate(&installation, &bytes);
                next.url = "http://127.0.0.1:1/no-request".into();
                let id = if mutation == 0 {
                    "0".repeat(64)
                } else {
                    old.receipt_id.clone()
                };
                if mutation == 1 {
                    next.repository = "foreign/addon".into();
                }
                if mutation == 2 {
                    fs::write(root.join("Foo/user.lua"), b"unrecorded edit").unwrap();
                }
                let before = prepared_tree(&root);
                let error = next
                    .prepare(&installation, Some(&id), temp.path())
                    .await
                    .err()
                    .unwrap();
                assert!(!error.to_string().contains("HTTP"));
                assert_eq!(prepared_tree(&root), before);
            }
        });
    }

    #[test]
    fn strict_manifest_requires_unique_explicit_product_and_digest() {
        let (_temp, installation, _) = fixture();
        let bytes = zip(&[("Foo/Foo.toc", "## Interface: 120105")]);
        let valid = candidate(&installation, &bytes);
        let mut release: Release=serde_json::from_value(serde_json::json!({"id":1,"tag_name":"v2","draft":false,"prerelease":false,"assets":[{"id":2,"name":"Foo.zip","state":"uploaded","size":bytes.len(),"digest":valid.digest,"browser_download_url":valid.url}]})).unwrap();
        for manifest in [
            br#"{"releases":[]}"#.as_slice(),
            br#"{"releases":[{"filename":"Foo.zip","nolib":false,"metadata":[{"flavor":"mists","interface":120105}]}]}"#,
            br#"{"releases":[{"filename":"Foo.zip","nolib":false,"metadata":[{"flavor":"mainline","interface":11509}]}]}"#,
            br#"{"releases":[{"filename":"Foo.zip","nolib":false,"metadata":[{"flavor":"mainline","interface":120105}]},{"filename":"Foo.zip","nolib":false,"metadata":[{"flavor":"mainline","interface":120105}]}]}"#,
        ] {assert!(resolve("fixture/addon",&installation,&release,manifest).is_err());}
        release.assets.push(Asset {
            id: 3,
            name: "Foo.zip".into(),
            state: "uploaded".into(),
            size: 1,
            digest: None,
            browser_download_url: release.assets[0].browser_download_url.clone(),
        });
        assert!(asset(&release, "Foo.zip", "fixture/addon").is_err());
        release.assets.pop();
        let manifest=br#"{"releases":[{"filename":"Foo.zip","nolib":false,"metadata":[{"flavor":"mainline","interface":120105}]}]}"#;
        release.assets[0].digest = None;
        assert!(resolve("fixture/addon", &installation, &release, manifest).is_err());
        release.assets[0].digest = Some(valid.digest.clone());
        release.assets[0].browser_download_url = "https://other.invalid/Foo.zip".into();
        assert!(resolve("fixture/addon", &installation, &release, manifest).is_err());
        for name in [
            "../bad",
            "owner/../repo",
            "https://github.com/o/r",
            "o/r?token=x",
        ] {
            assert!(repository_name(name).is_err());
        }
    }
    #[test]
    fn fixture_check_download_install_reinventory_and_recheck() {
        async_std::task::block_on(async {
            let (temp, installation, previous) = fixture();
            let bytes = zip(&[
                (
                    "Foo/Foo_Classic.toc",
                    "## Interface: 11509\n## Version: wrong-classic\n",
                ),
                (
                    "Foo/Foo_Mainline.toc",
                    "## Interface: 120105\n## Version: v2\n",
                ),
                ("Foo/new.lua", "new"),
            ]);
            let mut selected = candidate(&installation, &bytes);
            selected.url = server(bytes.clone());
            let receipt = selected
                .install(&installation, &previous, temp.path())
                .await
                .unwrap();
            assert_eq!(fs::read(previous[0].path.join("new.lua")).unwrap(), b"new");
            assert!(!previous[0].path.join("old.lua").exists());
            assert_eq!(receipt.folders[0].version.as_deref(), Some("v2"));
            let inventory = discovery::discover(installation.directory.parent().unwrap()).unwrap();
            assert!(inventory.installations[0].addons[0].selected.is_some());
            let rechecked = candidate(&inventory.installations[0], &bytes);
            assert!(receipt.matches_release(&rechecked));
            assert!(fs::read_dir(temp.path()).unwrap().all(|e| !e
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".patchloom-provider-")));
        });
    }
    #[test]
    fn unsafe_or_incompatible_download_preserves_originals_and_unowned_folders() {
        async_std::task::block_on(async {
            for entries in [
                vec![("../escape", "bad")],
                vec![("Foo/Foo.toc", "## Interface: 11509\n")],
                vec![("Other/Other.toc", "## Interface: 120105\n")],
            ] {
                let (temp, installation, previous) = fixture();
                let addons = installation.directory.join("Interface/AddOns");
                fs::create_dir(addons.join("Other")).unwrap();
                fs::write(addons.join("Other/keep"), "unowned").unwrap();
                let bytes = zip(&entries);
                let mut selected = candidate(&installation, &bytes);
                selected.url = server(bytes);
                assert!(selected
                    .install(&installation, &previous, temp.path())
                    .await
                    .is_err());
                assert_eq!(
                    fs::read(previous[0].path.join("old.lua")).unwrap(),
                    b"preserve"
                );
                assert_eq!(fs::read(addons.join("Other/keep")).unwrap(), b"unowned");
                assert!(!temp.path().join("escape").exists());
            }
        });
    }
    #[test]
    fn durable_receipt_reloads_for_confirmed_update_under_the_install_lock() {
        async_std::task::block_on(async {
            let (temp, installation, previous) = fixture();
            let bytes = zip(&[
                ("Foo/Foo.toc", "## Interface: 120105\n## Version: v2\n"),
                ("Foo/new.lua", "v2"),
            ]);
            let mut first = candidate(&installation, &bytes);
            first.url = server(bytes);
            let installed = first
                .install(&installation, &previous, temp.path())
                .await
                .unwrap();
            let id = installed.receipt_id.clone();
            drop(installed);
            let root = installation.directory.join("Interface/AddOns");
            // Modified 2026-10-28: real writer/source/reader compatibility;
            // original GPLv3 applies.
            let source = serde_json::to_value(&first).unwrap();
            assert_eq!(source["repository"], "fixture/addon");
            assert_eq!(source["product"], "Retail");
            assert_eq!(
                source["product"],
                serde_json::to_value(installation.product).unwrap()
            );
            assert_eq!(installation.product, Product::Retail);

            let receipt_path = root.join(".patchloom-ownership").join(format!("{id}.json"));
            let receipt_bytes = fs::read(&receipt_path).unwrap();
            let record: serde_json::Value = serde_json::from_slice(&receipt_bytes).unwrap();
            assert_eq!(record["format"], 1);
            assert_eq!(record["source"], source);
            assert_eq!(record["source"]["repository"], "fixture/addon");
            assert_eq!(record["source"]["product"], "Retail");

            let preview = crate::fs::inspect_ownership(&root, &id).unwrap();
            assert_eq!(preview.id, id);
            assert_eq!(preview.root, root.canonicalize().unwrap());
            assert_eq!(preview.source, source);
            assert_eq!(preview.folders, ["Foo"]);
            assert_eq!(fs::read(&receipt_path).unwrap(), receipt_bytes);
            let bytes = zip(&[
                ("Foo/Foo.toc", "## Interface: 120105\n## Version: v3\n"),
                ("Foo/new.lua", "v3"),
            ]);
            let mut next = candidate(&installation, &bytes);
            next.version = "v3".into();
            next.asset_id = 3;
            next.url = server(bytes.clone());
            assert!(next
                .install_from_receipt(&installation, &id, temp.path(), |_| false)
                .await
                .is_err());
            assert_eq!(fs::read(root.join("Foo/new.lua")).unwrap(), b"v2");
            next.url = server(bytes);
            let updated = next
                .install_from_receipt(&installation, &id, temp.path(), |preview| {
                    assert_eq!(preview.folders, ["Foo"]);
                    let lock = fs::File::open(root.join(".patchloom-install.lock")).unwrap();
                    assert!(
                        lock.try_lock().is_err(),
                        "confirmation must run under installer lock"
                    );
                    true
                })
                .await
                .unwrap();
            assert_ne!(updated.receipt_id, id);
            assert_eq!(updated.folders[0].version.as_deref(), Some("v3"));
            assert_eq!(fs::read(root.join("Foo/new.lua")).unwrap(), b"v3");
            assert!(
                root.join(".patchloom-ownership")
                    .join(format!("{id}.json"))
                    .exists(),
                "prior receipt is retained"
            );
            assert!(
                crate::fs::inspect_ownership(&root, &id).is_err(),
                "stale receipt cannot claim current contents"
            );
        });
    }
    #[test]
    fn receipt_update_rechecks_modified_files_and_confirmation_window() {
        async_std::task::block_on(async {
            for change_during_confirmation in [false, true] {
                let (temp, installation, previous) = fixture();
                let bytes = zip(&[
                    ("Foo/Foo.toc", "## Interface: 120105\n"),
                    ("Foo/new.lua", "v2"),
                ]);
                let mut selected = candidate(&installation, &bytes);
                selected.url = server(bytes.clone());
                let receipt = selected
                    .install(&installation, &previous, temp.path())
                    .await
                    .unwrap();
                let root = installation.directory.join("Interface/AddOns");
                if !change_during_confirmation {
                    fs::write(root.join("Foo/new.lua"), "user-change").unwrap();
                }
                selected.url = server(bytes);
                assert!(selected
                    .install_from_receipt(&installation, &receipt.receipt_id, temp.path(), |_| {
                        assert!(
                            change_during_confirmation,
                            "changed data must fail before confirmation"
                        );
                        fs::write(root.join("Foo/new.lua"), "user-change").unwrap();
                        true
                    })
                    .await
                    .is_err());
                assert_eq!(fs::read(root.join("Foo/new.lua")).unwrap(), b"user-change");
                assert!(!root.join(".patchloom-install").exists());
            }
        });
    }
    #[test]
    fn failed_receipt_publication_reports_committed_state_and_keeps_original_backup() {
        async_std::task::block_on(async {
            let (temp, installation, previous) = fixture();
            let root = installation.directory.join("Interface/AddOns");
            fs::write(root.join(".patchloom-ownership"), b"unowned-conflict").unwrap();
            let bytes = zip(&[
                ("Foo/Foo.toc", "## Interface: 120105\n"),
                ("Foo/new.lua", "new"),
            ]);
            let mut selected = candidate(&installation, &bytes);
            selected.url = server(bytes);
            let error = selected
                .install(&installation, &previous, temp.path())
                .await
                .err()
                .expect("publication conflict must not report success");
            assert!(error.to_string().contains("Addon committed"));
            assert_eq!(
                fs::read(root.join(".patchloom-install/backup/Foo/old.lua")).unwrap(),
                b"preserve"
            );
            assert_eq!(fs::read(root.join("Foo/new.lua")).unwrap(), b"new");
            assert_eq!(
                fs::read(root.join(".patchloom-ownership")).unwrap(),
                b"unowned-conflict"
            );
            assert_eq!(
                crate::fs::inspect_recovery(&root).unwrap().phase,
                "committed-marker"
            );
        });
    }
    #[test]
    #[ignore = "explicit live public contract probe; never part of offline CI"]
    fn public_metadata_manifest_and_zip_contract_probe() {
        assert_eq!(
            std::env::var("PATCHLOOM_PUBLIC_GITHUB_PROBE").as_deref(),
            Ok("1"),
            "Set PATCHLOOM_PUBLIC_GITHUB_PROBE=1 for this explicit live probe"
        );
        async_std::task::block_on(async {
            let (temp, installation, _) = fixture();
            let selected = check("BigWigsMods/BigWigs", &installation).await.unwrap();
            network::download_verified(
                &selected.url,
                &temp.path().join("public-addon.zip"),
                Some(selected.size),
                Some(&selected.digest),
            )
            .await
            .unwrap();
            let archive =
                zip::ZipArchive::new(fs::File::open(temp.path().join("public-addon.zip")).unwrap())
                    .unwrap();
            assert!(!archive.is_empty());
            println!("Verified public release {}: {} bytes, SHA-256 matched; ZIP only read, never installed/executed",selected.version,selected.size);
        });
    }

    // CORE: append inside the existing #[cfg(test)] mod tests containing
    // fixture(), zip(), candidate(), and server(). No existing code changes.
    // Added 2026-10-28; original GPLv3 applies.

    #[test]
    #[ignore = "explicit paired core/app unit-test harness; disposable fixtures only"]
    fn real_writer_receipt_reaches_gui_completion_parent() {
        use std::{
            path::PathBuf,
            process::Command as ProcessCommand,
            time::{Duration, Instant},
        };

        // Explicitly supplied app UNIT-TEST executable, not the application binary.
        // Never search target/ or accept an existing receipt as input.
        let gui_test = PathBuf::from(
            std::env::var_os("PATCHLOOM_GUI_BRIDGE_TEST_EXE")
                .expect("Supply the matching app unit-test executable explicitly"),
        );
        assert!(gui_test.is_absolute(), "Use an absolute executable path");
        assert!(gui_test.is_file(), "App unit-test executable is missing");

        let (temp, installation, previous) = fixture();
        let root = installation.directory.parent().unwrap().to_path_buf();
        let addons = installation.directory.join("Interface/AddOns");
        let ack = temp.path().join("gui-bridge-complete");
        assert_eq!(
            root.parent().unwrap().canonicalize().unwrap(),
            temp.path().canonicalize().unwrap()
        );
        assert!(!ack.exists());

        let bytes = zip(&[
            ("Foo/Foo.toc", "## Interface: 120105\n## Version: v2\n"),
            ("Foo/new.lua", "v2"),
        ]);
        let mut selected = candidate(&installation, &bytes);
        selected.url = server(bytes);

        let installed =
            async_std::task::block_on(selected.install(&installation, &previous, temp.path()))
                .unwrap();
        assert_eq!(installed.folders.len(), 1);
        assert_eq!(installed.folders[0].version.as_deref(), Some("v2"));

        // Forward the ID returned by the real installer, never a manufactured ID
        // or a serialized OwnershipPreview.
        let id = installed.receipt_id.clone();
        let source = serde_json::to_value(&selected).unwrap();
        assert_eq!(source["repository"], "fixture/addon");
        assert_eq!(source["product"], "Retail");
        let receipt_path = addons
            .join(".patchloom-ownership")
            .join(format!("{id}.json"));
        let receipt_before = fs::read(&receipt_path).unwrap();
        let record: serde_json::Value = serde_json::from_slice(&receipt_before).unwrap();
        assert_eq!(record["format"], 1);
        assert_eq!(record["source"], source);

        let toc_before = fs::read(addons.join("Foo/Foo.toc")).unwrap();
        let lua_before = fs::read(addons.join("Foo/new.lua")).unwrap();
        assert_eq!(lua_before, b"v2");
        assert!(!addons.join("Foo/old.lua").exists());

        // The parent retains TempDir ownership until the app test finishes.
        // The child receives fixture location and expectations, not receipt bytes.
        let mut child = ProcessCommand::new(&gui_test)
            .arg("real_writer_receipt_reaches_gui_completion_child")
            .arg("--ignored")
            .arg("--test-threads=1")
            .env("APPDATA", temp.path().join("isolated-appdata"))
            .env("LOCALAPPDATA", temp.path().join("isolated-appdata"))
            .env("XDG_CONFIG_HOME", temp.path().join("isolated-appdata"))
            .env("HOME", temp.path().join("isolated-appdata"))
            .env("PATCHLOOM_GUI_BRIDGE_ROOT", &root)
            .env("PATCHLOOM_GUI_BRIDGE_ID", &id)
            .env("PATCHLOOM_GUI_BRIDGE_SOURCE", source.to_string())
            .spawn()
            .expect("Could not start the supplied app unit-test executable");

        // ponytail: fixed test-only ceiling; revisit only for measured runner needs.
        let deadline = Instant::now() + Duration::from_secs(60);
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                outcome => {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!(
                        "GUI bridge child timed out or could not be waited: {:?}",
                        outcome
                    );
                }
            }
        };
        assert!(status.success(), "GUI bridge child failed: {}", status);

        // A libtest filter matching zero tests also exits successfully. Require a
        // fresh acknowledgement written only after the child's GUI assertions.
        assert_eq!(
            fs::read(&ack).expect("GUI child did not complete its assertions"),
            b"writer-receipt/gui-completion/v1\n",
        );

        assert_eq!(fs::read(&receipt_path).unwrap(), receipt_before);
        assert_eq!(fs::read(addons.join("Foo/Foo.toc")).unwrap(), toc_before);
        assert_eq!(fs::read(addons.join("Foo/new.lua")).unwrap(), lua_before);
        assert!(!addons.join("Foo/old.lua").exists());
    }
}
