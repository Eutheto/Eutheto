use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt::{self, Write as _};
use std::fs;
use std::io::{self, Read as _};
use std::path::{Path, PathBuf};

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const CARGO_LOCK: &str = "Cargo.lock";
const PNPM_LOCK: &str = "pnpm-lock.yaml";
const REVIEWED_INPUTS: &str = "xtask/supply-chain-inputs.json";
const LICENSE_INVENTORY: &str = "xtask/generated/license-inventory.json";
const NOTICE: &str = "THIRD_PARTY_NOTICES.md";
const SBOM: &str = "xtask/generated/sbom.spdx.json";
const NOASSERTION: &str = "NOASSERTION";
const MAX_EXTRACTED_LICENSE_BYTES: usize = 128 * 1024;

pub type SupplyChainResult<T> = Result<T, SupplyChainError>;

#[derive(Debug)]
pub enum SupplyChainError {
    MissingInput {
        path: PathBuf,
    },
    ReadInput {
        path: PathBuf,
        source: io::Error,
    },
    MalformedLock {
        path: PathBuf,
        detail: String,
    },
    MalformedReviewedInput {
        path: PathBuf,
        detail: String,
    },
    StaleReviewedInput {
        detail: String,
    },
    Render {
        artifact: &'static str,
        detail: String,
    },
    WriteArtifact {
        path: PathBuf,
        source: io::Error,
    },
    Drift {
        paths: Vec<&'static str>,
    },
}

impl fmt::Display for SupplyChainError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingInput { path } => {
                write!(
                    formatter,
                    "required supply-chain input is missing: {}",
                    path.display()
                )
            }
            Self::ReadInput { path, source } => {
                write!(formatter, "failed to read {}: {source}", path.display())
            }
            Self::MalformedLock { path, detail } => {
                write!(
                    formatter,
                    "malformed lock file {}: {detail}",
                    path.display()
                )
            }
            Self::MalformedReviewedInput { path, detail } => {
                write!(
                    formatter,
                    "malformed reviewed input {}: {detail}",
                    path.display()
                )
            }
            Self::StaleReviewedInput { detail } => {
                write!(
                    formatter,
                    "reviewed supply-chain input does not match the locks: {detail}"
                )
            }
            Self::Render { artifact, detail } => {
                write!(formatter, "failed to render {artifact}: {detail}")
            }
            Self::WriteArtifact { path, source } => {
                write!(formatter, "failed to write {}: {source}", path.display())
            }
            Self::Drift { paths } => write!(
                formatter,
                "supply-chain artifacts are missing or stale: {}; run `cargo xtask licenses generate` and `cargo xtask sbom generate`",
                paths.join(", ")
            ),
        }
    }
}

impl Error for SupplyChainError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::ReadInput { source, .. } | Self::WriteArtifact { source, .. } => Some(source),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReviewedInputs {
    schema_version: u32,
    document_name: String,
    document_namespace: String,
    created: String,
    workspace_packages: Vec<ReviewedWorkspacePackage>,
    #[serde(default)]
    reviewed_vendored_packages: Vec<ReviewedVendoredPackage>,
    license_conclusions: Vec<ReviewedLicense>,
    #[serde(default)]
    reviewed_assets: Vec<ReviewedAsset>,
    #[serde(default)]
    extracted_licenses: Vec<ReviewedExtractedLicense>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReviewedExtractedLicense {
    license_id: String,
    name: String,
    source: String,
    file: ReviewedAssetFile,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReviewedWorkspacePackage {
    ecosystem: String,
    name: String,
    version: String,
    path: String,
    license: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReviewedVendoredPackage {
    ecosystem: String,
    name: String,
    version: String,
    path: String,
    license: String,
    source: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReviewedLicense {
    ecosystem: String,
    name: String,
    version: String,
    license: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReviewedAsset {
    name: String,
    version: String,
    source: String,
    license_concluded: String,
    files: Vec<ReviewedAssetFile>,
    notices: Vec<ReviewedAssetFile>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReviewedAssetFile {
    path: String,
    sha256: String,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct PackageKey {
    ecosystem: String,
    name: String,
    version: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PnpmPackage {
    key: PackageKey,
    checksum: PackageChecksum,
    source: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct PackageChecksum {
    algorithm: ChecksumAlgorithm,
    value: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
enum ChecksumAlgorithm {
    #[serde(rename = "SHA256")]
    Sha256,
    #[serde(rename = "SHA512")]
    Sha512,
}

#[derive(Clone, Debug)]
struct CargoPackage {
    key: PackageKey,
    source: Option<String>,
    checksum: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct InventoryPackage {
    ecosystem: String,
    name: String,
    version: String,
    kind: String,
    license_concluded: String,
    source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    checksum: Option<PackageChecksum>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct LicenseInventory<'a> {
    schema_version: u32,
    generated_by: &'static str,
    authoritative_inputs: [&'static str; 3],
    packages: &'a [InventoryPackage],
    #[serde(skip_serializing_if = "<[ReviewedAsset]>::is_empty")]
    assets: &'a [ReviewedAsset],
}

#[derive(Debug)]
struct Inventory {
    reviewed: ReviewedInputs,
    packages: Vec<InventoryPackage>,
    extracted_licensing_infos: Vec<SpdxExtractedLicense>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SpdxDocument<'a> {
    spdx_version: &'static str,
    data_license: &'static str,
    #[serde(rename = "SPDXID")]
    spdx_id: &'static str,
    name: &'a str,
    document_namespace: &'a str,
    creation_info: SpdxCreationInfo<'a>,
    packages: Vec<SpdxPackage>,
    relationships: Vec<SpdxRelationship>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    files: Vec<SpdxFile>,
    #[serde(
        rename = "hasExtractedLicensingInfos",
        skip_serializing_if = "<[SpdxExtractedLicense]>::is_empty"
    )]
    extracted_licensing_infos: &'a [SpdxExtractedLicense],
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SpdxExtractedLicense {
    license_id: String,
    name: String,
    extracted_text: String,
    see_alsos: [String; 1],
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SpdxCreationInfo<'a> {
    created: &'a str,
    creators: [&'static str; 1],
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SpdxPackage {
    #[serde(rename = "SPDXID")]
    spdx_id: String,
    name: String,
    version_info: String,
    download_location: String,
    files_analyzed: bool,
    license_concluded: String,
    license_declared: String,
    copyright_text: &'static str,
    external_refs: [SpdxExternalRef; 1],
    #[serde(skip_serializing_if = "Vec::is_empty")]
    checksums: Vec<SpdxChecksum>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SpdxExternalRef {
    #[serde(rename = "referenceCategory")]
    category: &'static str,
    #[serde(rename = "referenceType")]
    kind: &'static str,
    #[serde(rename = "referenceLocator")]
    locator: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SpdxChecksum {
    algorithm: &'static str,
    checksum_value: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SpdxFile {
    #[serde(rename = "SPDXID")]
    spdx_id: String,
    file_name: String,
    checksums: Vec<SpdxChecksum>,
    license_concluded: String,
    license_info_in_files: Vec<String>,
    copyright_text: &'static str,
    comment: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SpdxRelationship {
    spdx_element_id: &'static str,
    relationship_type: &'static str,
    related_spdx_element: String,
}

pub fn generate_licenses(repo_root: &Path) -> SupplyChainResult<()> {
    let inventory = load_inventory(repo_root)?;
    let notice = render_notice(&inventory.packages, &inventory.reviewed.reviewed_assets)?;
    let inventory_json =
        render_inventory(&inventory.packages, &inventory.reviewed.reviewed_assets)?;
    write_if_changed(repo_root, NOTICE, &notice)?;
    write_if_changed(repo_root, LICENSE_INVENTORY, &inventory_json)?;
    println!(
        "generated license inventory for {} workspace package(s) and {} locked dependency package(s)",
        inventory
            .packages
            .iter()
            .filter(|package| package.kind == "workspace")
            .count(),
        inventory
            .packages
            .iter()
            .filter(|package| package.kind == "dependency")
            .count()
    );
    Ok(())
}

pub fn generate_sbom(repo_root: &Path) -> SupplyChainResult<()> {
    let inventory = load_inventory(repo_root)?;
    let sbom = render_sbom(&inventory)?;
    write_if_changed(repo_root, SBOM, &sbom)?;
    println!(
        "generated SPDX-JSON SBOM for {} package(s)",
        inventory.packages.len()
    );
    Ok(())
}

pub fn check_licenses(repo_root: &Path) -> SupplyChainResult<()> {
    let inventory = load_inventory(repo_root)?;
    let expected_notice = render_notice(&inventory.packages, &inventory.reviewed.reviewed_assets)?;
    let expected_inventory =
        render_inventory(&inventory.packages, &inventory.reviewed.reviewed_assets)?;
    check_outputs(
        repo_root,
        &[
            (NOTICE, expected_notice),
            (LICENSE_INVENTORY, expected_inventory),
        ],
    )
}

pub fn check_sbom(repo_root: &Path) -> SupplyChainResult<()> {
    let inventory = load_inventory(repo_root)?;
    let expected = render_sbom(&inventory)?;
    check_outputs(repo_root, &[(SBOM, expected)])
}

#[allow(
    clippy::too_many_lines,
    reason = "inventory reconciliation is one order-sensitive validation pass across both lockfiles"
)]
fn load_inventory(repo_root: &Path) -> SupplyChainResult<Inventory> {
    let reviewed = read_reviewed_inputs(repo_root)?;
    let extracted_licensing_infos = load_extracted_licenses(repo_root, &reviewed)?;
    validate_assets(repo_root, &reviewed.reviewed_assets)?;
    let cargo_packages = parse_cargo_lock(repo_root)?;
    let (pnpm_packages, pnpm_importers) = parse_pnpm_lock(repo_root)?;

    let mut workspace = BTreeMap::new();
    for package in &reviewed.workspace_packages {
        validate_reviewed_workspace(package, repo_root)?;
        let key = PackageKey {
            ecosystem: package.ecosystem.clone(),
            name: package.name.clone(),
            version: package.version.clone(),
        };
        if workspace.insert(key, package).is_some() {
            return Err(SupplyChainError::StaleReviewedInput {
                detail: format!(
                    "duplicate workspace package {}:{}@{}",
                    package.ecosystem, package.name, package.version
                ),
            });
        }
    }

    let mut vendored = BTreeMap::new();
    for package in &reviewed.reviewed_vendored_packages {
        validate_reviewed_vendored(package, repo_root, &reviewed.reviewed_assets)?;
        let key = PackageKey {
            ecosystem: package.ecosystem.clone(),
            name: package.name.clone(),
            version: package.version.clone(),
        };
        if workspace.contains_key(&key) || vendored.insert(key, package).is_some() {
            return Err(asset_error("duplicate reviewed local package identity"));
        }
    }

    let mut conclusions = BTreeMap::new();
    for conclusion in &reviewed.license_conclusions {
        validate_ecosystem(&conclusion.ecosystem, "license conclusion")?;
        require_nonempty(&conclusion.name, "license conclusion name")?;
        require_nonempty(&conclusion.version, "license conclusion version")?;
        require_nonempty(&conclusion.license, "license conclusion SPDX expression")?;
        let key = PackageKey {
            ecosystem: conclusion.ecosystem.clone(),
            name: conclusion.name.clone(),
            version: conclusion.version.clone(),
        };
        if conclusions
            .insert(key, conclusion.license.clone())
            .is_some()
        {
            return Err(SupplyChainError::StaleReviewedInput {
                detail: format!(
                    "duplicate license conclusion {}:{}@{}",
                    conclusion.ecosystem, conclusion.name, conclusion.version
                ),
            });
        }
    }

    let mut packages = BTreeMap::<PackageKey, InventoryPackage>::new();
    let mut seen_workspace = BTreeSet::new();
    let mut seen_vendored = BTreeSet::new();
    for package in cargo_packages {
        let (kind, license, source) = match &package.source {
            None if vendored.contains_key(&package.key) => {
                let reviewed_package = vendored[&package.key];
                seen_vendored.insert(package.key.clone());
                (
                    "dependency".to_owned(),
                    reviewed_package.license.clone(),
                    reviewed_package.source.clone(),
                )
            }
            None => {
                let reviewed_package = workspace.get(&package.key).ok_or_else(|| {
                    SupplyChainError::StaleReviewedInput {
                        detail: format!(
                            "Cargo workspace package {}@{} is absent from {REVIEWED_INPUTS}",
                            package.key.name, package.key.version
                        ),
                    }
                })?;
                seen_workspace.insert(package.key.clone());
                (
                    "workspace".to_owned(),
                    reviewed_package.license.clone(),
                    reviewed_package.path.clone(),
                )
            }
            Some(source) => (
                "dependency".to_owned(),
                conclusions
                    .get(&package.key)
                    .cloned()
                    .unwrap_or_else(|| NOASSERTION.to_owned()),
                source.clone(),
            ),
        };
        insert_package(
            &mut packages,
            InventoryPackage {
                ecosystem: package.key.ecosystem.clone(),
                name: package.key.name.clone(),
                version: package.key.version.clone(),
                kind,
                license_concluded: license,
                source,
                checksum: package.checksum.map(|value| PackageChecksum {
                    algorithm: ChecksumAlgorithm::Sha256,
                    value,
                }),
            },
        )?;
    }

    for package in workspace
        .values()
        .filter(|package| package.ecosystem == "npm")
    {
        if !pnpm_importers.contains(&package.path) {
            return Err(SupplyChainError::StaleReviewedInput {
                detail: format!(
                    "npm workspace package {} points to importer `{}` absent from {PNPM_LOCK}",
                    package.name, package.path
                ),
            });
        }
        let key = PackageKey {
            ecosystem: package.ecosystem.clone(),
            name: package.name.clone(),
            version: package.version.clone(),
        };
        seen_workspace.insert(key.clone());
        insert_package(
            &mut packages,
            InventoryPackage {
                ecosystem: key.ecosystem,
                name: key.name,
                version: key.version,
                kind: "workspace".to_owned(),
                license_concluded: package.license.clone(),
                source: package.path.clone(),
                checksum: None,
            },
        )?;
    }

    let reviewed_npm_paths: BTreeSet<&str> = workspace
        .values()
        .filter(|package| package.ecosystem == "npm")
        .map(|package| package.path.as_str())
        .collect();
    for importer in &pnpm_importers {
        if !reviewed_npm_paths.contains(importer.as_str()) {
            return Err(SupplyChainError::StaleReviewedInput {
                detail: format!(
                    "pnpm importer `{importer}` is absent from the reviewed workspace package list"
                ),
            });
        }
    }

    for package in pnpm_packages {
        insert_package(
            &mut packages,
            InventoryPackage {
                ecosystem: package.key.ecosystem.clone(),
                name: package.key.name.clone(),
                version: package.key.version.clone(),
                kind: "dependency".to_owned(),
                license_concluded: conclusions
                    .get(&package.key)
                    .cloned()
                    .unwrap_or_else(|| NOASSERTION.to_owned()),
                source: package.source,
                checksum: Some(package.checksum),
            },
        )?;
    }

    for key in workspace.keys() {
        if !seen_workspace.contains(key) {
            return Err(SupplyChainError::StaleReviewedInput {
                detail: format!(
                    "reviewed workspace package {}:{}@{} is absent from its committed lock",
                    key.ecosystem, key.name, key.version
                ),
            });
        }
    }
    for key in vendored.keys() {
        if !seen_vendored.contains(key) {
            return Err(asset_error(
                "reviewed vendored package is absent from source-less Cargo lock entries",
            ));
        }
    }
    for key in conclusions.keys() {
        let is_dependency = packages
            .get(key)
            .is_some_and(|package| package.kind == "dependency");
        if !is_dependency {
            return Err(SupplyChainError::StaleReviewedInput {
                detail: format!(
                    "license conclusion {}:{}@{} does not match a locked dependency",
                    key.ecosystem, key.name, key.version
                ),
            });
        }
    }

    Ok(Inventory {
        reviewed,
        packages: packages.into_values().collect(),
        extracted_licensing_infos,
    })
}

fn valid_license_ref(value: &str) -> bool {
    value.strip_prefix("LicenseRef-").is_some_and(|suffix| {
        !suffix.is_empty()
            && suffix
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-'))
    })
}

fn load_extracted_licenses(
    repo_root: &Path,
    reviewed: &ReviewedInputs,
) -> SupplyChainResult<Vec<SpdxExtractedLicense>> {
    let expressions = reviewed
        .workspace_packages
        .iter()
        .map(|package| package.license.as_str())
        .chain(
            reviewed
                .license_conclusions
                .iter()
                .map(|license| license.license.as_str()),
        )
        .chain(
            reviewed
                .reviewed_assets
                .iter()
                .map(|asset| asset.license_concluded.as_str()),
        );
    let mut referenced = BTreeSet::new();
    for expression in expressions {
        for token in expression.split(|character: char| {
            character.is_ascii_whitespace() || matches!(character, '(' | ')')
        }) {
            if token.contains("LicenseRef-") {
                if !valid_license_ref(token) {
                    return Err(asset_error("invalid referenced LicenseRef identifier"));
                }
                referenced.insert(token);
            }
        }
    }
    let mut loaded = BTreeMap::new();
    for license in &reviewed.extracted_licenses {
        if !valid_license_ref(&license.license_id)
            || !license.source.starts_with("https://")
            || loaded.contains_key(&license.license_id)
        {
            return Err(asset_error("invalid or duplicate extracted license"));
        }
        require_nonempty(&license.name, "extracted license name")?;
        if !referenced.contains(license.license_id.as_str()) {
            return Err(asset_error("extracted license is not referenced"));
        }
        let bytes = read_asset_file(repo_root, &license.file, Some(MAX_EXTRACTED_LICENSE_BYTES))?;
        let text = String::from_utf8(bytes)
            .map_err(|_| asset_error("extracted license text must be UTF-8"))?;
        require_nonempty(&text, "extracted license text")?;
        loaded.insert(
            license.license_id.clone(),
            SpdxExtractedLicense {
                license_id: license.license_id.clone(),
                name: license.name.clone(),
                extracted_text: text,
                see_alsos: [license.source.clone()],
            },
        );
    }
    if referenced.iter().any(|id| !loaded.contains_key(*id)) {
        return Err(asset_error(
            "referenced LicenseRef has no extracted license text",
        ));
    }
    Ok(loaded.into_values().collect())
}

fn validate_assets(repo_root: &Path, assets: &[ReviewedAsset]) -> SupplyChainResult<()> {
    let mut identities = BTreeSet::new();
    let mut files = BTreeSet::new();
    for asset in assets {
        for value in [&asset.name, &asset.version, &asset.license_concluded] {
            require_nonempty(value, "reviewed asset metadata")?;
        }
        if !asset.source.starts_with("https://")
            || asset.files.is_empty()
            || asset.notices.is_empty()
            || !identities.insert((&asset.name, &asset.version))
        {
            return Err(asset_error("invalid or duplicate reviewed asset"));
        }
        for file in asset.files.iter().chain(&asset.notices) {
            if asset.files.iter().any(|font| font.path == file.path) && !files.insert(&file.path) {
                return Err(asset_error("duplicate reviewed asset file"));
            }
            validate_asset_file(repo_root, file)?;
        }
    }
    Ok(())
}

fn asset_error(detail: &str) -> SupplyChainError {
    SupplyChainError::MalformedReviewedInput {
        path: PathBuf::from(REVIEWED_INPUTS),
        detail: detail.to_owned(),
    }
}

fn validate_asset_file(repo_root: &Path, file: &ReviewedAssetFile) -> SupplyChainResult<()> {
    read_asset_file(repo_root, file, None).map(|_| ())
}

fn read_asset_file(
    repo_root: &Path,
    file: &ReviewedAssetFile,
    max_bytes: Option<usize>,
) -> SupplyChainResult<Vec<u8>> {
    if file.path.is_empty()
        || file.path.contains('\\')
        || file
            .path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == ".." || part.contains(':'))
        || file.sha256.len() != 64
        || !file
            .sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(asset_error(
            "asset requires a clean relative path and lowercase SHA256",
        ));
    }
    let mut path = repo_root.to_path_buf();
    for part in file.path.split('/') {
        path.push(part);
        let metadata =
            fs::symlink_metadata(&path).map_err(|source| SupplyChainError::ReadInput {
                path: path.clone(),
                source,
            })?;
        if metadata.file_type().is_symlink() || (!metadata.is_dir() && !metadata.is_file()) {
            return Err(asset_error(
                "reviewed asset paths must not traverse symlinks",
            ));
        }
    }
    if !path.is_file() {
        return Err(asset_error("reviewed asset must be a regular file"));
    }
    let mut bytes = Vec::new();
    fs::File::open(&path)
        .and_then(|input| {
            input
                .take(max_bytes.map_or(u64::MAX, |limit| limit.saturating_add(1) as u64))
                .read_to_end(&mut bytes)
        })
        .map_err(|source| SupplyChainError::ReadInput { path, source })?;
    if max_bytes.is_some_and(|limit| bytes.len() > limit) {
        return Err(asset_error("extracted license text exceeds its byte limit"));
    }
    if format!("{:x}", Sha256::digest(&bytes)) != file.sha256 {
        return Err(asset_error(&format!(
            "reviewed asset digest changed: {}",
            file.path
        )));
    }
    Ok(bytes)
}

fn read_reviewed_inputs(repo_root: &Path) -> SupplyChainResult<ReviewedInputs> {
    let path = repo_root.join(REVIEWED_INPUTS);
    let contents = read_required(&path)?;
    let inputs: ReviewedInputs = serde_json::from_str(&contents).map_err(|error| {
        SupplyChainError::MalformedReviewedInput {
            path: path.clone(),
            detail: error.to_string(),
        }
    })?;
    if inputs.schema_version != 1 {
        return Err(SupplyChainError::MalformedReviewedInput {
            path,
            detail: format!(
                "unsupported schemaVersion {}; expected 1",
                inputs.schema_version
            ),
        });
    }
    require_nonempty(&inputs.document_name, "documentName")?;
    if !inputs.document_namespace.starts_with("urn:")
        && !inputs.document_namespace.starts_with("https://")
    {
        return Err(SupplyChainError::MalformedReviewedInput {
            path: repo_root.join(REVIEWED_INPUTS),
            detail: "documentNamespace must be an absolute urn: or https: URI".to_owned(),
        });
    }
    if !looks_like_utc_timestamp(&inputs.created) {
        return Err(SupplyChainError::MalformedReviewedInput {
            path: repo_root.join(REVIEWED_INPUTS),
            detail: "created must be a fixed UTC timestamp in YYYY-MM-DDTHH:MM:SSZ form".to_owned(),
        });
    }
    Ok(inputs)
}

fn validate_reviewed_vendored(
    package: &ReviewedVendoredPackage,
    repo_root: &Path,
    assets: &[ReviewedAsset],
) -> SupplyChainResult<()> {
    if package.ecosystem != "cargo" {
        return Err(asset_error(
            "vendored packages must use the cargo ecosystem",
        ));
    }
    require_nonempty(&package.license, "vendored package SPDX license")?;
    let revision = package.source.rsplit('/').next().unwrap_or_default();
    if !package.source.starts_with("https://")
        || revision.len() != 40
        || !revision.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(asset_error(
            "vendored source must identify an exact upstream commit",
        ));
    }
    let manifest_path = format!("{}/Cargo.toml", package.path);
    if !assets
        .iter()
        .flat_map(|asset| &asset.files)
        .any(|file| file.path == manifest_path)
    {
        return Err(asset_error(
            "vendored manifest must be a hash-checked reviewed asset",
        ));
    }
    validate_vendored_file_census(repo_root, &package.path, assets)?;
    let manifest = read_required(&repo_root.join(&manifest_path))?;
    let mut package_section = false;
    let mut identity = BTreeMap::new();
    for (index, line) in manifest.lines().enumerate() {
        let line = line.trim();
        if line.starts_with('[') {
            package_section = line == "[package]";
        } else if package_section {
            for field in ["name", "version", "license"] {
                if line
                    .split_once('=')
                    .is_some_and(|(key, _)| key.trim() == field)
                {
                    identity.insert(
                        field,
                        parse_toml_string(
                            assignment_value(line, field, Path::new(&manifest_path), index + 1)?,
                            Path::new(&manifest_path),
                            index + 1,
                        )?,
                    );
                }
            }
        }
    }
    for (field, expected) in [
        ("name", &package.name),
        ("version", &package.version),
        ("license", &package.license),
    ] {
        if identity.get(field) != Some(expected) {
            return Err(asset_error(
                "vendored package identity/license does not match its manifest",
            ));
        }
    }
    Ok(())
}

fn validate_vendored_file_census(
    repo_root: &Path,
    package_path: &str,
    assets: &[ReviewedAsset],
) -> SupplyChainResult<()> {
    let prefix = format!("{package_path}/");
    let expected: BTreeSet<String> = assets
        .iter()
        .flat_map(|asset| asset.files.iter().chain(&asset.notices))
        .filter(|file| file.path.starts_with(&prefix))
        .map(|file| file.path.clone())
        .collect();
    let mut pending = vec![repo_root.join(package_path)];
    let mut actual = BTreeSet::new();
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory).map_err(|source| SupplyChainError::ReadInput {
            path: directory.clone(),
            source,
        })? {
            let path = entry
                .map_err(|source| SupplyChainError::ReadInput {
                    path: directory.clone(),
                    source,
                })?
                .path();
            let metadata =
                fs::symlink_metadata(&path).map_err(|source| SupplyChainError::ReadInput {
                    path: path.clone(),
                    source,
                })?;
            if metadata.is_dir() {
                pending.push(path);
            } else if metadata.is_file() {
                let relative = path
                    .strip_prefix(repo_root)
                    .ok()
                    .and_then(|relative| {
                        relative
                            .iter()
                            .map(|part| part.to_str())
                            .collect::<Option<Vec<_>>>()
                    })
                    .ok_or_else(|| {
                        asset_error("vendored source path must be UTF-8 and relative")
                    })?;
                actual.insert(relative.join("/"));
            } else {
                return Err(asset_error(
                    "vendored source tree must contain only regular files and directories",
                ));
            }
        }
    }
    if actual != expected {
        return Err(asset_error(
            "vendored source tree differs from the reviewed file census",
        ));
    }
    Ok(())
}

fn validate_reviewed_workspace(
    package: &ReviewedWorkspacePackage,
    repo_root: &Path,
) -> SupplyChainResult<()> {
    validate_ecosystem(&package.ecosystem, "workspace package")?;
    require_nonempty(&package.name, "workspace package name")?;
    require_nonempty(&package.version, "workspace package version")?;
    require_nonempty(&package.license, "workspace package SPDX license")?;
    if package.path.is_empty()
        || package.path.starts_with('/')
        || package.path.split('/').any(|component| component == "..")
    {
        return Err(SupplyChainError::MalformedReviewedInput {
            path: repo_root.join(REVIEWED_INPUTS),
            detail: format!(
                "workspace package path is not repository-relative: {}",
                package.path
            ),
        });
    }
    Ok(())
}

fn validate_ecosystem(ecosystem: &str, subject: &str) -> SupplyChainResult<()> {
    if matches!(ecosystem, "cargo" | "npm") {
        Ok(())
    } else {
        Err(SupplyChainError::StaleReviewedInput {
            detail: format!("{subject} has unsupported ecosystem `{ecosystem}`"),
        })
    }
}

fn require_nonempty(value: &str, field: &str) -> SupplyChainResult<()> {
    if value.trim().is_empty() {
        Err(SupplyChainError::StaleReviewedInput {
            detail: format!("{field} must not be empty"),
        })
    } else {
        Ok(())
    }
}

fn parse_cargo_lock(repo_root: &Path) -> SupplyChainResult<Vec<CargoPackage>> {
    let path = repo_root.join(CARGO_LOCK);
    let contents = read_required(&path)?;
    let mut lock_version = None;
    let mut packages = Vec::new();
    let mut current = None;

    for (index, raw_line) in contents.lines().enumerate() {
        let line_number = index + 1;
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line == "[[package]]" {
            if let Some(package) = current.take() {
                packages.push(finish_cargo_package(package, &path, line_number)?);
            }
            current = Some(PartialCargoPackage::default());
            continue;
        }
        if line.starts_with("[[package") {
            return malformed_lock(
                &path,
                format!("line {line_number}: malformed package header"),
            );
        }
        if current.is_none() && line.starts_with("version") {
            let value = assignment_value(line, "version", &path, line_number)?;
            let parsed = value
                .parse::<u32>()
                .map_err(|_| SupplyChainError::MalformedLock {
                    path: path.clone(),
                    detail: format!("line {line_number}: lock version is not an integer"),
                })?;
            if lock_version.replace(parsed).is_some() {
                return malformed_lock(
                    &path,
                    format!("line {line_number}: duplicate top-level lock version"),
                );
            }
            continue;
        }
        if let Some(package) = current.as_mut() {
            if line.starts_with("name") {
                package.name = Some(parse_toml_string(
                    assignment_value(line, "name", &path, line_number)?,
                    &path,
                    line_number,
                )?);
            } else if line.starts_with("version") {
                package.version = Some(parse_toml_string(
                    assignment_value(line, "version", &path, line_number)?,
                    &path,
                    line_number,
                )?);
            } else if line.starts_with("source") {
                package.source = Some(parse_toml_string(
                    assignment_value(line, "source", &path, line_number)?,
                    &path,
                    line_number,
                )?);
            } else if line.starts_with("checksum") {
                let checksum = parse_toml_string(
                    assignment_value(line, "checksum", &path, line_number)?,
                    &path,
                    line_number,
                )?;
                if checksum.len() != 64 || !checksum.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                    return malformed_lock(
                        &path,
                        format!("line {line_number}: checksum is not a 64-digit SHA-256 value"),
                    );
                }
                package.checksum = Some(checksum.to_ascii_lowercase());
            }
        }
    }
    if let Some(package) = current {
        packages.push(finish_cargo_package(
            package,
            &path,
            contents.lines().count() + 1,
        )?);
    }

    match lock_version {
        None => return malformed_lock(&path, "missing top-level lock version".to_owned()),
        Some(4) => {}
        Some(version) => {
            return malformed_lock(
                &path,
                format!("unsupported Cargo lock version `{version}`; expected 4"),
            );
        }
    }
    if packages.is_empty() {
        return malformed_lock(&path, "no [[package]] entries".to_owned());
    }
    packages.sort_by(|left, right| left.key.cmp(&right.key));
    Ok(packages)
}

#[derive(Default)]
struct PartialCargoPackage {
    name: Option<String>,
    version: Option<String>,
    source: Option<String>,
    checksum: Option<String>,
}

fn finish_cargo_package(
    package: PartialCargoPackage,
    path: &Path,
    line_number: usize,
) -> SupplyChainResult<CargoPackage> {
    let name = package
        .name
        .ok_or_else(|| SupplyChainError::MalformedLock {
            path: path.to_path_buf(),
            detail: format!("package ending before line {line_number} has no name"),
        })?;
    let version = package
        .version
        .ok_or_else(|| SupplyChainError::MalformedLock {
            path: path.to_path_buf(),
            detail: format!("package `{name}` ending before line {line_number} has no version"),
        })?;
    Ok(CargoPackage {
        key: PackageKey {
            ecosystem: "cargo".to_owned(),
            name,
            version,
        },
        source: package.source,
        checksum: package.checksum,
    })
}

fn assignment_value<'a>(
    line: &'a str,
    key: &str,
    path: &Path,
    line_number: usize,
) -> SupplyChainResult<&'a str> {
    let (actual_key, value) =
        line.split_once('=')
            .ok_or_else(|| SupplyChainError::MalformedLock {
                path: path.to_path_buf(),
                detail: format!("line {line_number}: expected `{key} = value`"),
            })?;
    if actual_key.trim() != key || value.trim().is_empty() {
        return malformed_lock(
            path,
            format!("line {line_number}: expected `{key} = value`"),
        );
    }
    Ok(value.trim())
}

fn parse_toml_string(value: &str, path: &Path, line_number: usize) -> SupplyChainResult<String> {
    serde_json::from_str::<String>(value).map_err(|error| SupplyChainError::MalformedLock {
        path: path.to_path_buf(),
        detail: format!("line {line_number}: invalid quoted string: {error}"),
    })
}

fn parse_pnpm_lock(repo_root: &Path) -> SupplyChainResult<(Vec<PnpmPackage>, BTreeSet<String>)> {
    let path = repo_root.join(PNPM_LOCK);
    let contents = read_required(&path)?;
    if contents.lines().any(|line| line.contains('\t')) {
        return malformed_lock(
            &path,
            "tab indentation is not valid in the committed pnpm lock".to_owned(),
        );
    }

    let lock_version = contents
        .lines()
        .find_map(|line| line.strip_prefix("lockfileVersion:"))
        .map(str::trim)
        .ok_or_else(|| SupplyChainError::MalformedLock {
            path: path.clone(),
            detail: "missing top-level lockfileVersion".to_owned(),
        })?;
    let lock_version = parse_yaml_scalar(lock_version, &path, 1)?;
    if lock_version != "9.0" {
        return malformed_lock(
            &path,
            format!("unsupported lockfileVersion `{lock_version}`; pnpm 11 must emit 9.0"),
        );
    }

    let importers = parse_yaml_section_keys(&contents, "importers", &path)?;
    if importers.is_empty() {
        return malformed_lock(&path, "the importers section is empty".to_owned());
    }
    let package_entries = parse_yaml_section_keys(&contents, "packages", &path)?;
    if package_entries.is_empty() {
        return malformed_lock(&path, "the packages section is empty".to_owned());
    }

    let packages = parse_pnpm_packages(&contents, package_entries, &path)?;
    Ok((packages, importers.into_keys().collect()))
}

fn parse_pnpm_packages(
    contents: &str,
    package_entries: BTreeMap<String, usize>,
    path: &Path,
) -> SupplyChainResult<Vec<PnpmPackage>> {
    let lines: Vec<_> = contents.lines().collect();
    let mut entries: Vec<_> = package_entries.into_iter().collect();
    entries.sort_by_key(|(_, line_number)| *line_number);
    let section_end = entries.last().map_or(lines.len() + 1, |(_, line_number)| {
        lines
            .iter()
            .enumerate()
            .skip(*line_number)
            .find(|(_, line)| !line.is_empty() && !line.starts_with(' '))
            .map_or(lines.len() + 1, |(index, _)| index + 1)
    });
    let mut packages = BTreeMap::new();

    for (index, (entry, line_number)) in entries.iter().enumerate() {
        let end_line = entries
            .get(index + 1)
            .map_or(section_end, |(_, next_line)| *next_line);
        let key = parse_pnpm_package_key(entry, path, *line_number)?;
        let (checksum, tarball) =
            parse_pnpm_resolution(&lines, *line_number, end_line, entry, path)?;
        let source = tarball.unwrap_or_else(|| canonical_npm_tarball_url(&key.name, &key.version));
        let package = PnpmPackage {
            key: key.clone(),
            checksum,
            source,
        };
        if packages.insert(key.clone(), package).is_some() {
            return malformed_lock(
                path,
                format!(
                    "line {line_number}: duplicate package identity npm:{}@{}",
                    key.name, key.version
                ),
            );
        }
    }

    Ok(packages.into_values().collect())
}

fn parse_pnpm_resolution(
    lines: &[&str],
    package_line: usize,
    end_line: usize,
    entry: &str,
    path: &Path,
) -> SupplyChainResult<(PackageChecksum, Option<String>)> {
    let mut resolution = None;
    for (index, line) in lines
        .iter()
        .enumerate()
        .take(end_line - 1)
        .skip(package_line)
    {
        let line_number = index + 1;
        let line = *line;
        let Some(value) = line.strip_prefix("    resolution:") else {
            continue;
        };
        if resolution.is_some() {
            return malformed_lock(
                path,
                format!("line {line_number}: package `{entry}` has duplicate resolution"),
            );
        }
        resolution = Some(parse_pnpm_resolution_value(
            lines,
            line_number,
            end_line,
            value.trim(),
            path,
        )?);
    }

    resolution.ok_or_else(|| SupplyChainError::MalformedLock {
        path: path.to_path_buf(),
        detail: format!("line {package_line}: package `{entry}` is missing resolution"),
    })
}

fn parse_pnpm_resolution_value(
    lines: &[&str],
    resolution_line: usize,
    end_line: usize,
    value: &str,
    path: &Path,
) -> SupplyChainResult<(PackageChecksum, Option<String>)> {
    let mut integrity = None;
    let mut tarball = None;
    if value.is_empty() {
        for (index, line) in lines
            .iter()
            .enumerate()
            .take(end_line - 1)
            .skip(resolution_line)
        {
            let line_number = index + 1;
            let line = *line;
            if !line.starts_with("      ") {
                break;
            }
            let Some((key, value)) = line[6..].split_once(':') else {
                continue;
            };
            parse_pnpm_resolution_field(
                key.trim(),
                value.trim(),
                line_number,
                path,
                &mut integrity,
                &mut tarball,
            )?;
        }
    } else {
        let mapping = value
            .strip_prefix('{')
            .and_then(|value| value.strip_suffix('}'))
            .ok_or_else(|| SupplyChainError::MalformedLock {
                path: path.to_path_buf(),
                detail: format!("line {resolution_line}: resolution must be a mapping"),
            })?;
        for field in split_yaml_flow_fields(mapping, path, resolution_line)? {
            let (key, value) =
                field
                    .split_once(':')
                    .ok_or_else(|| SupplyChainError::MalformedLock {
                        path: path.to_path_buf(),
                        detail: format!(
                            "line {resolution_line}: malformed field in resolution mapping"
                        ),
                    })?;
            parse_pnpm_resolution_field(
                key.trim(),
                value.trim(),
                resolution_line,
                path,
                &mut integrity,
                &mut tarball,
            )?;
        }
    }

    let (integrity, integrity_line) = integrity.ok_or_else(|| SupplyChainError::MalformedLock {
        path: path.to_path_buf(),
        detail: format!("line {resolution_line}: resolution is missing integrity"),
    })?;
    Ok((
        decode_sha512_integrity(&integrity, path, integrity_line)?,
        tarball.map(|(value, _)| value),
    ))
}

fn parse_pnpm_resolution_field(
    key: &str,
    value: &str,
    line_number: usize,
    path: &Path,
    integrity: &mut Option<(String, usize)>,
    tarball: &mut Option<(String, usize)>,
) -> SupplyChainResult<()> {
    let destination = match key {
        "integrity" => integrity,
        "tarball" => tarball,
        _ => return Ok(()),
    };
    let value = parse_yaml_scalar(value, path, line_number)?;
    if destination.replace((value, line_number)).is_some() {
        return malformed_lock(
            path,
            format!("line {line_number}: duplicate resolution `{key}` field"),
        );
    }
    Ok(())
}

fn split_yaml_flow_fields<'a>(
    mapping: &'a str,
    path: &Path,
    line_number: usize,
) -> SupplyChainResult<Vec<&'a str>> {
    let mut fields = Vec::new();
    let mut start = 0;
    let mut quote = None;
    let mut escaped = false;
    for (offset, character) in mapping.char_indices() {
        match quote {
            Some('"') if escaped => escaped = false,
            Some('"') if character == '\\' => escaped = true,
            Some(current) if character == current => quote = None,
            None if matches!(character, '"' | '\'') => quote = Some(character),
            None if character == ',' => {
                fields.push(mapping[start..offset].trim());
                start = offset + 1;
            }
            Some(_) | None => {}
        }
    }
    if quote.is_some() {
        return malformed_lock(
            path,
            format!("line {line_number}: unterminated quote in resolution mapping"),
        );
    }
    fields.push(mapping[start..].trim());
    if fields.iter().any(|field| field.is_empty()) {
        return malformed_lock(
            path,
            format!("line {line_number}: empty field in resolution mapping"),
        );
    }
    Ok(fields)
}

fn decode_sha512_integrity(
    integrity: &str,
    path: &Path,
    line_number: usize,
) -> SupplyChainResult<PackageChecksum> {
    let (algorithm, encoded) =
        integrity
            .split_once('-')
            .ok_or_else(|| SupplyChainError::MalformedLock {
                path: path.to_path_buf(),
                detail: format!("line {line_number}: malformed package integrity"),
            })?;
    if algorithm != "sha512" {
        return malformed_lock(
            path,
            format!("line {line_number}: unsupported package integrity algorithm `{algorithm}`"),
        );
    }
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|error| SupplyChainError::MalformedLock {
            path: path.to_path_buf(),
            detail: format!("line {line_number}: malformed SHA-512 integrity: {error}"),
        })?;
    if decoded.len() != 64 {
        return malformed_lock(
            path,
            format!(
                "line {line_number}: SHA-512 integrity decoded to {} bytes; expected 64",
                decoded.len()
            ),
        );
    }
    let mut value = String::with_capacity(128);
    for byte in decoded {
        let _ = write!(value, "{byte:02x}");
    }
    Ok(PackageChecksum {
        algorithm: ChecksumAlgorithm::Sha512,
        value,
    })
}

fn parse_yaml_section_keys(
    contents: &str,
    section: &str,
    path: &Path,
) -> SupplyChainResult<BTreeMap<String, usize>> {
    let header = format!("{section}:");
    let mut in_section = false;
    let mut found = false;
    let mut keys = BTreeMap::new();
    for (index, line) in contents.lines().enumerate() {
        let line_number = index + 1;
        if !in_section {
            if line == header {
                in_section = true;
                found = true;
            }
            continue;
        }
        if !line.is_empty() && !line.starts_with(' ') {
            break;
        }
        if let Some(entry) = line.strip_prefix("  ") {
            if entry.starts_with(' ')
                || entry.trim().is_empty()
                || entry.trim_start().starts_with('#')
            {
                continue;
            }

            let mut characters = entry.char_indices().peekable();
            let mut quote = None;
            let mut escaped = false;
            let mut separator = None;
            while let Some((offset, character)) = characters.next() {
                match quote {
                    Some('"') => {
                        if escaped {
                            escaped = false;
                        } else if character == '\\' {
                            escaped = true;
                        } else if character == '"' {
                            quote = None;
                        }
                    }
                    Some('\'') => {
                        if character == '\'' {
                            if characters.peek().is_some_and(|(_, next)| *next == '\'') {
                                characters.next();
                            } else {
                                quote = None;
                            }
                        }
                    }
                    Some(_) => unreachable!("only YAML string quote characters are tracked"),
                    None if offset == 0 && matches!(character, '"' | '\'') => {
                        quote = Some(character);
                    }
                    None if character == ':' => {
                        let inline_value = &entry[offset + character.len_utf8()..];
                        if inline_value.is_empty() || inline_value.starts_with(' ') {
                            separator = Some((offset, inline_value));
                            break;
                        }
                    }
                    None => {}
                }
            }
            let (separator, inline_value) =
                separator.ok_or_else(|| SupplyChainError::MalformedLock {
                    path: path.to_path_buf(),
                    detail: format!("line {line_number}: malformed key in `{section}` section"),
                })?;
            if !matches!(inline_value, "" | " {}") {
                return malformed_lock(
                    path,
                    format!("line {line_number}: malformed key in `{section}` section"),
                );
            }

            let key = parse_yaml_scalar(entry[..separator].trim(), path, line_number)?;
            if keys.insert(key.clone(), line_number).is_some() {
                return malformed_lock(
                    path,
                    format!("line {line_number}: duplicate `{section}` key `{key}`"),
                );
            }
        }
    }
    if !found {
        return malformed_lock(path, format!("missing top-level `{section}` section"));
    }
    Ok(keys)
}

fn parse_yaml_scalar(value: &str, path: &Path, line_number: usize) -> SupplyChainResult<String> {
    if value.len() >= 2 && value.starts_with('\'') && value.ends_with('\'') {
        return Ok(value[1..value.len() - 1].replace("''", "'"));
    }
    if value.starts_with('"') {
        return serde_json::from_str(value).map_err(|error| SupplyChainError::MalformedLock {
            path: path.to_path_buf(),
            detail: format!("line {line_number}: invalid quoted YAML scalar: {error}"),
        });
    }
    if value.is_empty() || value.contains(" #") {
        return malformed_lock(
            path,
            format!("line {line_number}: invalid empty or annotated scalar"),
        );
    }
    Ok(value.to_owned())
}

fn parse_pnpm_package_key(
    entry: &str,
    path: &Path,
    line_number: usize,
) -> SupplyChainResult<PackageKey> {
    let entry = entry.strip_prefix('/').unwrap_or(entry);
    let base = entry.split_once('(').map_or(entry, |(base, _)| base);
    let split = base.rfind('@').filter(|index| *index > 0).ok_or_else(|| {
        SupplyChainError::MalformedLock {
            path: path.to_path_buf(),
            detail: format!("line {line_number}: package key `{entry}` has no exact name@version"),
        }
    })?;
    let (name, version_with_at) = base.split_at(split);
    let version = &version_with_at[1..];
    if name.is_empty() || version.is_empty() {
        return malformed_lock(
            path,
            format!("line {line_number}: package key `{entry}` has an empty name or version"),
        );
    }
    Ok(PackageKey {
        ecosystem: "npm".to_owned(),
        name: name.to_owned(),
        version: version.to_owned(),
    })
}

fn canonical_npm_tarball_url(name: &str, version: &str) -> String {
    let filename = name.rsplit('/').next().unwrap_or(name);
    format!("https://registry.npmjs.org/{name}/-/{filename}-{version}.tgz")
}

fn insert_package(
    packages: &mut BTreeMap<PackageKey, InventoryPackage>,
    package: InventoryPackage,
) -> SupplyChainResult<()> {
    let key = PackageKey {
        ecosystem: package.ecosystem.clone(),
        name: package.name.clone(),
        version: package.version.clone(),
    };
    if let Some(previous) = packages.insert(key.clone(), package) {
        return Err(SupplyChainError::MalformedLock {
            path: PathBuf::from(if key.ecosystem == "cargo" {
                CARGO_LOCK
            } else {
                PNPM_LOCK
            }),
            detail: format!(
                "duplicate package identity {}:{}@{} (previous source: {})",
                key.ecosystem, key.name, key.version, previous.source
            ),
        });
    }
    Ok(())
}

fn render_inventory(
    packages: &[InventoryPackage],
    assets: &[ReviewedAsset],
) -> SupplyChainResult<String> {
    let inventory = LicenseInventory {
        schema_version: 2,
        generated_by: "cargo xtask licenses generate",
        authoritative_inputs: [CARGO_LOCK, PNPM_LOCK, REVIEWED_INPUTS],
        packages,
        assets,
    };
    render_json(&inventory, "license inventory")
}

fn render_notice(
    packages: &[InventoryPackage],
    assets: &[ReviewedAsset],
) -> SupplyChainResult<String> {
    let dependencies: Vec<_> = packages
        .iter()
        .filter(|package| package.kind == "dependency")
        .collect();
    let unresolved = dependencies
        .iter()
        .filter(|package| package.license_concluded == NOASSERTION)
        .count();
    let mut output = String::new();
    writeln!(
        output,
        "<!-- @generated by `cargo xtask licenses generate`; do not edit by hand. -->"
    )
    .map_err(notice_render_error)?;
    writeln!(output, "<!-- SPDX-License-Identifier: Apache-2.0 -->\n")
        .map_err(notice_render_error)?;
    writeln!(output, "# Third-party notices\n").map_err(notice_render_error)?;
    writeln!(
        output,
        "This deterministic Phase-00 inventory is derived from `{CARGO_LOCK}`, `{PNPM_LOCK}`, and `{REVIEWED_INPUTS}`. It inventories the locked development workspace; it is not a Phase-11 assembled-artifact notice set.\n"
    )
    .map_err(notice_render_error)?;
    writeln!(output, "## Locked dependency inventory\n").map_err(notice_render_error)?;
    writeln!(
        output,
        "| Ecosystem | Package | Version | Concluded license | Locked source |"
    )
    .map_err(notice_render_error)?;
    writeln!(output, "|---|---|---:|---|---|").map_err(notice_render_error)?;
    for package in dependencies {
        writeln!(
            output,
            "| {} | `{}` | `{}` | `{}` | `{}` |",
            markdown_cell(&package.ecosystem),
            markdown_cell(&package.name),
            markdown_cell(&package.version),
            markdown_cell(&package.license_concluded),
            markdown_cell(&package.source)
        )
        .map_err(notice_render_error)?;
    }
    writeln!(output).map_err(notice_render_error)?;
    if !assets.is_empty() {
        writeln!(output, "## Reviewed bundled assets\n").map_err(notice_render_error)?;
        for asset in assets {
            writeln!(
                output,
                "### {} {}\n\nSource: {}\n\nLicense: `{}`\n",
                markdown_cell(&asset.name),
                markdown_cell(&asset.version),
                markdown_cell(&asset.source),
                markdown_cell(&asset.license_concluded)
            )
            .map_err(notice_render_error)?;
            for file in asset.files.iter().chain(&asset.notices) {
                writeln!(
                    output,
                    "- [{}]({}) — SHA256 `{}`",
                    markdown_cell(&file.path),
                    markdown_cell(&file.path),
                    file.sha256
                )
                .map_err(notice_render_error)?;
            }
            writeln!(output).map_err(notice_render_error)?;
        }
    }
    writeln!(output, "## Review state\n").map_err(notice_render_error)?;
    let unresolved_assets = assets
        .iter()
        .filter(|asset| asset.license_concluded == NOASSERTION)
        .count();
    writeln!(output, "{unresolved_assets} reviewed asset group(s) have `NOASSERTION`: exact source bytes and attribution are recorded, but their license conclusion remains unresolved.\n")
        .map_err(notice_render_error)?;

    writeln!(
        output,
        "{unresolved} locked dependency package(s) have `NOASSERTION` because no exact conclusion is present in the reviewed static input. Generation records those unresolved facts rather than guessing. A Phase-11 release remains blocked until every shipped component has a reviewed conclusion, required attribution, and corresponding license text."
    )
    .map_err(notice_render_error)?;
    Ok(output)
}

fn notice_render_error(error: fmt::Error) -> SupplyChainError {
    SupplyChainError::Render {
        artifact: NOTICE,
        detail: error.to_string(),
    }
}

fn markdown_cell(value: &str) -> String {
    value.replace('|', "\\|").replace(['\n', '\r'], " ")
}

fn render_sbom(inventory: &Inventory) -> SupplyChainResult<String> {
    let mut spdx_packages = Vec::with_capacity(inventory.packages.len());
    let mut relationships = Vec::with_capacity(inventory.packages.len());
    for (index, package) in inventory.packages.iter().enumerate() {
        let spdx_id = format!(
            "SPDXRef-Package-{}-{}-{}-{:04}",
            spdx_slug(&package.ecosystem),
            spdx_slug(&package.name),
            spdx_slug(&package.version),
            index + 1
        );
        relationships.push(SpdxRelationship {
            spdx_element_id: "SPDXRef-DOCUMENT",
            relationship_type: "DESCRIBES",
            related_spdx_element: spdx_id.clone(),
        });
        let checksums = package.checksum.as_ref().map_or_else(Vec::new, |checksum| {
            vec![SpdxChecksum {
                algorithm: match checksum.algorithm {
                    ChecksumAlgorithm::Sha256 => "SHA256",
                    ChecksumAlgorithm::Sha512 => "SHA512",
                },
                checksum_value: checksum.value.clone(),
            }]
        });
        spdx_packages.push(SpdxPackage {
            spdx_id,
            name: package.name.clone(),
            version_info: package.version.clone(),
            download_location: if package.kind == "dependency"
                && (package.ecosystem == "npm" || package.source.starts_with("https://"))
            {
                package.source.clone()
            } else {
                NOASSERTION.to_owned()
            },
            files_analyzed: false,
            license_concluded: package.license_concluded.clone(),
            license_declared: package.license_concluded.clone(),
            copyright_text: NOASSERTION,
            external_refs: [SpdxExternalRef {
                category: "PACKAGE-MANAGER",
                kind: "purl",
                locator: package_url(package),
            }],
            checksums,
        });
    }

    let files = render_asset_files(&inventory.reviewed.reviewed_assets, &mut relationships);

    render_json(
        &SpdxDocument {
            spdx_version: "SPDX-2.3",
            data_license: "CC0-1.0",
            spdx_id: "SPDXRef-DOCUMENT",
            name: &inventory.reviewed.document_name,
            document_namespace: &inventory.reviewed.document_namespace,
            creation_info: SpdxCreationInfo {
                created: &inventory.reviewed.created,
                creators: ["Tool: eutheto xtask"],
            },
            packages: spdx_packages,
            files,
            relationships,
            extracted_licensing_infos: &inventory.extracted_licensing_infos,
        },
        "SPDX-JSON SBOM",
    )
}

fn render_asset_files(
    assets: &[ReviewedAsset],
    relationships: &mut Vec<SpdxRelationship>,
) -> Vec<SpdxFile> {
    let mut files = Vec::new();
    let mut seen_asset_paths = BTreeSet::new();
    for asset in assets {
        for file in asset.files.iter().chain(&asset.notices) {
            if !seen_asset_paths.insert(&file.path) {
                continue;
            }
            let spdx_id = format!("SPDXRef-AssetFile-{}", files.len() + 1);
            relationships.push(SpdxRelationship {
                spdx_element_id: "SPDXRef-DOCUMENT",
                relationship_type: "DESCRIBES",
                related_spdx_element: spdx_id.clone(),
            });
            files.push(SpdxFile {
                spdx_id,
                file_name: format!("./{}", file.path),
                checksums: vec![SpdxChecksum {
                    algorithm: "SHA256",
                    checksum_value: file.sha256.clone(),
                }],
                license_concluded: if asset.files.iter().any(|font| font.path == file.path) {
                    asset.license_concluded.clone()
                } else {
                    NOASSERTION.to_owned()
                },
                license_info_in_files: vec![NOASSERTION.to_owned()],
                copyright_text: NOASSERTION,
                comment: format!(
                    "Reviewed asset {} {}; source {}",
                    asset.name, asset.version, asset.source
                ),
            });
        }
    }

    files
}

fn render_json(value: &impl Serialize, artifact: &'static str) -> SupplyChainResult<String> {
    let mut output =
        serde_json::to_string_pretty(value).map_err(|error| SupplyChainError::Render {
            artifact,
            detail: error.to_string(),
        })?;
    output.push('\n');
    Ok(output)
}

fn package_url(package: &InventoryPackage) -> String {
    format!(
        "pkg:{}/{}@{}",
        package.ecosystem,
        percent_encode(&package.name, package.ecosystem == "npm"),
        percent_encode(&package.version, false)
    )
}

fn percent_encode(value: &str, preserve_slash: bool) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric()
            || matches!(byte, b'-' | b'.' | b'_' | b'~')
            || (preserve_slash && byte == b'/')
        {
            encoded.push(char::from(byte));
        } else {
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}

fn spdx_slug(value: &str) -> String {
    let slug: String = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '-') {
                character
            } else {
                '-'
            }
        })
        .collect();
    if slug.is_empty() {
        "unknown".to_owned()
    } else {
        slug
    }
}

fn write_if_changed(
    repo_root: &Path,
    relative_path: &'static str,
    contents: &str,
) -> SupplyChainResult<()> {
    let path = repo_root.join(relative_path);
    if fs::read_to_string(&path).ok().as_deref() == Some(contents) {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| SupplyChainError::WriteArtifact {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    fs::write(&path, contents).map_err(|source| SupplyChainError::WriteArtifact { path, source })
}

fn check_outputs(repo_root: &Path, expected: &[(&'static str, String)]) -> SupplyChainResult<()> {
    let drifted: Vec<_> = expected
        .iter()
        .filter_map(|(relative_path, expected_contents)| {
            let path = repo_root.join(relative_path);
            match fs::read_to_string(path) {
                Ok(actual) if actual == *expected_contents => None,
                Ok(_) | Err(_) => Some(*relative_path),
            }
        })
        .collect();
    if drifted.is_empty() {
        Ok(())
    } else {
        Err(SupplyChainError::Drift { paths: drifted })
    }
}

fn read_required(path: &Path) -> SupplyChainResult<String> {
    fs::read_to_string(path).map_err(|source| {
        if source.kind() == io::ErrorKind::NotFound {
            SupplyChainError::MissingInput {
                path: path.to_path_buf(),
            }
        } else {
            SupplyChainError::ReadInput {
                path: path.to_path_buf(),
                source,
            }
        }
    })
}

fn malformed_lock<T>(path: &Path, detail: String) -> SupplyChainResult<T> {
    Err(SupplyChainError::MalformedLock {
        path: path.to_path_buf(),
        detail,
    })
}

fn looks_like_utc_timestamp(value: &str) -> bool {
    value.len() == 20
        && value.as_bytes().get(4) == Some(&b'-')
        && value.as_bytes().get(7) == Some(&b'-')
        && value.as_bytes().get(10) == Some(&b'T')
        && value.as_bytes().get(13) == Some(&b':')
        && value.as_bytes().get(16) == Some(&b':')
        && value.ends_with('Z')
        && value.bytes().enumerate().all(|(index, byte)| {
            matches!(index, 4 | 7 | 10 | 13 | 16 | 19) || byte.is_ascii_digit()
        })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::Path;

    use base64::Engine as _;
    use serde_json::json;

    use super::{
        ChecksumAlgorithm, Inventory, InventoryPackage, PackageChecksum, PackageKey,
        ReviewedInputs, SupplyChainError, SupplyChainResult, canonical_npm_tarball_url,
        parse_pnpm_package_key, parse_pnpm_packages, parse_yaml_section_keys, percent_encode,
        render_inventory, render_sbom,
    };

    fn pnpm_package_with_resolution(resolution: &str) -> SupplyChainResult<super::PnpmPackage> {
        let contents =
            format!("packages:\n  package@1.2.3:\n    resolution: {resolution}\nsnapshots:\n");
        let path = Path::new("pnpm-lock.yaml");
        let entries = parse_yaml_section_keys(&contents, "packages", path)?;
        let mut packages = parse_pnpm_packages(&contents, entries, path)?;
        packages
            .pop()
            .ok_or_else(|| SupplyChainError::MalformedLock {
                path: path.to_path_buf(),
                detail: "synthetic package was not parsed".to_owned(),
            })
    }

    #[test]
    fn parses_scoped_pnpm_key_with_peer_context() -> SupplyChainResult<()> {
        let key = parse_pnpm_package_key(
            "@vue/server-renderer@3.5.42(vue@3.5.42)",
            Path::new("pnpm-lock.yaml"),
            1,
        )?;
        assert_eq!(
            key,
            PackageKey {
                ecosystem: "npm".to_owned(),
                name: "@vue/server-renderer".to_owned(),
                version: "3.5.42".to_owned(),
            }
        );
        Ok(())
    }

    #[test]
    fn parses_nested_and_empty_flow_map_pnpm_section_keys() -> SupplyChainResult<()> {
        let contents = concat!(
            "importers:\n",
            "  .: {}\n",
            "  'workspace:tools':\n",
            "    dependencies: {}\n",
            "packages:\n",
            "  name@1.2.3: {}\n",
            "  nested@4.5.6:\n",
            "    resolution: {}\n",
        );
        let path = Path::new("pnpm-lock.yaml");

        assert_eq!(
            parse_yaml_section_keys(contents, "importers", path)?,
            BTreeMap::from([(".".to_owned(), 2), ("workspace:tools".to_owned(), 3)])
        );
        assert_eq!(
            parse_yaml_section_keys(contents, "packages", path)?,
            BTreeMap::from([("name@1.2.3".to_owned(), 6), ("nested@4.5.6".to_owned(), 7),])
        );
        Ok(())
    }

    #[test]
    fn rejects_inline_pnpm_section_values_and_comments() {
        for entry in [
            "name@1.2.3: false",
            "name@1.2.3: { resolution: registry }",
            "name@1.2.3: # empty package",
        ] {
            let contents = format!("packages:\n  {entry}\n");
            assert_eq!(
                parse_yaml_section_keys(&contents, "packages", Path::new("pnpm-lock.yaml"))
                    .map_err(|error| error.to_string()),
                Err(
                    "malformed lock file pnpm-lock.yaml: line 2: malformed key in `packages` section"
                        .to_owned()
                )
            );
        }
    }

    #[test]
    fn preserves_duplicate_pnpm_section_key_diagnostic_across_mapping_forms() {
        let contents = "packages:\n  name@1.2.3:\n  name@1.2.3: {}\n";
        assert_eq!(
            parse_yaml_section_keys(contents, "packages", Path::new("pnpm-lock.yaml"))
                .map_err(|error| error.to_string()),
            Err(
                "malformed lock file pnpm-lock.yaml: line 3: duplicate `packages` key `name@1.2.3`"
                    .to_owned()
            )
        );
    }

    #[test]
    fn purl_encoding_preserves_npm_namespace_separator() {
        assert_eq!(percent_encode("@tauri-apps/api", true), "%40tauri-apps/api");
    }

    #[test]
    fn canonical_scoped_npm_tarball_url_uses_unscoped_filename() {
        assert_eq!(
            canonical_npm_tarball_url("@tauri-apps/api", "2.11.1"),
            "https://registry.npmjs.org/@tauri-apps/api/-/api-2.11.1.tgz"
        );
    }

    #[test]
    fn decodes_sha512_integrity_to_canonical_hex() -> SupplyChainResult<()> {
        let encoded = format!("{}AA==", "A".repeat(84));
        let package = pnpm_package_with_resolution(&format!("{{integrity: sha512-{encoded}}}"))?;
        assert_eq!(
            package.checksum,
            PackageChecksum {
                algorithm: ChecksumAlgorithm::Sha512,
                value: "00".repeat(64),
            }
        );
        assert_eq!(
            package.source,
            "https://registry.npmjs.org/package/-/package-1.2.3.tgz"
        );
        Ok(())
    }

    #[test]
    fn preserves_explicit_pnpm_tarball_source() -> SupplyChainResult<()> {
        let encoded = base64::engine::general_purpose::STANDARD.encode([1_u8; 64]);
        let tarball = "https://registry.example.test/package-1.2.3.tgz";
        let package = pnpm_package_with_resolution(&format!(
            "{{integrity: sha512-{encoded}, tarball: {tarball}}}"
        ))?;
        assert_eq!(package.source, tarball);
        Ok(())
    }

    #[test]
    fn rejects_malformed_sha512_base64_at_resolution_line() {
        assert_eq!(
            pnpm_package_with_resolution("{integrity: sha512-***}")
                .map_err(|error| error.to_string()),
            Err(
                "malformed lock file pnpm-lock.yaml: line 3: malformed SHA-512 integrity: Invalid symbol 42, offset 0."
                    .to_owned()
            )
        );
    }

    #[test]
    fn rejects_noncanonical_sha512_base64_at_resolution_line() {
        let canonical = format!("{}AA==", "A".repeat(84));
        for encoded in [
            canonical.trim_end_matches('=').to_owned(),
            format!("{canonical}="),
            format!("{}AB==", "A".repeat(84)),
            format!("{}A_==", "A".repeat(84)),
        ] {
            let result = pnpm_package_with_resolution(&format!("{{integrity: sha512-{encoded}}}"));
            assert!(
                matches!(result, Err(error) if error.to_string().starts_with(
                    "malformed lock file pnpm-lock.yaml: line 3: malformed SHA-512 integrity:"
                ))
            );
        }
    }

    #[test]
    fn rejects_wrong_sha512_integrity_length_at_resolution_line() {
        for length in [63, 65] {
            let encoded = base64::engine::general_purpose::STANDARD.encode(vec![0_u8; length]);
            assert_eq!(
                pnpm_package_with_resolution(&format!("{{integrity: sha512-{encoded}}}"))
                    .map_err(|error| error.to_string()),
                Err(format!(
                    "malformed lock file pnpm-lock.yaml: line 3: SHA-512 integrity decoded to {length} bytes; expected 64"
                ))
            );
        }
    }

    #[test]
    fn rejects_unsupported_integrity_algorithm_at_resolution_line() {
        let encoded = base64::engine::general_purpose::STANDARD.encode([0_u8; 64]);
        assert_eq!(
            pnpm_package_with_resolution(&format!("{{integrity: sha256-{encoded}}}"))
                .map_err(|error| error.to_string()),
            Err(
                "malformed lock file pnpm-lock.yaml: line 3: unsupported package integrity algorithm `sha256`"
                    .to_owned()
            )
        );
    }

    #[test]
    fn rejects_package_without_resolution_at_package_line() -> SupplyChainResult<()> {
        let contents = "packages:\n  package@1.2.3:\n    engines: {}\nsnapshots:\n";
        let path = Path::new("pnpm-lock.yaml");
        let entries = parse_yaml_section_keys(contents, "packages", path)?;
        assert_eq!(
            parse_pnpm_packages(contents, entries, path).map_err(|error| error.to_string()),
            Err(
                "malformed lock file pnpm-lock.yaml: line 2: package `package@1.2.3` is missing resolution"
                    .to_owned()
            )
        );
        Ok(())
    }

    #[test]
    fn renders_spdx_checksum_algorithms_and_npm_download_location() -> SupplyChainResult<()> {
        let npm_source = canonical_npm_tarball_url("@scope/package", "2.0.0");
        let inventory = Inventory {
            reviewed: ReviewedInputs {
                schema_version: 1,
                document_name: "test".to_owned(),
                document_namespace: "https://example.test/sbom".to_owned(),
                created: "2026-01-01T00:00:00Z".to_owned(),
                workspace_packages: Vec::new(),
                reviewed_vendored_packages: Vec::new(),
                license_conclusions: Vec::new(),
                reviewed_assets: Vec::new(),
                extracted_licenses: Vec::new(),
            },
            extracted_licensing_infos: Vec::new(),
            packages: vec![
                InventoryPackage {
                    ecosystem: "cargo".to_owned(),
                    name: "crate".to_owned(),
                    version: "1.0.0".to_owned(),
                    kind: "dependency".to_owned(),
                    license_concluded: "MIT".to_owned(),
                    source: "registry+https://github.com/rust-lang/crates.io-index".to_owned(),
                    checksum: Some(PackageChecksum {
                        algorithm: ChecksumAlgorithm::Sha256,
                        value: "11".repeat(32),
                    }),
                },
                InventoryPackage {
                    ecosystem: "npm".to_owned(),
                    name: "@scope/package".to_owned(),
                    version: "2.0.0".to_owned(),
                    kind: "dependency".to_owned(),
                    license_concluded: "MIT".to_owned(),
                    source: npm_source.clone(),
                    checksum: Some(PackageChecksum {
                        algorithm: ChecksumAlgorithm::Sha512,
                        value: "22".repeat(64),
                    }),
                },
            ],
        };

        let rendered_inventory =
            render_inventory(&inventory.packages, &inventory.reviewed.reviewed_assets)?;
        let rendered_inventory: serde_json::Value = serde_json::from_str(&rendered_inventory)
            .map_err(|error| SupplyChainError::Render {
                artifact: "test inventory",
                detail: error.to_string(),
            })?;
        assert_eq!(rendered_inventory["schemaVersion"], 2);
        assert_eq!(
            rendered_inventory["packages"][0]["checksum"],
            json!({"algorithm": "SHA256", "value": "11".repeat(32)})
        );
        assert_eq!(
            rendered_inventory["packages"][1]["checksum"],
            json!({"algorithm": "SHA512", "value": "22".repeat(64)})
        );

        let rendered = render_sbom(&inventory)?;
        let rendered: serde_json::Value =
            serde_json::from_str(&rendered).map_err(|error| SupplyChainError::Render {
                artifact: "test SPDX",
                detail: error.to_string(),
            })?;
        assert_eq!(
            rendered["packages"][0]["checksums"],
            json!([{"algorithm": "SHA256", "checksumValue": "11".repeat(32)}])
        );
        assert_eq!(
            rendered["packages"][1]["checksums"],
            json!([{"algorithm": "SHA512", "checksumValue": "22".repeat(64)}])
        );
        assert_eq!(rendered["packages"][1]["downloadLocation"], npm_source);
        assert!(rendered.get("hasExtractedLicensingInfos").is_none());
        Ok(())
    }
    #[test]
    fn assets_require_matching_direct_files_and_render_as_spdx_files()
    -> Result<(), Box<dyn std::error::Error>> {
        use sha2::{Digest, Sha256};
        let directory = tempfile::tempdir()?;
        std::fs::write(directory.path().join("font.otf"), b"font fixture")?;
        std::fs::write(directory.path().join("LICENSE"), b"license fixture")?;
        let file = super::ReviewedAssetFile {
            path: "font.otf".to_owned(),
            sha256: format!("{:x}", Sha256::digest(b"font fixture")),
        };
        let asset = super::ReviewedAsset {
            name: "Fixture font".to_owned(),
            version: "1".to_owned(),
            source: "https://example.test/font".to_owned(),
            license_concluded: "OFL-1.1".to_owned(),
            files: vec![file.clone()],
            notices: vec![super::ReviewedAssetFile {
                path: "LICENSE".to_owned(),
                sha256: format!("{:x}", Sha256::digest(b"license fixture")),
            }],
        };
        super::validate_assets(directory.path(), std::slice::from_ref(&asset))?;
        let inventory = Inventory {
            reviewed: ReviewedInputs {
                schema_version: 1,
                document_name: "fixture".to_owned(),
                document_namespace: "urn:fixture".to_owned(),
                created: "2026-01-01T00:00:00Z".to_owned(),
                workspace_packages: vec![],
                reviewed_vendored_packages: vec![],
                license_conclusions: vec![],
                reviewed_assets: vec![asset],
                extracted_licenses: vec![],
            },
            packages: vec![],
            extracted_licensing_infos: vec![],
        };
        let sbom: serde_json::Value = serde_json::from_str(&render_sbom(&inventory)?)?;
        assert_eq!(sbom["packages"], json!([]));
        assert_eq!(sbom["files"][0]["licenseConcluded"], "OFL-1.1");
        assert_eq!(
            sbom["files"][0]["checksums"][0]["checksumValue"],
            file.sha256
        );
        assert_eq!(sbom["files"][1]["licenseConcluded"], "NOASSERTION");
        std::fs::write(directory.path().join("font.otf"), b"changed")?;
        assert!(
            super::validate_assets(directory.path(), &inventory.reviewed.reviewed_assets).is_err()
        );
        let mut unsafe_file = file;
        unsafe_file.path = "../font.otf".to_owned();
        assert!(super::validate_asset_file(directory.path(), &unsafe_file).is_err());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("LICENSE", directory.path().join("alias"))?;
            unsafe_file.path = "alias".to_owned();
            assert!(super::validate_asset_file(directory.path(), &unsafe_file).is_err());
        }
        Ok(())
    }

    fn extracted_license_fixture(
        root: &Path,
        text: &[u8],
    ) -> Result<ReviewedInputs, Box<dyn std::error::Error>> {
        use sha2::{Digest, Sha256};
        std::fs::write(root.join("LICENSE"), text)?;
        Ok(serde_json::from_value(json!({
            "schemaVersion": 1,
            "documentName": "fixture",
            "documentNamespace": "urn:fixture",
            "created": "2026-01-01T00:00:00Z",
            "workspacePackages": [],
            "licenseConclusions": [{
                "ecosystem": "cargo", "name": "fixture", "version": "1",
                "license": "MIT AND (LicenseRef-Fixture-1.0 OR Apache-2.0)"
            }],
            "extractedLicenses": [{
                "licenseId": "LicenseRef-Fixture-1.0",
                "name": "Fixture license",
                "source": "https://example.test/license",
                "file": {"path": "LICENSE", "sha256": format!("{:x}", Sha256::digest(text))}
            }]
        }))?)
    }

    #[test]
    fn extracted_license_preserves_reviewed_text_only_in_spdx()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let text = "Fixture grant ©\r\n\nExact whitespace.  \n";
        let reviewed = extracted_license_fixture(directory.path(), text.as_bytes())?;
        let extracted_licensing_infos =
            super::load_extracted_licenses(directory.path(), &reviewed)?;
        let inventory = Inventory {
            reviewed,
            packages: vec![],
            extracted_licensing_infos,
        };
        let sbom: serde_json::Value = serde_json::from_str(&render_sbom(&inventory)?)?;
        assert_eq!(
            sbom["hasExtractedLicensingInfos"],
            json!([{
                "licenseId": "LicenseRef-Fixture-1.0",
                "name": "Fixture license",
                "extractedText": text,
                "seeAlsos": ["https://example.test/license"]
            }])
        );
        assert!(sbom.get("extractedLicensingInfos").is_none());
        let frontend: serde_json::Value = serde_json::from_str(&render_inventory(
            &inventory.packages,
            &inventory.reviewed.reviewed_assets,
        )?)?;
        assert!(frontend.get("extractedLicensingInfos").is_none());
        assert!(frontend.get("hasExtractedLicensingInfos").is_none());
        assert!(frontend.get("extractedLicenses").is_none());
        std::fs::write(directory.path().join("LICENSE"), b"changed grant")?;
        let error = super::load_extracted_licenses(directory.path(), &inventory.reviewed)
            .err()
            .ok_or("changed text was accepted")?;
        assert!(error.to_string().contains("digest changed"));
        Ok(())
    }

    #[test]
    fn extracted_license_requires_exact_unique_referenced_ids()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let reviewed = extracted_license_fixture(directory.path(), b"grant")?;
        let mut missing = reviewed.clone();
        missing.extracted_licenses.clear();
        assert!(super::load_extracted_licenses(directory.path(), &missing).is_err());
        let mut duplicate = reviewed.clone();
        duplicate
            .extracted_licenses
            .push(duplicate.extracted_licenses[0].clone());
        assert!(super::load_extracted_licenses(directory.path(), &duplicate).is_err());
        for id in ["LicenseRef-", "LicenseRef-bad/id", "MIT"] {
            let mut invalid = reviewed.clone();
            invalid.extracted_licenses[0].license_id = id.to_owned();
            assert!(super::load_extracted_licenses(directory.path(), &invalid).is_err());
        }
        let mut unused = reviewed.clone();
        unused.license_conclusions[0].license = "MIT".to_owned();
        assert!(super::load_extracted_licenses(directory.path(), &unused).is_err());
        for expression in [
            "LicenseRef-bad/id",
            "DocumentRef-other:LicenseRef-Fixture-1.0",
        ] {
            let mut invalid_reference = reviewed.clone();
            invalid_reference.license_conclusions[0].license = expression.to_owned();
            assert!(super::load_extracted_licenses(directory.path(), &invalid_reference).is_err());
        }
        Ok(())
    }

    #[test]
    fn inventory_bounds_extracted_text_before_validating_shared_notice()
    -> Result<(), Box<dyn std::error::Error>> {
        use sha2::Digest as _;
        let directory = tempfile::tempdir()?;
        std::fs::create_dir(directory.path().join("xtask"))?;
        std::fs::write(directory.path().join("asset.bin"), b"asset")?;
        std::fs::write(
            directory.path().join("LICENSE"),
            vec![b'a'; super::MAX_EXTRACTED_LICENSE_BYTES + 1],
        )?;
        let stale_license = json!({"path": "LICENSE", "sha256": "00".repeat(32)});
        let inputs = json!({
            "schemaVersion": 1, "documentName": "fixture",
            "documentNamespace": "urn:fixture", "created": "2026-01-01T00:00:00Z",
            "workspacePackages": [], "licenseConclusions": [],
            "reviewedAssets": [{
                "name": "fixture", "version": "1", "source": "https://example.test/asset",
                "licenseConcluded": "LicenseRef-Fixture-1.0",
                "files": [{"path": "asset.bin", "sha256": format!("{:x}", sha2::Sha256::digest(b"asset"))}],
                "notices": [stale_license.clone()]
            }],
            "extractedLicenses": [{
                "licenseId": "LicenseRef-Fixture-1.0", "name": "Fixture license",
                "source": "https://example.test/license", "file": stale_license
            }]
        });
        std::fs::write(
            directory.path().join(super::REVIEWED_INPUTS),
            serde_json::to_vec(&inputs)?,
        )?;
        let error = super::load_inventory(directory.path())
            .err()
            .ok_or("oversized shared notice was accepted")?;
        assert!(
            error
                .to_string()
                .contains("extracted license text exceeds its byte limit")
        );
        Ok(())
    }

    #[test]
    fn extracted_license_text_is_bounded_utf8_and_uses_asset_path_rules()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let reviewed = extracted_license_fixture(directory.path(), &[0xff])?;
        assert!(super::load_extracted_licenses(directory.path(), &reviewed).is_err());
        let mut reviewed = extracted_license_fixture(
            directory.path(),
            &vec![b'a'; super::MAX_EXTRACTED_LICENSE_BYTES],
        )?;
        assert!(super::load_extracted_licenses(directory.path(), &reviewed).is_ok());
        std::fs::write(
            directory.path().join("LICENSE"),
            vec![b'a'; super::MAX_EXTRACTED_LICENSE_BYTES + 1],
        )?;
        let error = super::load_extracted_licenses(directory.path(), &reviewed)
            .err()
            .ok_or("oversized text was accepted")?;
        assert!(error.to_string().contains("byte limit"));
        reviewed.extracted_licenses[0].file.path = "../LICENSE".to_owned();
        assert!(super::load_extracted_licenses(directory.path(), &reviewed).is_err());
        Ok(())
    }
    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "one inventory fixture checks identity, byte integrity and the exact source census"
    )]
    fn vendored_inventory_preserves_dependency_identity_and_checks_source_files()
    -> Result<(), Box<dyn std::error::Error>> {
        use sha2::Digest as _;
        let directory = tempfile::tempdir()?;
        let root = directory.path();
        std::fs::create_dir(root.join("xtask"))?;
        std::fs::create_dir_all(root.join("vendor/fixture"))?;
        let manifest = b"[package]\nname = \"fixture\"\nversion = \"1.0.0\"\nlicense = \"MIT\"\n";
        std::fs::write(root.join("vendor/fixture/Cargo.toml"), manifest)?;
        std::fs::write(root.join("vendor/fixture/lib.rs"), b"source")?;
        std::fs::create_dir(root.join("vendor/fixture/nested"))?;
        std::fs::write(
            root.join("vendor/fixture/nested/source.rs"),
            b"nested source",
        )?;
        std::fs::write(root.join("vendor/fixture/LICENSE"), b"license")?;
        let lock = "version = 4\n[[package]]\nname = \"fixture\"\nversion = \"1.0.0\"\n";
        std::fs::write(root.join("Cargo.lock"), lock)?;
        std::fs::write(
            root.join("pnpm-lock.yaml"),
            format!(
                "lockfileVersion: '9.0'\nimporters:\n  .: {{}}\npackages:\n  npm-fixture@1.0.0:\n    resolution: {{integrity: sha512-{}}}\nsnapshots:\n",
                base64::engine::general_purpose::STANDARD.encode([0_u8; 64])
            ),
        )?;
        let source = format!("https://example.test/repository/tree/{}", "a".repeat(40));
        let mut inputs = json!({
            "schemaVersion": 1, "documentName": "fixture",
            "documentNamespace": "urn:fixture", "created": "2026-01-01T00:00:00Z",
            "workspacePackages": [{"ecosystem":"npm", "name":"workspace", "version":"1", "license":"MIT", "path":"."}],
            "licenseConclusions": [],
            "reviewedVendoredPackages": [{"ecosystem":"cargo", "name":"fixture", "version":"1.0.0", "license":"MIT", "path":"vendor/fixture", "source":source}],
            "reviewedAssets": [{"name":"fixture source", "version":"1.0.0", "source":source, "licenseConcluded":"MIT",
                "files":[
                    {"path":"vendor/fixture/Cargo.toml", "sha256":format!("{:x}", sha2::Sha256::digest(manifest))},
                    {"path":"vendor/fixture/nested/source.rs", "sha256":format!("{:x}", sha2::Sha256::digest(b"nested source"))},
                    {"path":"vendor/fixture/lib.rs", "sha256":format!("{:x}", sha2::Sha256::digest(b"source"))}
                ], "notices":[{"path":"vendor/fixture/LICENSE", "sha256":format!("{:x}", sha2::Sha256::digest(b"license"))}]}]
        });
        let save = |inputs: &serde_json::Value| -> Result<(), Box<dyn std::error::Error>> {
            std::fs::write(
                root.join(super::REVIEWED_INPUTS),
                serde_json::to_vec(inputs)?,
            )?;
            Ok(())
        };
        save(&inputs)?;
        let inventory = super::load_inventory(root)?;
        let package = &inventory.packages[0];
        assert_eq!(package.kind, "dependency");
        assert_eq!(package.source, source);
        assert_eq!(package.license_concluded, "MIT");
        assert!(package.checksum.is_none());
        let sbom: serde_json::Value = serde_json::from_str(&render_sbom(&inventory)?)?;
        assert_eq!(sbom["packages"][0]["downloadLocation"], source);
        std::fs::write(root.join("vendor/fixture/build.rs"), b"fn main() {}")?;
        assert!(super::load_inventory(root).is_err());
        std::fs::remove_file(root.join("vendor/fixture/build.rs"))?;
        assert!(super::load_inventory(root).is_ok());
        #[cfg(unix)]
        {
            std::fs::write(
                root.join("vendor/fixture/nested\\source.rs"),
                b"unreviewed source",
            )?;
            assert!(super::load_inventory(root).is_err());
            std::fs::remove_file(root.join("vendor/fixture/nested\\source.rs"))?;
            std::os::unix::fs::symlink("lib.rs", root.join("vendor/fixture/unlisted.rs"))?;
            assert!(super::load_inventory(root).is_err());
            std::fs::remove_file(root.join("vendor/fixture/unlisted.rs"))?;
        }
        std::fs::write(root.join("vendor/fixture/lib.rs"), b"changed")?;
        assert!(super::load_inventory(root).is_err());
        std::fs::remove_file(root.join("vendor/fixture/lib.rs"))?;
        assert!(super::load_inventory(root).is_err());
        std::fs::write(root.join("vendor/fixture/lib.rs"), b"source")?;
        let original = inputs.clone();
        inputs["reviewedVendoredPackages"][0]["name"] = json!("wrong");
        save(&inputs)?;
        assert!(super::load_inventory(root).is_err());
        inputs = original.clone();
        let duplicate = inputs["reviewedVendoredPackages"][0].clone();
        inputs["reviewedVendoredPackages"]
            .as_array_mut()
            .ok_or("missing array")?
            .push(duplicate);
        save(&inputs)?;
        assert!(super::load_inventory(root).is_err());
        inputs = original.clone();
        inputs["workspacePackages"].as_array_mut().ok_or("missing array")?.push(json!({"ecosystem":"cargo", "name":"fixture", "version":"1.0.0", "license":"MIT", "path":"vendor/fixture"}));
        save(&inputs)?;
        assert!(super::load_inventory(root).is_err());
        save(&original)?;
        std::fs::write(root.join("Cargo.lock"), lock.replace("fixture", "other"))?;
        assert!(super::load_inventory(root).is_err());
        std::fs::write(
            root.join("Cargo.lock"),
            format!(
                "{lock}source = \"registry+https://github.com/rust-lang/crates.io-index\"\nchecksum = \"{}\"\n",
                "b".repeat(64)
            ),
        )?;
        assert!(super::load_inventory(root).is_err());
        inputs = original;
        inputs["reviewedVendoredPackages"] = json!([]);
        save(&inputs)?;
        std::fs::write(root.join("Cargo.lock"), lock)?;
        assert!(super::load_inventory(root).is_err());
        Ok(())
    }
}
