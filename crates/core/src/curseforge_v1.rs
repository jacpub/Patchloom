//! Offline CurseForge v1 parsing/eligibility foundation, added 2026-11-05.
//! Original Ajour contributors' attribution and GPLv3 license apply.
//!
//! This module performs no HTTP, authentication, persistence, configuration,
//! fingerprinting, download or installation operations.
//!
//! Caller contract:
//! - Independently verify the binding between a discovered Product, game ID and
//!   official version-type ID. No current flavor IDs are defined here.
//! - Supply every page of ONE file-list query, starting at index zero, with the
//!   same page size and exact gameVersionTypeId/gameVersion filters.
//! - Attach exactly one checked FilterEvidence to EACH response. This is a
//!   caller assertion about the actual request/response association, NOT proof
//!   extracted from JSON. Do not attach it to unfiltered or uncertain responses.
//! - Keep query results coherent; pagination checks cannot detect a remote
//!   dataset changing without changing its total or producing duplicates.
//!
//! Official File.gameVersions contains version strings, not a verified product
//! mapping. FileIndex and SortableGameVersion are not treated as File schemas;
//! an undocumented File.gameVersionTypeIds field is never used as evidence.
//!
//! Candidates are inert metadata, not download authorization, cryptographic
//! authenticity, ownership evidence or compatibility validation of an archive.
//! A missing downloadUrl is unavailable: no URL is reconstructed. A later
//! official download-url request may still return 404.
//!
//! Runtime approval terms, credential handling and provider integration remain
//! outside this module.
//! File.fileStatus is not interpreted here; availability and release channel
//! do not establish moderation status or authorize transport. URLs are screened
//! for basic HTTPS syntax only, not DNS, redirect policy or approved CDN hosts.
//! Hashes are provider claims and are never recomputed by this module.

use crate::discovery::Product;
use serde::{de::DeserializeOwned, Deserialize};
use std::collections::HashSet;

pub const MAX_JSON_BYTES: usize = 1024 * 1024;
pub const MAX_BATCH_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_PAGES: usize = 200;
pub const MAX_FILES: u32 = 10_000;
pub const MAX_PAGE_SIZE: u32 = 50;

const MAX_NAME_BYTES: usize = 512;
const MAX_URL_BYTES: usize = 4096;
const MAX_VERSION_BYTES: usize = 128;
const MAX_VERSIONS: usize = 64;
const MAX_HASHES: usize = 8;
const MAX_HASH_BYTES: usize = 128;
const MAX_FILE_BYTES: u64 = 128 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    OversizedJson,
    MalformedJson,
    MetadataLimit,
    InvalidIdentity,
    IdentityMismatch,
    InvalidRequest,
    TooManyPages,
    BatchLimit,
    InconsistentPagination,
    IncompletePages,
    DuplicateFile,
    MissingVersionEvidence,
    AmbiguousVersionEvidence,
    VersionEvidenceMismatch,
    InvalidHash,
    DuplicateHashAlgorithm,
}

impl std::fmt::Display for Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Fixed errors only; never include response text or URLs in diagnostics.
        write!(formatter, "Offline CurseForge metadata error: {:?}", self)
    }
}

impl std::error::Error for Error {}

type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Distribution {
    Allowed,
    Denied,
    Unknown,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModMetadata {
    id: u32,
    game_id: u32,
    allow_mod_distribution: Option<bool>,
    // Search visibility only; deliberately not an eligibility condition.
    is_available: bool,
}

impl ModMetadata {
    pub fn id(&self) -> u32 {
        self.id
    }

    pub fn game_id(&self) -> u32 {
        self.game_id
    }

    pub fn search_visible(&self) -> bool {
        self.is_available
    }

    pub fn distribution(&self) -> Distribution {
        match self.allow_mod_distribution {
            Some(true) => Distribution::Allowed,
            Some(false) => Distribution::Denied,
            None => Distribution::Unknown,
        }
    }
}

#[derive(Deserialize)]
struct Envelope<T> {
    data: T,
}

fn parse<T: DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    if bytes.len() > MAX_JSON_BYTES {
        return Err(Error::OversizedJson);
    }
    // Keep serde_json's default recursion limit; no unbounded parsing mode.
    serde_json::from_slice(bytes).map_err(|_| Error::MalformedJson)
}

/// Parse the standard GET /v1/mods/{modId} data envelope.
/// IDs are caller expectations, not values inferred from a filename or config.
pub fn parse_mod(bytes: &[u8], expected_mod_id: u32, expected_game_id: u32) -> Result<ModMetadata> {
    if expected_mod_id == 0 || expected_game_id == 0 {
        return Err(Error::InvalidIdentity);
    }
    let response: Envelope<ModMetadata> = parse(bytes)?;
    if response.data.id != expected_mod_id || response.data.game_id != expected_game_id {
        return Err(Error::IdentityMismatch);
    }
    Ok(response.data)
}

/// Explicit caller-verified product/version-type binding.
/// Synthetic IDs in tests are not current WoW mappings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Compatibility {
    pub product: Product,
    pub game_id: u32,
    pub version_type_id: u32,
    pub game_version: String,
}

impl Compatibility {
    fn validate(&self) -> Result<()> {
        if self.game_id == 0 || self.version_type_id == 0 {
            return Err(Error::InvalidIdentity);
        }
        bounded_text(&self.game_version, MAX_VERSION_BYTES)
    }
}

/// Checked request association supplied by the caller, not parsed from File.
/// Both filters must actually have been used for this exact response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilterEvidence {
    pub mod_id: u32,
    pub game_id: u32,
    pub version_type_id: u32,
    pub game_version: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageRequest {
    index: u32,
    page_size: u32,
}

impl PageRequest {
    /// None means the documented default page size of 50.
    pub fn new(index: u32, page_size: Option<u32>) -> Result<Self> {
        let page_size = page_size.unwrap_or(MAX_PAGE_SIZE);
        if page_size == 0
            || page_size > MAX_PAGE_SIZE
            || index
                .checked_add(page_size)
                .is_none_or(|end| end > MAX_FILES)
        {
            return Err(Error::InvalidRequest);
        }
        Ok(Self { index, page_size })
    }

    pub fn index(self) -> u32 {
        self.index
    }

    pub fn page_size(self) -> u32 {
        self.page_size
    }
}

/// Borrowed inputs prevent this module from fetching or retaining responses.
/// Zero evidence entries means missing; more than one means ambiguous.
pub struct PageInput<'a> {
    pub request: PageRequest,
    pub evidence: &'a [FilterEvidence],
    pub json: &'a [u8],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    Stable,
    Beta,
    Alpha,
}

impl Channel {
    fn number(self) -> u32 {
        match self {
            Self::Stable => 1,
            Self::Beta => 2,
            Self::Alpha => 3,
        }
    }
}

/// Provider-reported hashes only. No digest is recomputed here.
/// Neither SHA1 nor MD5 is SHA256, authenticity or ownership proof.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderHash {
    Sha1(String),
    Md5(String),
    Unsupported { algorithm: u32, value: String },
}

/// Inert metadata with no install/download method or conversion to the
/// inherited GitHub Candidate, downloader or ownership receipt.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub mod_id: u32,
    pub game_id: u32,
    pub file_id: u32,
    pub compatibility: Compatibility,
    pub channel: Channel,
    pub display_name: String,
    pub file_name: String,
    pub file_length: u64,
    pub download_url: String,
    pub hashes: Vec<ProviderHash>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unavailable {
    DistributionDenied,
    DistributionUnknown,
    NoEligibleFile,
    MissingDownloadUrl,
    AmbiguousFiles,
}

#[derive(Debug, Clone)]
pub enum Selection {
    Candidate(Candidate),
    Unavailable(Unavailable),
}

#[derive(Deserialize)]
struct FilePage {
    data: Vec<FileMetadata>,
    pagination: Pagination,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Pagination {
    index: u32,
    page_size: u32,
    result_count: u32,
    total_count: u32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FileMetadata {
    id: u32,
    game_id: u32,
    mod_id: u32,
    is_available: bool,
    release_type: u32,
    display_name: String,
    file_name: String,
    file_length: u64,
    download_url: Option<String>,
    game_versions: Vec<String>,
    hashes: Vec<RawHash>,
}

#[derive(Deserialize)]
struct RawHash {
    algo: u32,
    value: String,
}

fn bounded_text(value: &str, limit: usize) -> Result<()> {
    if value.is_empty() || value.len() > limit || value.chars().any(char::is_control) {
        return Err(Error::MetadataLimit);
    }
    Ok(())
}

fn validate_hashes(hashes: &[RawHash]) -> Result<()> {
    if hashes.len() > MAX_HASHES {
        return Err(Error::MetadataLimit);
    }
    let mut algorithms = HashSet::new();
    for hash in hashes {
        bounded_text(&hash.value, MAX_HASH_BYTES)?;
        if !algorithms.insert(hash.algo) {
            return Err(Error::DuplicateHashAlgorithm);
        }
        let required_length = match hash.algo {
            1 => Some(40),
            2 => Some(32),
            _ => None,
        };
        if required_length.is_some_and(|length| {
            hash.value.len() != length || !hash.value.bytes().all(|byte| byte.is_ascii_hexdigit())
        }) {
            return Err(Error::InvalidHash);
        }
    }
    Ok(())
}

fn validate_file(file: &FileMetadata, metadata: &ModMetadata) -> Result<()> {
    if file.id == 0 {
        return Err(Error::InvalidIdentity);
    }
    if file.mod_id != metadata.id || file.game_id != metadata.game_id {
        return Err(Error::IdentityMismatch);
    }
    bounded_text(&file.display_name, MAX_NAME_BYTES)?;
    bounded_text(&file.file_name, MAX_NAME_BYTES)?;
    if file.file_length == 0 || file.file_length > MAX_FILE_BYTES {
        return Err(Error::MetadataLimit);
    }
    if let Some(url) = &file.download_url {
        // A bounded opaque string, NOT URL validation or download permission.
        bounded_text(url, MAX_URL_BYTES)?;
        let uri: isahc::http::Uri = url.parse().map_err(|_| Error::MetadataLimit)?;
        if uri.scheme_str() != Some("https")
            || uri.host().is_none()
            || uri.authority().is_some_and(|a| a.as_str().contains('@'))
        {
            return Err(Error::MetadataLimit);
        }
    }
    if file.game_versions.len() > MAX_VERSIONS {
        return Err(Error::MetadataLimit);
    }
    let mut versions = HashSet::new();
    for version in &file.game_versions {
        bounded_text(version, MAX_VERSION_BYTES)?;
        if !versions.insert(version) {
            return Err(Error::MetadataLimit);
        }
    }
    validate_hashes(&file.hashes)
}

fn validate_evidence(
    evidence: &[FilterEvidence],
    metadata: &ModMetadata,
    compatibility: &Compatibility,
) -> Result<()> {
    let checked = match evidence {
        [] => return Err(Error::MissingVersionEvidence),
        [checked] => checked,
        _ => return Err(Error::AmbiguousVersionEvidence),
    };
    bounded_text(&checked.game_version, MAX_VERSION_BYTES)?;
    if checked.mod_id != metadata.id
        || checked.game_id != compatibility.game_id
        || checked.version_type_id != compatibility.version_type_id
        || checked.game_version != compatibility.game_version
    {
        return Err(Error::VersionEvidenceMismatch);
    }
    Ok(())
}

/// Select only from a complete, bounded file-list response set.
///
/// This deliberately rejects competing compatible files instead of guessing
/// newest from IDs, dates, response order or local filenames. The requested
/// channel is exact: beta/alpha/unknown never fall back to stable.
///
/// Structural errors anywhere fail the whole batch, including unavailable
/// files. Unsupported hash algorithms remain explicitly unsupported metadata.
///
/// ponytail: at most 200 pages / 10,000 files / 16 MiB JSON per batch.
/// Queries needing more pages are unavailable here; increase only after a
/// measured need and corresponding resource-limit review.
pub fn select(
    metadata: &ModMetadata,
    compatibility: &Compatibility,
    channel: Channel,
    pages: &[PageInput<'_>],
) -> Result<Selection> {
    compatibility.validate()?;
    if metadata.id == 0 || metadata.game_id == 0 {
        return Err(Error::InvalidIdentity);
    }
    if compatibility.game_id != metadata.game_id {
        return Err(Error::IdentityMismatch);
    }
    if pages.is_empty() {
        return Err(Error::IncompletePages);
    }
    if pages.len() > MAX_PAGES {
        return Err(Error::TooManyPages);
    }

    // Check ALL byte budgets before any page is deserialized.
    let mut bytes = 0usize;
    for page in pages {
        if page.json.len() > MAX_JSON_BYTES {
            return Err(Error::OversizedJson);
        }
        bytes = bytes
            .checked_add(page.json.len())
            .ok_or(Error::BatchLimit)?;
        if bytes > MAX_BATCH_BYTES {
            return Err(Error::BatchLimit);
        }
    }

    let page_size = pages[0].request.page_size;
    let mut next_index = 0u32;
    let mut total = None;
    let mut seen = HashSet::new();
    let mut files = Vec::new();

    for (position, input) in pages.iter().enumerate() {
        validate_evidence(input.evidence, metadata, compatibility)?;
        if input.request.index != next_index || input.request.page_size != page_size {
            return Err(Error::InconsistentPagination);
        }
        let page: FilePage = parse(input.json)?;
        let pagination = page.pagination;
        if pagination.index != input.request.index
            || pagination.page_size != page_size
            || pagination.total_count > MAX_FILES
            || pagination.result_count > page_size
            || page.data.len() != pagination.result_count as usize
        {
            return Err(Error::InconsistentPagination);
        }
        if total.is_some_and(|expected| expected != pagination.total_count) {
            return Err(Error::InconsistentPagination);
        }
        total = Some(pagination.total_count);

        let remaining = pagination
            .total_count
            .checked_sub(next_index)
            .ok_or(Error::InconsistentPagination)?;
        if pagination.result_count != remaining.min(page_size) {
            return Err(Error::InconsistentPagination);
        }
        // Exactly one empty page is permitted for an empty result set.
        if remaining == 0 && (position != 0 || pages.len() != 1) {
            return Err(Error::InconsistentPagination);
        }
        for file in page.data {
            validate_file(&file, metadata)?;
            if !seen.insert(file.id) {
                return Err(Error::DuplicateFile);
            }
            files.push(file);
        }
        next_index = next_index
            .checked_add(pagination.result_count)
            .ok_or(Error::InconsistentPagination)?;
    }
    if total != Some(next_index) {
        return Err(Error::IncompletePages);
    }

    match metadata.distribution() {
        Distribution::Denied => {
            return Ok(Selection::Unavailable(Unavailable::DistributionDenied));
        }
        Distribution::Unknown => {
            return Ok(Selection::Unavailable(Unavailable::DistributionUnknown));
        }
        Distribution::Allowed => {}
    }

    let mut eligible = files.into_iter().filter(|file| {
        file.is_available
            && file.release_type == channel.number()
            && file
                .game_versions
                .iter()
                .any(|version| version == &compatibility.game_version)
    });
    let Some(file) = eligible.next() else {
        return Ok(Selection::Unavailable(Unavailable::NoEligibleFile));
    };
    // Count competing files even when one lacks a URL; do not silently fall back.
    if eligible.next().is_some() {
        return Ok(Selection::Unavailable(Unavailable::AmbiguousFiles));
    }
    let Some(download_url) = file.download_url else {
        return Ok(Selection::Unavailable(Unavailable::MissingDownloadUrl));
    };
    let hashes = file
        .hashes
        .into_iter()
        .map(|hash| match hash.algo {
            1 => ProviderHash::Sha1(hash.value),
            2 => ProviderHash::Md5(hash.value),
            algorithm => ProviderHash::Unsupported {
                algorithm,
                value: hash.value,
            },
        })
        .collect();

    Ok(Selection::Candidate(Candidate {
        mod_id: metadata.id,
        game_id: metadata.game_id,
        file_id: file.id,
        compatibility: compatibility.clone(),
        channel,
        display_name: file.display_name,
        file_name: file.file_name,
        file_length: file.file_length,
        download_url,
        hashes,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    // Entirely synthetic: neither game nor flavor IDs are current WoW IDs.
    const GAME: u32 = 900_001;
    const MOD: u32 = 900_002;
    const TYPE: u32 = 900_003;

    fn compatibility() -> Compatibility {
        Compatibility {
            product: Product::Anniversary,
            game_id: GAME,
            version_type_id: TYPE,
            game_version: "fixture-version-1".into(),
        }
    }

    fn evidence() -> Vec<FilterEvidence> {
        vec![FilterEvidence {
            mod_id: MOD,
            game_id: GAME,
            version_type_id: TYPE,
            game_version: compatibility().game_version,
        }]
    }

    fn mod_json() -> Value {
        json!({
            "data": {
                "id": MOD,
                "gameId": GAME,
                "allowModDistribution": true,
                "isAvailable": true
            }
        })
    }

    fn metadata() -> ModMetadata {
        parse_mod(&serde_json::to_vec(&mod_json()).unwrap(), MOD, GAME).unwrap()
    }

    fn file(id: u32) -> Value {
        json!({
            "id": id,
            "gameId": GAME,
            "modId": MOD,
            "isAvailable": true,
            "releaseType": 1,
            "displayName": "Synthetic fixture",
            "fileName": "Fixture_Mainline_Classic.zip",
            "fileLength": 123,
            "downloadUrl": "https://fixture.invalid/opaque-package",
            "gameVersions": ["fixture-version-1"],
            "hashes": [
                {"algo": 1, "value": "a".repeat(40)},
                {"algo": 2, "value": "B".repeat(32)}
            ]
        })
    }

    fn page(files: Vec<Value>, index: u32, size: u32, total: u32) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "data": files,
            "pagination": {
                "index": index,
                "pageSize": size,
                "resultCount": files.len(),
                "totalCount": total
            }
        }))
        .unwrap()
    }

    fn run_with(
        metadata: &ModMetadata,
        compatibility: &Compatibility,
        channel: Channel,
        evidence: &[FilterEvidence],
        files: Vec<Value>,
    ) -> Result<Selection> {
        let total = files.len() as u32;
        let bytes = page(files, 0, 50, total);
        select(
            metadata,
            compatibility,
            channel,
            &[PageInput {
                request: PageRequest::new(0, None).unwrap(),
                evidence,
                json: &bytes,
            }],
        )
    }

    fn run(files: Vec<Value>) -> Result<Selection> {
        run_with(
            &metadata(),
            &compatibility(),
            Channel::Stable,
            &evidence(),
            files,
        )
    }

    fn unavailable(result: Result<Selection>, expected: Unavailable) {
        assert!(matches!(
            result,
            Ok(Selection::Unavailable(actual)) if actual == expected
        ));
    }

    #[test]
    fn direct_metadata_deserialization_cannot_select_zero_project() {
        let mut value = mod_json()["data"].clone();
        value["id"] = json!(0);
        let metadata: ModMetadata = serde_json::from_value(value).unwrap();
        let mut checked = evidence();
        checked[0].mod_id = 0;
        let mut item = file(1);
        item["modId"] = json!(0);
        assert!(matches!(
            run_with(
                &metadata,
                &compatibility(),
                Channel::Stable,
                &checked,
                vec![item]
            ),
            Err(Error::InvalidIdentity)
        ));
    }

    #[test]
    fn transport_metadata_bounds_do_not_grant_download_authorization() {
        for size in [0, MAX_FILE_BYTES + 1] {
            let mut item = file(1);
            item["fileLength"] = json!(size);
            assert!(matches!(run(vec![item]), Err(Error::MetadataLimit)));
        }
        for url in [
            "http://fixture.invalid/file",
            "https://user:pass@fixture.invalid/file",
            "file:///tmp/addon",
            "../addon.zip",
        ] {
            let mut item = file(1);
            item["downloadUrl"] = json!(url);
            assert!(matches!(run(vec![item]), Err(Error::MetadataLimit)));
        }
        let mut item = file(1);
        item["fileStatus"] = json!(999);
        // This layer does not interpret moderation status; a candidate is inert.
        assert!(matches!(run(vec![item]), Ok(Selection::Candidate(_))));
    }

    #[test]
    fn positive_envelopes_preserve_explicit_synthetic_binding_and_hashes() {
        let Selection::Candidate(candidate) = run(vec![file(1)]).unwrap() else {
            panic!("Expected synthetic candidate");
        };
        assert_eq!(candidate.file_id, 1);
        assert_eq!(candidate.mod_id, MOD);
        assert_eq!(candidate.game_id, GAME);
        assert_eq!(candidate.compatibility, compatibility());
        assert_eq!(candidate.compatibility.product, Product::Anniversary);
        assert_eq!(candidate.channel, Channel::Stable);
        assert_eq!(candidate.file_length, 123);
        assert_eq!(
            candidate.download_url,
            "https://fixture.invalid/opaque-package"
        );
        assert_eq!(
            candidate.hashes,
            vec![
                ProviderHash::Sha1("a".repeat(40)),
                ProviderHash::Md5("B".repeat(32))
            ]
        );
    }

    #[test]
    fn distribution_is_independent_of_search_visibility_and_file_availability() {
        for (permission, expected) in [
            (Some(json!(true)), Distribution::Allowed),
            (Some(json!(false)), Distribution::Denied),
            (Some(Value::Null), Distribution::Unknown),
            (None, Distribution::Unknown),
        ] {
            let mut value = mod_json();
            let object = value["data"].as_object_mut().unwrap();
            object.insert("isAvailable".into(), json!(false));
            match permission {
                Some(permission) => {
                    object.insert("allowModDistribution".into(), permission);
                }
                None => {
                    object.remove("allowModDistribution");
                }
            }
            let metadata = parse_mod(&serde_json::to_vec(&value).unwrap(), MOD, GAME).unwrap();
            assert!(!metadata.search_visible());
            assert_eq!(metadata.distribution(), expected);
            let result = run_with(
                &metadata,
                &compatibility(),
                Channel::Stable,
                &evidence(),
                vec![file(1)],
            );
            match expected {
                Distribution::Allowed => {
                    assert!(matches!(result, Ok(Selection::Candidate(_))));
                }
                Distribution::Denied => {
                    unavailable(result, Unavailable::DistributionDenied);
                }
                Distribution::Unknown => {
                    unavailable(result, Unavailable::DistributionUnknown);
                }
            }
        }
        let mut value = file(1);
        value["isAvailable"] = json!(false);
        unavailable(run(vec![value]), Unavailable::NoEligibleFile);
    }

    #[test]
    fn numeric_widths_and_expected_identities_fail_closed() {
        for field in ["id", "gameId"] {
            for bad in [json!(-1), json!(4_294_967_296u64), json!("1"), json!(1.5)] {
                let mut value = mod_json();
                value["data"][field] = bad;
                assert_eq!(
                    parse_mod(&serde_json::to_vec(&value).unwrap(), MOD, GAME).unwrap_err(),
                    Error::MalformedJson
                );
            }
        }
        for field in ["id", "gameId"] {
            let mut value = mod_json();
            value["data"][field] = json!(123);
            assert_eq!(
                parse_mod(&serde_json::to_vec(&value).unwrap(), MOD, GAME).unwrap_err(),
                Error::IdentityMismatch
            );
        }
        assert_eq!(
            parse_mod(&serde_json::to_vec(&mod_json()).unwrap(), 0, GAME).unwrap_err(),
            Error::InvalidIdentity
        );
        for field in ["modId", "gameId"] {
            let mut value = file(1);
            value[field] = json!(123);
            assert!(matches!(run(vec![value]), Err(Error::IdentityMismatch)));
        }
        let mut value = file(0);
        assert!(matches!(
            run(vec![value.clone()]),
            Err(Error::InvalidIdentity)
        ));
        value["id"] = json!(-1);
        assert!(matches!(run(vec![value]), Err(Error::MalformedJson)));
        for field in ["id", "gameId", "modId", "releaseType"] {
            let mut value = file(1);
            value[field] = json!(4_294_967_296u64);
            assert!(matches!(run(vec![value]), Err(Error::MalformedJson)));
        }
        let mut value = file(1);
        value["fileLength"] = json!(-1);
        assert!(matches!(run(vec![value]), Err(Error::MalformedJson)));
    }

    #[test]
    fn exact_known_channels_only_and_competing_files_are_rejected() {
        for channel in [Channel::Stable, Channel::Beta, Channel::Alpha] {
            let mut value = file(1);
            value["releaseType"] = json!(channel.number());
            assert!(matches!(
                run_with(
                    &metadata(),
                    &compatibility(),
                    channel,
                    &evidence(),
                    vec![value]
                ),
                Ok(Selection::Candidate(_))
            ));
        }
        for release_type in [0, 2, 3, 99, u32::MAX] {
            let mut value = file(1);
            value["releaseType"] = json!(release_type);
            unavailable(run(vec![value]), Unavailable::NoEligibleFile);
        }
        for files in [vec![file(1), file(2)], vec![file(2), file(1)]] {
            unavailable(run(files), Unavailable::AmbiguousFiles);
        }
        let mut value = file(2);
        value["downloadUrl"] = Value::Null;
        unavailable(run(vec![file(1), value]), Unavailable::AmbiguousFiles);
    }

    #[test]
    fn evidence_must_be_unique_exact_and_not_invented_from_file_fields() {
        for (checked, expected) in [
            (vec![], Error::MissingVersionEvidence),
            (
                vec![evidence()[0].clone(), evidence()[0].clone()],
                Error::AmbiguousVersionEvidence,
            ),
        ] {
            let mut value = file(1);
            value["gameVersionTypeIds"] = json!([TYPE]);
            assert!(matches!(
                run_with(
                    &metadata(),
                    &compatibility(),
                    Channel::Stable,
                    &checked,
                    vec![value]
                ),
                Err(actual) if actual == expected
            ));
        }
        for field in 0..4 {
            let mut checked = evidence();
            match field {
                0 => checked[0].mod_id += 1,
                1 => checked[0].game_id += 1,
                2 => checked[0].version_type_id += 1,
                _ => checked[0].game_version = "other-version".into(),
            }
            assert!(matches!(
                run_with(
                    &metadata(),
                    &compatibility(),
                    Channel::Stable,
                    &checked,
                    vec![file(1)]
                ),
                Err(Error::VersionEvidenceMismatch)
            ));
        }
        let mut context = compatibility();
        context.game_id += 1;
        assert!(matches!(
            run_with(
                &metadata(),
                &context,
                Channel::Stable,
                &evidence(),
                vec![file(1)]
            ),
            Err(Error::IdentityMismatch)
        ));
        context = compatibility();
        context.version_type_id = 0;
        assert!(matches!(
            run_with(
                &metadata(),
                &context,
                Channel::Stable,
                &evidence(),
                vec![file(1)]
            ),
            Err(Error::InvalidIdentity)
        ));
        for versions in [json!([]), json!(["wrong-version"])] {
            let mut value = file(1);
            value["gameVersions"] = versions;
            // Even the apparently flavor-specific filename proves nothing.
            unavailable(run(vec![value]), Unavailable::NoEligibleFile);
        }
    }

    #[test]
    fn missing_url_is_never_derived_and_unknown_hashes_stay_unsupported() {
        for missing in [false, true] {
            let mut value = file(1);
            if missing {
                value.as_object_mut().unwrap().remove("downloadUrl");
            } else {
                value["downloadUrl"] = Value::Null;
            }
            unavailable(run(vec![value]), Unavailable::MissingDownloadUrl);
        }
        let mut value = file(1);
        value["hashes"] = json!([{"algo": 77, "value": "opaque-provider-value"}]);
        let Selection::Candidate(candidate) = run(vec![value]).unwrap() else {
            panic!("Expected inert metadata");
        };
        assert_eq!(
            candidate.hashes,
            vec![ProviderHash::Unsupported {
                algorithm: 77,
                value: "opaque-provider-value".into()
            }]
        );
        let mut value = file(1);
        value["hashes"] = json!([]);
        assert!(matches!(run(vec![value]), Ok(Selection::Candidate(_))));
    }

    #[test]
    fn hashes_reject_invalid_lengths_formats_algorithms_and_duplicates() {
        for (algorithm, digest) in [
            (1, "a".repeat(39)),
            (1, "a".repeat(41)),
            (1, "g".repeat(40)),
            (1, format!("sha1:{}", "a".repeat(40))),
            (2, "a".repeat(31)),
            (2, "a".repeat(33)),
            (2, "z".repeat(32)),
            (2, "a".repeat(64)),
        ] {
            let mut value = file(1);
            value["hashes"] = json!([{"algo": algorithm, "value": digest}]);
            assert!(matches!(run(vec![value]), Err(Error::InvalidHash)));
        }
        for algorithm in [json!(-1), json!(4_294_967_296u64)] {
            let mut value = file(1);
            value["hashes"] = json!([{"algo": algorithm, "value": "opaque"}]);
            assert!(matches!(run(vec![value]), Err(Error::MalformedJson)));
        }
        let mut value = file(1);
        value["hashes"] = json!([
            {"algo": 1, "value": "a".repeat(40)},
            {"algo": 1, "value": "b".repeat(40)}
        ]);
        assert!(matches!(
            run(vec![value]),
            Err(Error::DuplicateHashAlgorithm)
        ));
    }

    #[test]
    fn request_defaults_and_documented_boundaries_are_checked() {
        assert_eq!(PageRequest::new(0, None).unwrap().page_size(), 50);
        assert_eq!(PageRequest::new(0, Some(1)).unwrap().index(), 0);
        assert!(PageRequest::new(9950, Some(50)).is_ok());
        assert!(PageRequest::new(9999, Some(1)).is_ok());
        for (index, size) in [(0, 0), (0, 51), (9951, 50), (10_000, 1), (u32::MAX, 1)] {
            assert_eq!(
                PageRequest::new(index, Some(size)).unwrap_err(),
                Error::InvalidRequest
            );
        }
    }

    #[test]
    fn complete_multiple_pages_work_but_partial_duplicate_and_reordered_fail() {
        let mut first_file = file(1);
        first_file["isAvailable"] = json!(false);
        let first = page(vec![first_file], 0, 1, 2);
        let second = page(vec![file(2)], 1, 1, 2);
        let duplicate = page(vec![file(1)], 1, 1, 2);
        let changed_total = page(vec![file(2)], 1, 1, 3);
        let checked = evidence();
        let input = |index, bytes| PageInput {
            request: PageRequest::new(index, Some(1)).unwrap(),
            evidence: checked.as_slice(),
            json: bytes,
        };
        let evaluate =
            |pages: &[PageInput<'_>]| select(&metadata(), &compatibility(), Channel::Stable, pages);
        assert!(matches!(
            evaluate(&[input(0, &first), input(1, &second)]),
            Ok(Selection::Candidate(candidate)) if candidate.file_id == 2
        ));
        assert!(matches!(
            evaluate(&[input(0, &first)]),
            Err(Error::IncompletePages)
        ));
        assert!(matches!(
            evaluate(&[input(0, &first), input(1, &duplicate)]),
            Err(Error::DuplicateFile)
        ));
        assert!(matches!(
            evaluate(&[input(0, &first), input(1, &changed_total)]),
            Err(Error::InconsistentPagination)
        ));
        assert!(matches!(
            evaluate(&[input(1, &second), input(0, &first)]),
            Err(Error::InconsistentPagination)
        ));
        assert!(matches!(
            evaluate(&[input(0, &first), input(0, &first)]),
            Err(Error::InconsistentPagination)
        ));
        let missing_evidence = PageInput {
            request: PageRequest::new(1, Some(1)).unwrap(),
            evidence: &[],
            json: &second,
        };
        assert!(matches!(
            evaluate(&[input(0, &first), missing_evidence]),
            Err(Error::MissingVersionEvidence)
        ));
        assert!(matches!(evaluate(&[]), Err(Error::IncompletePages)));
        unavailable(run(vec![]), Unavailable::NoEligibleFile);
    }

    #[test]
    fn pagination_truncation_inconsistency_and_extra_pages_fail_closed() {
        let checked = evidence();
        let evaluate = |bytes: &[u8]| {
            select(
                &metadata(),
                &compatibility(),
                Channel::Stable,
                &[PageInput {
                    request: PageRequest::new(0, None).unwrap(),
                    evidence: &checked,
                    json: bytes,
                }],
            )
        };
        let base: Value = serde_json::from_slice(&page(vec![file(1)], 0, 50, 1)).unwrap();
        for (field, bad) in [
            ("index", json!(1)),
            ("pageSize", json!(49)),
            ("resultCount", json!(0)),
            ("resultCount", json!(51)),
            ("totalCount", json!(0)),
            ("totalCount", json!(2)),
            ("totalCount", json!(10_001)),
            ("index", json!(-1)),
            ("totalCount", json!(4_294_967_296u64)),
        ] {
            let mut value = base.clone();
            value["pagination"][field] = bad;
            assert!(evaluate(&serde_json::to_vec(&value).unwrap()).is_err());
        }
        let mut value = base;
        value.as_object_mut().unwrap().remove("pagination");
        assert!(matches!(
            evaluate(&serde_json::to_vec(&value).unwrap()),
            Err(Error::MalformedJson)
        ));
        let bytes = page(vec![], 0, 50, 0);
        let extra = page(vec![], 0, 50, 0);
        assert!(select(
            &metadata(),
            &compatibility(),
            Channel::Stable,
            &[
                PageInput {
                    request: PageRequest::new(0, None).unwrap(),
                    evidence: &checked,
                    json: &bytes,
                },
                PageInput {
                    request: PageRequest::new(0, None).unwrap(),
                    evidence: &checked,
                    json: &extra,
                }
            ]
        )
        .is_err());
    }

    #[test]
    fn malformed_oversized_and_excessive_metadata_are_rejected() {
        for bytes in [
            b"".as_slice(),
            b"{",
            b"[]",
            b"{\"data\":null}",
            b"{\"data\":{}}",
            b"{\"data\":{},\"data\":{}}",
            b"\xff",
        ] {
            assert_eq!(
                parse_mod(bytes, MOD, GAME).unwrap_err(),
                Error::MalformedJson
            );
        }
        let oversized = vec![b' '; MAX_JSON_BYTES + 1];
        assert_eq!(
            parse_mod(&oversized, MOD, GAME).unwrap_err(),
            Error::OversizedJson
        );
        let checked = evidence();
        assert!(matches!(
            select(
                &metadata(),
                &compatibility(),
                Channel::Stable,
                &[PageInput {
                    request: PageRequest::new(0, None).unwrap(),
                    evidence: &checked,
                    json: &oversized,
                }]
            ),
            Err(Error::OversizedJson)
        ));
        let mut truncated = page(vec![file(1)], 0, 50, 1);
        truncated.pop();
        assert!(matches!(
            select(
                &metadata(),
                &compatibility(),
                Channel::Stable,
                &[PageInput {
                    request: PageRequest::new(0, None).unwrap(),
                    evidence: &checked,
                    json: &truncated,
                }]
            ),
            Err(Error::MalformedJson)
        ));
        for field in ["displayName", "fileName", "downloadUrl"] {
            let mut value = file(1);
            value[field] = json!("x".repeat(MAX_URL_BYTES + 1));
            assert!(matches!(run(vec![value]), Err(Error::MetadataLimit)));
        }
        let mut value = file(1);
        value["displayName"] = json!("fixture\ninjected");
        assert!(matches!(run(vec![value]), Err(Error::MetadataLimit)));
        let mut value = file(1);
        value["gameVersions"] = json!(vec!["v"; MAX_VERSIONS + 1]);
        assert!(matches!(run(vec![value]), Err(Error::MetadataLimit)));
        let mut value = file(1);
        value["gameVersions"] = json!(["fixture-version-1", "fixture-version-1"]);
        assert!(matches!(run(vec![value]), Err(Error::MetadataLimit)));
        let mut value = file(1);
        value["hashes"] = json!([{
            "algo": 77,
            "value": "x".repeat(MAX_HASH_BYTES + 1)
        }]);
        assert!(matches!(run(vec![value]), Err(Error::MetadataLimit)));
        let mut value = file(1);
        value["hashes"] = json!(vec![json!({"algo": 77, "value": "opaque"}); MAX_HASHES + 1]);
        assert!(matches!(run(vec![value]), Err(Error::MetadataLimit)));
    }

    #[test]
    fn batch_budgets_are_checked_before_deserialization() {
        let checked = evidence();
        let small = b"{}";
        let too_many: Vec<_> = (0..=MAX_PAGES)
            .map(|_| PageInput {
                request: PageRequest::new(0, None).unwrap(),
                evidence: &checked,
                json: small,
            })
            .collect();
        assert!(matches!(
            select(&metadata(), &compatibility(), Channel::Stable, &too_many),
            Err(Error::TooManyPages)
        ));
        let full = vec![b' '; MAX_JSON_BYTES];
        let too_large: Vec<_> = (0..17)
            .map(|_| PageInput {
                request: PageRequest::new(0, None).unwrap(),
                evidence: &checked,
                json: &full,
            })
            .collect();
        assert!(matches!(
            select(&metadata(), &compatibility(), Channel::Stable, &too_large),
            Err(Error::BatchLimit)
        ));
    }

    #[test]
    fn maximum_file_count_and_page_count_are_supported() {
        let checked = evidence();
        let bodies: Vec<_> = (0..MAX_PAGES)
            .map(|position| {
                let index = position as u32 * MAX_PAGE_SIZE;
                let files = (1..=MAX_PAGE_SIZE)
                    .map(|offset| {
                        let mut value = file(index + offset);
                        value["isAvailable"] = json!(false);
                        value
                    })
                    .collect();
                page(files, index, MAX_PAGE_SIZE, MAX_FILES)
            })
            .collect();
        let pages: Vec<_> = bodies
            .iter()
            .enumerate()
            .map(|(position, bytes)| PageInput {
                request: PageRequest::new(position as u32 * MAX_PAGE_SIZE, None).unwrap(),
                evidence: &checked,
                json: bytes,
            })
            .collect();
        unavailable(
            select(&metadata(), &compatibility(), Channel::Stable, &pages),
            Unavailable::NoEligibleFile,
        );
    }
}
