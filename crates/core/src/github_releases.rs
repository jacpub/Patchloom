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
    /// Caller explicitly supplies previously owned folders; discovery alone never proves ownership.
    /// The returned in-memory receipt is produced only after the installer commits.
    pub async fn install(
        &self,
        installation: &Installation,
        previous: &[AddonFolder],
        cache: &Path,
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
        let folders =
            crate::fs::install_addon_checked(&addon, temporary.path(), &addons, |staged, _| {
                discovery::parse_product_tocs(staged, self.product, self.interface)
                    .map_err(Into::into)
            })
            .await?;
        Ok(Installed {
            candidate: self.clone(),
            folders,
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
}
