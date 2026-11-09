//! Copy-only legacy settings inspection, 2026-11-09; original GPLv3 applies.
//! Reports are unverified hints, not an import authorization or path validation.
//!
//! The caller supplies an optional UTF-8 copy. None means explicit absence;
//! callers must preserve their own file-read errors and bound file reading.
//! This API accepts at most 64 KiB, 16 collection levels, 4096 events, 8192
//! scanner tokens, 4096 bytes per scalar/path, 64 bytes per flavor label and
//! 16 directory hints. It rejects aliases, anchors, tags/directives, merge keys,
//! duplicate/complex/non-string keys and multiple documents. The token pass
//! precedes event checks; backend lookahead may consume bounded input before a
//! rejection. These are parsing/data bounds, not a wall-clock or streaming quota.
//!
//! Only the unversioned legacy wow shape is inspected. Other settings are
//! counted, not validated or imported. Missing flavors stay unspecified;
//! Classic, unknown and test labels require later resolution. RetailLabel is
//! a legacy label only. Accepted path strings remain unchanged and unverified:
//! filtering does not establish existence, portable validity or WoW support.
//! The Classic basename flag is a literal clue, not a filesystem identity test.
//!
//! Reports carry source identity only. Current-product verification, resolving
//! relative paths, choosing settings, retaining unknown fields, fresh consent
//! and safe persistence remain separate, unimplemented migration decisions.
use serde::Serialize;
use serde_yaml::{Mapping, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use yaml_rust::parser::{Event, Parser};
use yaml_rust::scanner::{Scanner, TokenType};

pub const MAX_INPUT_BYTES: usize = 64 * 1024;
pub const MAX_HINTS: usize = 16;
pub const MAX_FIELD_BYTES: usize = 4096;
const MAX_DEPTH: usize = 16;
const MAX_EVENTS: usize = 4096;
const MAX_TOKENS: usize = 8192;
const MAX_FLAVOR_BYTES: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum Preflight {
    Absent,
    Present(Report),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Report {
    pub source_sha256: String,
    pub source_bytes: usize,
    pub selected_legacy_flavor: Option<FlavorHint>,
    pub directories: Vec<DirectoryHint>,
    pub uninspected_root_fields: usize,
    pub uninspected_wow_fields: usize,
    pub notices: Vec<Notice>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FlavorHint {
    pub label: String,
    pub kind: FlavorKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum FlavorKind {
    RetailLabel,
    ClassicRequiresResolution,
    UnsupportedTestVariant,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DirectoryHint {
    pub legacy_flavor: Option<FlavorHint>,
    pub path_hint: String,
    pub classic_folder_ambiguous: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Notice {
    NoDirectoryHints,
    LegacyRootNeedsProductSelection,
    MultipleLocationsNeedResolution,
    OtherSettingsNotInspected,
    ClassicNeedsResolution,
    UnsupportedOrUnknownFlavor,
    RepeatedPathHints,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PreflightError {
    #[error("No configuration data was supplied")]
    Empty,
    #[error("Configuration copy exceeds a preflight resource limit: {0}")]
    Limit(&'static str),
    #[error("Configuration copy could not be decoded as supported UTF-8 YAML")]
    InvalidYaml,
    #[error("Unsupported YAML construction")]
    UnsupportedYaml,
    #[error("Duplicate mapping key in configuration copy")]
    DuplicateKey,
    #[error("Only an unversioned legacy configuration with a wow mapping is supported")]
    UnsupportedSchema,
    #[error("Invalid legacy configuration field: {0}")]
    InvalidField(&'static str),
    #[error("Unsafe or unsupported directory hint")]
    UnsafePath,
}

/// Inspect a caller-owned copy. None is absence; empty/malformed data is an error.
/// This never loads settings, follows hints or applies/persists an import.
pub fn inspect_copy(source: Option<&[u8]>) -> Result<Preflight, PreflightError> {
    let Some(source) = source else {
        return Ok(Preflight::Absent);
    };
    if source.len() > MAX_INPUT_BYTES {
        return Err(PreflightError::Limit("input bytes"));
    }
    let text = std::str::from_utf8(source).map_err(|_| PreflightError::InvalidYaml)?;
    if text.trim().is_empty() {
        return Err(PreflightError::Empty);
    }
    guard_yaml(text)?;
    let value: Value = serde_yaml::from_str(text).map_err(|_| PreflightError::InvalidYaml)?;
    // Non-string map keys are unsupported, including in uninspected settings.
    // This also excludes scalar-type key collisions after YAML resolution.
    let mut pending = vec![&value];
    while let Some(value) = pending.pop() {
        match value {
            Value::Mapping(map) => {
                for (key, value) in map {
                    if key.as_str().is_none() {
                        return Err(PreflightError::UnsupportedYaml);
                    }
                    pending.push(value);
                }
            }
            Value::Sequence(items) => pending.extend(items),
            _ => {}
        }
    }
    let root = value
        .as_mapping()
        .ok_or(PreflightError::UnsupportedSchema)?;
    for key in ["version", "schema", "schema_version", "schemaVersion"] {
        if get(root, key).is_some() {
            return Err(PreflightError::UnsupportedSchema);
        }
    }
    let wow = get(root, "wow")
        .and_then(Value::as_mapping)
        .ok_or(PreflightError::UnsupportedSchema)?;
    let selected = optional_string(get(wow, "flavor"), "wow.flavor")?
        .map(flavor)
        .transpose()?;
    let mut report = Report {
        source_sha256: format!("{:x}", Sha256::digest(source)),
        source_bytes: source.len(),
        selected_legacy_flavor: selected,
        directories: Vec::new(),
        uninspected_root_fields: root.len().saturating_sub(1),
        uninspected_wow_fields: wow
            .iter()
            .filter(|(key, _)| {
                !matches!(key.as_str(), Some("directory" | "directories" | "flavor"))
            })
            .count(),
        notices: Vec::new(),
    };
    if let Some(path) = optional_string(get(wow, "directory"), "wow.directory")? {
        add_directory(&mut report, path, None)?;
        notice(&mut report, Notice::LegacyRootNeedsProductSelection);
    }
    if let Some(value) = get(wow, "directories").filter(|v| !v.is_null()) {
        let map = value
            .as_mapping()
            .ok_or(PreflightError::InvalidField("wow.directories"))?;
        // Order is deterministic while original labels and complete paths remain intact.
        let mut entries: Vec<_> = map.iter().collect();
        entries.sort_by_key(|(key, _)| key.as_str());
        for (key, value) in entries {
            let label = key
                .as_str()
                .ok_or(PreflightError::InvalidField("directory flavor"))?;
            let path = value
                .as_str()
                .ok_or(PreflightError::InvalidField("directory path"))?;
            add_directory(&mut report, path, Some(flavor(label)?))?;
        }
    }
    if report.directories.is_empty() {
        notice(&mut report, Notice::NoDirectoryHints);
    }
    if report.directories.len() > 1 {
        notice(&mut report, Notice::MultipleLocationsNeedResolution);
    }
    if report.uninspected_root_fields != 0 || report.uninspected_wow_fields != 0 {
        notice(&mut report, Notice::OtherSettingsNotInspected);
    }
    if report
        .selected_legacy_flavor
        .iter()
        .chain(
            report
                .directories
                .iter()
                .filter_map(|h| h.legacy_flavor.as_ref()),
        )
        .any(|h| h.kind == FlavorKind::ClassicRequiresResolution)
        || report
            .directories
            .iter()
            .any(|h| h.classic_folder_ambiguous)
    {
        notice(&mut report, Notice::ClassicNeedsResolution);
    }
    if report
        .selected_legacy_flavor
        .iter()
        .chain(
            report
                .directories
                .iter()
                .filter_map(|h| h.legacy_flavor.as_ref()),
        )
        .any(|h| {
            matches!(
                h.kind,
                FlavorKind::Unknown | FlavorKind::UnsupportedTestVariant
            )
        })
    {
        notice(&mut report, Notice::UnsupportedOrUnknownFlavor);
    }
    Ok(Preflight::Present(report))
}

fn get<'a>(map: &'a Mapping, key: &str) -> Option<&'a Value> {
    map.get(&Value::String(key.to_owned()))
}

fn optional_string<'a>(
    value: Option<&'a Value>,
    field: &'static str,
) -> Result<Option<&'a str>, PreflightError> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value)),
        _ => Err(PreflightError::InvalidField(field)),
    }
}

fn flavor(label: &str) -> Result<FlavorHint, PreflightError> {
    if label.len() > MAX_FLAVOR_BYTES {
        return Err(PreflightError::Limit("flavor label"));
    }
    if label.is_empty() || label.chars().any(unsafe_character) {
        return Err(PreflightError::InvalidField("flavor label"));
    }
    let kind = match label {
        "Retail" | "retail" | "mainline" | "wow_retail" => FlavorKind::RetailLabel,
        "Classic"
        | "classic"
        | "ClassicEra"
        | "wow_classic"
        | "classic_era"
        | "ClassicTbc"
        | "wow_burning_crusade"
        | "burningCrusade"
        | "burning_crusade"
        | "bcc" => FlavorKind::ClassicRequiresResolution,
        "RetailPtr" | "RetailPTR" | "RetailBeta" | "ClassicPtr" | "ClassicPTR" | "ClassicBeta"
        | "ClassicEraPtr" | "ClassicEraPTR" => FlavorKind::UnsupportedTestVariant,
        _ => FlavorKind::Unknown,
    };
    Ok(FlavorHint {
        label: label.to_owned(),
        kind,
    })
}

fn unsafe_character(c: char) -> bool {
    c.is_control() || matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
}

fn add_directory(
    report: &mut Report,
    path: &str,
    legacy_flavor: Option<FlavorHint>,
) -> Result<(), PreflightError> {
    if report.directories.len() >= MAX_HINTS {
        return Err(PreflightError::Limit("directory hints"));
    }
    if path.len() > MAX_FIELD_BYTES {
        return Err(PreflightError::Limit("path bytes"));
    }
    if path.is_empty()
        || path.trim() != path
        || path.chars().any(unsafe_character)
        || path.contains("://")
        || path.starts_with('\\')
        || path.starts_with("//")
        || path
            .split(['/', '\\'])
            .any(|part| matches!(part, "." | ".."))
        || (path.as_bytes().get(1) == Some(&b':')
            && !matches!(path.as_bytes().get(2), Some(b'/' | b'\\')))
    {
        return Err(PreflightError::UnsafePath);
    }
    if report.directories.iter().any(|h| h.path_hint == path) {
        notice(report, Notice::RepeatedPathHints);
    }
    let tail = path
        .trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("");
    report.directories.push(DirectoryHint {
        legacy_flavor,
        path_hint: path.to_owned(),
        classic_folder_ambiguous: tail.eq_ignore_ascii_case("_classic_"),
    });
    Ok(())
}

fn notice(report: &mut Report, value: Notice) {
    if !report.notices.contains(&value) {
        report.notices.push(value);
    }
}

struct MapState {
    key: bool,
    seen: BTreeSet<String>,
}

// serde_yaml0.8's loader recurses before its deserializer depth check. Guard
// using its exact locked backend's iterative APIs before constructing a Value.
fn guard_yaml(text: &str) -> Result<(), PreflightError> {
    guard_tokens(text.chars())?;
    guard_events(text.chars())
}

fn guard_tokens(input: impl Iterator<Item = char>) -> Result<(), PreflightError> {
    let mut scanner = Scanner::new(input);
    for (index, token) in scanner.by_ref().enumerate() {
        if index >= MAX_TOKENS {
            return Err(PreflightError::Limit("YAML tokens"));
        }
        match token.1 {
            TokenType::Alias(_)
            | TokenType::Anchor(_)
            | TokenType::Tag(_, _)
            | TokenType::TagDirective(_, _)
            | TokenType::VersionDirective(_, _) => return Err(PreflightError::UnsupportedYaml),
            TokenType::Scalar(_, ref value) if value.len() > MAX_FIELD_BYTES => {
                return Err(PreflightError::Limit("scalar bytes"))
            }
            _ => {}
        }
    }
    if scanner.get_error().is_some() {
        return Err(PreflightError::InvalidYaml);
    }
    Ok(())
}

fn guard_events(input: impl Iterator<Item = char>) -> Result<(), PreflightError> {
    let mut parser = Parser::new(input);
    let mut stack: Vec<Option<MapState>> = Vec::new();
    let mut documents = 0;
    for _ in 0..MAX_EVENTS {
        let (event, _) = parser.next().map_err(|_| PreflightError::InvalidYaml)?;
        match event {
            Event::StreamEnd => {
                return if documents == 1 && stack.is_empty() {
                    Ok(())
                } else {
                    Err(PreflightError::InvalidYaml)
                }
            }
            Event::DocumentStart => {
                documents += 1;
                if documents > 1 {
                    return Err(PreflightError::UnsupportedYaml);
                }
            }
            Event::MappingStart(_) | Event::SequenceStart(_) => {
                if let Some(Some(parent)) = stack.last_mut() {
                    if parent.key {
                        return Err(PreflightError::UnsupportedYaml);
                    }
                    parent.key = true;
                }
                if stack.len() >= MAX_DEPTH {
                    return Err(PreflightError::Limit("YAML depth"));
                }
                stack.push(if matches!(event, Event::MappingStart(_)) {
                    Some(MapState {
                        key: true,
                        seen: BTreeSet::new(),
                    })
                } else {
                    None
                });
            }
            Event::MappingEnd | Event::SequenceEnd => {
                stack.pop().ok_or(PreflightError::InvalidYaml)?;
            }
            Event::Scalar(value, _, _, _) => {
                if let Some(Some(map)) = stack.last_mut() {
                    if map.key {
                        if value == "<<" {
                            return Err(PreflightError::UnsupportedYaml);
                        }
                        if !map.seen.insert(value) {
                            return Err(PreflightError::DuplicateKey);
                        }
                    }
                    map.key = !map.key;
                }
            }
            Event::Alias(_) => return Err(PreflightError::UnsupportedYaml),
            _ => {}
        }
    }
    Err(PreflightError::Limit("YAML events"))
}

#[cfg(test)]
#[path = "legacy_import_tests.rs"]
mod tests;
