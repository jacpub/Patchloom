//! Read-only WoW discovery, 2026-10-27; original Ajour GPLv3 applies.
use serde::Serialize;
use std::{
    collections::{BTreeMap, HashSet},
    fs::{self, File},
    io::{self, Read},
    path::{Path, PathBuf},
};

const MAX_METADATA: u64 = 1024 * 1024;
const MAX_FOLDERS: usize = 10_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Product {
    Retail,
    ClassicProgression,
    ClassicEra,
    Anniversary,
}

impl Product {
    fn identify(id: &str, major: u32) -> io::Result<Self> {
        match (id, major) {
            ("wow", 12) => Ok(Self::Retail),
            ("wow_classic", 5) => Ok(Self::ClassicProgression),
            ("wow_classic_era", 1) => Ok(Self::ClassicEra),
            ("wow_anniversary", 2) => Ok(Self::Anniversary),
            _ => Err(invalid(
                "Unknown product/version family; no legacy flavor fallback",
            )),
        }
    }
    fn folder(self) -> &'static str {
        match self {
            Self::Retail => "_retail_",
            Self::ClassicProgression => "_classic_",
            Self::ClassicEra => "_classic_era_",
            Self::Anniversary => "_anniversary_",
        }
    }
    fn suffixes(self) -> &'static [&'static str] {
        match self {
            Self::Retail => &["Mainline"],
            Self::ClassicProgression => &["Mists"],
            Self::ClassicEra => &["Classic", "Vanilla"],
            Self::Anniversary => &["BCC", "TBC"],
        }
    }
}

#[derive(Debug, Serialize)]
pub struct TocDiscovery {
    pub addon: String,
    pub selected: Option<PathBuf>,
    pub issue: Option<String>,
}
#[derive(Debug, Serialize)]
pub struct Installation {
    pub product_id: String,
    pub product: Product,
    pub version: String,
    pub interface: u32,
    pub directory: PathBuf,
    pub addons: Vec<TocDiscovery>,
}
#[derive(Debug, Serialize)]
pub struct Discovery {
    pub root: PathBuf,
    pub installations: Vec<Installation>,
    pub issues: Vec<String>,
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

fn plain(path: &Path, directory: bool) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return Err(invalid(
                "Links/reparse points are not supported in discovery",
            ));
        }
    }
    if metadata.file_type().is_symlink()
        || (directory && !metadata.is_dir())
        || (!directory && !metadata.is_file())
    {
        return Err(invalid("Unexpected file type or link in discovery"));
    }
    Ok(())
}

fn text(path: &Path) -> io::Result<String> {
    let mut budget = MAX_METADATA;
    read_text(path, &mut budget)
}

fn read_text(path: &Path, budget: &mut u64) -> io::Result<String> {
    if *budget == 0 {
        return Err(invalid("Discovery metadata read budget exhausted"));
    }
    plain(path, false)?;
    let mut bytes = Vec::new();
    let limit = MAX_METADATA.min(*budget);
    File::open(path)?.take(limit + 1).read_to_end(&mut bytes)?;
    *budget = budget.saturating_sub(bytes.len() as u64);
    if bytes.len() as u64 > limit {
        return Err(invalid("Discovery metadata exceeds its read limit"));
    }
    String::from_utf8(bytes).map_err(|_| invalid("Discovery metadata must be UTF-8"))
}

fn active_builds(input: &str) -> io::Result<BTreeMap<String, Vec<String>>> {
    let mut lines = input
        .trim_start_matches('\u{feff}')
        .lines()
        .filter(|line| !line.trim().is_empty() && !line.starts_with('#'));
    let headers: Vec<_> = lines
        .next()
        .ok_or_else(|| invalid("Missing build-info header"))?
        .split('|')
        .collect();
    let mut seen = HashSet::new();
    if headers.len() > 64
        || headers
            .iter()
            .any(|h| !seen.insert(h.split('!').next().unwrap_or_default().to_ascii_lowercase()))
    {
        return Err(invalid("Duplicate build-info column"));
    }
    let column = |name| {
        headers
            .iter()
            .position(|h| *h == name)
            .ok_or_else(|| invalid(format!("Missing build-info column: {name}")))
    };
    let active = column("Active!DEC:1")?;
    let product = column("Product!STRING:0")?;
    let version = column("Version!STRING:0")?;
    let mut builds: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (index, line) in lines.enumerate() {
        if index >= MAX_FOLDERS {
            return Err(invalid("Too many build-info rows"));
        }
        let fields: Vec<_> = line.split('|').collect();
        if fields.len() != headers.len() {
            return Err(invalid("Malformed build-info row"));
        }
        match fields[active] {
            "0" => continue,
            "1" => (),
            _ => return Err(invalid("Invalid active build flag")),
        }
        if fields[product].is_empty()
            || !fields[product]
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_')
        {
            return Err(invalid("Invalid product identifier"));
        }
        builds
            .entry(fields[product].into())
            .or_default()
            .push(fields[version].into());
    }
    Ok(builds)
}

fn version_interface(version: &str) -> io::Result<(u32, u32)> {
    let parts: Vec<u32> = version
        .split('.')
        .map(|v| {
            if !v.is_empty() && v.bytes().all(|c| c.is_ascii_digit()) {
                v.parse().map_err(|_| invalid("Invalid client version"))
            } else {
                Err(invalid("Invalid client version"))
            }
        })
        .collect::<io::Result<_>>()?;
    if parts.len() != 4
        || parts[0] == 0
        || parts[0] > 99
        || parts[1] > 99
        || parts[2] > 99
        || parts[3] == 0
    {
        return Err(invalid("Unsupported client version shape"));
    }
    Ok((parts[0], parts[0] * 10_000 + parts[1] * 100 + parts[2]))
}

// ponytail: exact-interface inventory only; loader precedence is needed before install eligibility.
fn toc_matches(input: &str, interface: u32) -> io::Result<bool> {
    let mut declarations = Vec::new();
    for line in input.trim_start_matches('\u{feff}').lines() {
        if let Some((key, value)) = line
            .trim()
            .strip_prefix("##")
            .and_then(|v| v.split_once(':'))
        {
            if key.trim().to_ascii_lowercase().starts_with("interface-") {
                return Err(invalid(
                    "Qualified TOC Interface headers require resolution",
                ));
            }
            if key.trim().eq_ignore_ascii_case("Interface") {
                let values = value
                    .split(',')
                    .map(|v| {
                        v.trim()
                            .parse::<u32>()
                            .map_err(|_| invalid("Invalid TOC Interface declaration"))
                    })
                    .collect::<io::Result<Vec<_>>>()?;
                declarations.push(values);
            }
        }
    }
    if declarations.len() > 1 {
        return Err(invalid("Ambiguous TOC Interface declarations"));
    }
    Ok(declarations
        .first()
        .is_some_and(|values| values.contains(&interface)))
}

fn addon_toc(
    directory: &Path,
    product: Product,
    interface: u32,
    name: &str,
    budget: &mut u64,
) -> io::Result<TocDiscovery> {
    plain(directory, true)?;
    let mut names = vec![format!("{name}.toc")];
    for suffix in product.suffixes() {
        for separator in ['_', '-'] {
            names.push(format!("{name}{separator}{suffix}.toc"));
        }
    }
    let mut matching = Vec::new();
    for filename in names {
        let path = directory.join(filename);
        match fs::symlink_metadata(&path) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e),
            Ok(_) => (),
        }
        if toc_matches(&read_text(&path, budget)?, interface)? {
            matching.push(path);
        }
    }
    Ok(TocDiscovery {
        addon: name.into(),
        issue: match matching.len() {
            1 => None,
            0 => Some("No exact-interface TOC candidate; no fallback selected".into()),
            _ => Some("Ambiguous matching TOCs; explicit resolution required".into()),
        },
        selected: if matching.len() == 1 {
            matching.pop()
        } else {
            None
        },
    })
}

/// Inventory a caller-supplied copied installation root. No configuration getters or writes.
pub fn discover(root: &Path) -> io::Result<Discovery> {
    plain(root, true)?;
    let root = root.canonicalize()?;
    let builds = active_builds(&text(&root.join(".build.info"))?)?;
    let mut toc_budget = 64 * MAX_METADATA;
    let mut report = Discovery {
        root: root.clone(),
        installations: vec![],
        issues: vec![],
    };
    for (id, versions) in builds {
        let result = (|| -> io::Result<Installation> {
            if versions.len() != 1 {
                return Err(invalid("Ambiguous active product builds"));
            }
            let version = &versions[0];
            let (major, interface) = version_interface(version)?;
            let product = Product::identify(&id, major)?;
            let directory = root.join(product.folder());
            plain(&directory, true)?;
            let mut installation = Installation {
                product_id: id.clone(),
                product,
                version: version.clone(),
                interface,
                directory: directory.clone(),
                addons: vec![],
            };
            let interface_dir = directory.join("Interface");
            match fs::symlink_metadata(&interface_dir) {
                Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(installation),
                Err(e) => return Err(e),
                Ok(_) => (),
            }
            plain(&interface_dir, true)?;
            let addons = interface_dir.join("AddOns");
            match fs::symlink_metadata(&addons) {
                Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(installation),
                Err(e) => return Err(e),
                Ok(_) => (),
            }
            plain(&addons, true)?;
            for (index, entry) in fs::read_dir(&addons)?.enumerate() {
                if index >= MAX_FOLDERS {
                    return Err(invalid("Too many addon entries"));
                }
                let entry = entry?;
                let name = entry
                    .file_name()
                    .into_string()
                    .map_err(|_| invalid("Non-UTF-8 addon name"))?;
                if name.starts_with('.') {
                    continue;
                }
                let toc = addon_toc(&entry.path(), product, interface, &name, &mut toc_budget)
                    .unwrap_or_else(|e| TocDiscovery {
                        addon: name,
                        selected: None,
                        issue: Some(e.to_string()),
                    });
                installation.addons.push(toc);
            }
            installation.addons.sort_by(|a, b| a.addon.cmp(&b.addon));
            Ok(installation)
        })();
        match result {
            Ok(installation) => report.installations.push(installation),
            Err(e) => report.issues.push(format!("{id}: {e}")),
        }
    }
    if report.installations.is_empty() && report.issues.is_empty() {
        report
            .issues
            .push("No active supported WoW products".into());
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    const BUILDS: &str = "Active!DEC:1|Product!STRING:0|Version!STRING:0\n1|wow|12.1.5.70575\n1|wow_classic|5.5.4.70579\n1|wow_classic_era|1.15.9.70576\n1|wow_anniversary|2.5.6.70577\n";

    #[test]
    fn platform_layouts_discover_four_products_without_writes() {
        for layout in [
            "Program Files/World of Warcraft",
            "Applications/World of Warcraft",
            "steamapps/compatdata/0/pfx/drive_c/Program Files/World of Warcraft",
        ] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().join(layout);
            fs::create_dir_all(&root).unwrap();
            fs::write(root.join(".build.info"), BUILDS).unwrap();
            for (product, interface, suffix) in [
                (Product::Retail, 120105, "Mainline"),
                (Product::ClassicProgression, 50504, "Mists"),
                (Product::ClassicEra, 11509, "Vanilla"),
                (Product::Anniversary, 20506, "TBC"),
            ] {
                let addon = root.join(product.folder()).join("Interface/AddOns/Foo");
                fs::create_dir_all(&addon).unwrap();
                fs::write(
                    addon.join(format!("Foo_{suffix}.toc")),
                    format!("## Interface: {interface}\n## Title: Fixture\n"),
                )
                .unwrap();
            }
            let before: Vec<_> = walkdir::WalkDir::new(&root)
                .into_iter()
                .map(|e| e.unwrap().path().to_path_buf())
                .collect();
            let config = crate::config::Config::default();
            let saved = serde_yaml::to_string(&config).unwrap();
            let report = discover(&root).unwrap();
            assert!(report.issues.is_empty());
            assert_eq!(report.installations.len(), 4);
            for installation in report.installations {
                assert!(installation.addons[0].selected.is_some());
                assert!(installation.addons[0].issue.is_none());
            }
            let after: Vec<_> = walkdir::WalkDir::new(&root)
                .into_iter()
                .map(|e| e.unwrap().path().to_path_buf())
                .collect();
            assert_eq!(before, after);
            assert_eq!(
                fs::read_to_string(root.join(".build.info")).unwrap(),
                BUILDS
            );
            assert_eq!(serde_yaml::to_string(&config).unwrap(), saved);
            assert!(!root.join("WTF").exists());
        }
    }

    #[test]
    fn unknown_duplicate_and_future_products_are_not_reinterpreted() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join(".build.info"), "Active!DEC:1|Product!STRING:0|Version!STRING:0\n1|wow_classic|5.5.4.70579\n1|wow_classic|5.5.4.70579\n1|wow_future|13.0.0.1\n1|wow|13.0.0.1\n").unwrap();
        let report = discover(temp.path()).unwrap();
        assert!(report.installations.is_empty());
        assert_eq!(report.issues.len(), 3);
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
        assert!(
            active_builds("Active!DEC:1|Product!STRING:0|Version!STRING:0\n2|wow|12.1.5.1\n")
                .is_err()
        );
        assert!(active_builds("Active!DEC:1|Product!STRING:0|Version!STRING:0\n1|wow\n").is_err());
        assert!(active_builds("Active!DEC:1|Product!STRING:0|Product!STRING:0\n").is_err());
    }

    #[test]
    fn toc_selection_rejects_missing_malformed_and_ambiguous_matches() {
        let temp = tempfile::tempdir().unwrap();
        let mut budget = MAX_METADATA;
        let addon = temp.path().join("Foo");
        fs::create_dir(&addon).unwrap();
        fs::write(addon.join("Foo_Mainline.toc"), "## Interface: 120105\n").unwrap();
        fs::write(addon.join("Foo_Mists.toc"), "## Interface: 50504\n").unwrap();
        assert!(
            addon_toc(&addon, Product::Retail, 120105, "Foo", &mut budget)
                .unwrap()
                .selected
                .unwrap()
                .ends_with("Foo_Mainline.toc")
        );
        assert!(addon_toc(
            &addon,
            Product::ClassicProgression,
            50504,
            "Foo",
            &mut budget
        )
        .unwrap()
        .selected
        .unwrap()
        .ends_with("Foo_Mists.toc"));
        fs::write(addon.join("Foo.toc"), "## Interface: 120105, 50504\n").unwrap();
        assert!(
            addon_toc(&addon, Product::Retail, 120105, "Foo", &mut budget)
                .unwrap()
                .selected
                .is_none()
        );
        assert!(!toc_matches("## Interface: 11509\n", 120105).unwrap());
        assert!(toc_matches("## Interface: bad\n", 120105).is_err());
        assert!(toc_matches("## Interface: 120105\n## Interface: 120105\n", 120105).is_err());
        assert!(toc_matches(
            "## Interface: 120105\n## Interface-Mainline: 11509\n",
            120105
        )
        .is_err());
    }

    #[test]
    fn missing_optional_directories_are_never_created_and_metadata_is_bounded() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(
            temp.path().join(".build.info"),
            "Product!STRING:0|Version!STRING:0|Active!DEC:1\nwow|12.1.5.70575|1\n",
        )
        .unwrap();
        let product = temp.path().join("_retail_");
        fs::create_dir(&product).unwrap();
        assert!(discover(temp.path()).unwrap().installations[0]
            .addons
            .is_empty());
        assert!(!product.join("Interface").exists());
        fs::write(
            temp.path().join(".build.info"),
            vec![b'x'; MAX_METADATA as usize + 1],
        )
        .unwrap();
        assert!(discover(temp.path()).is_err());
    }

    #[test]
    fn total_read_budget_and_duplicate_semantic_headers_fail_closed() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("metadata");
        fs::write(&path, "123456").unwrap();
        let mut budget = 10;
        assert_eq!(read_text(&path, &mut budget).unwrap(), "123456");
        assert_eq!(budget, 4);
        assert!(read_text(&path, &mut budget).is_err());
        assert_eq!(budget, 0);
        assert!(read_text(&path, &mut budget).is_err());
        assert!(
            active_builds("Active!DEC:1|Product!STRING:0|Version!STRING:0|Version!STRING:1\n")
                .is_err()
        );
    }

    #[test]
    fn product_directory_links_are_not_traversed() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("wow");
        let outside = temp.path().join("outside");
        fs::create_dir(&root).unwrap();
        fs::create_dir(&outside).unwrap();
        fs::write(
            root.join(".build.info"),
            "Active!DEC:1|Product!STRING:0|Version!STRING:0\n1|wow|12.1.5.70575\n",
        )
        .unwrap();
        let link = root.join("_retail_");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            let result = std::process::Command::new("cmd.exe")
                .args(["/D", "/C", "mklink", "/J"])
                .arg(&link)
                .arg(&outside)
                .creation_flags(0x08000000)
                .output()
                .unwrap();
            assert!(result.status.success(), "{:?}", result);
        }
        let report = discover(&root).unwrap();
        assert!(report.installations.is_empty());
        assert_eq!(report.issues.len(), 1);
        assert!(report.issues[0].contains("link") || report.issues[0].contains("reparse"));
        assert_eq!(fs::read_dir(&outside).unwrap().count(), 0);
    }
}
