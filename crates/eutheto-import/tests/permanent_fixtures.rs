#[path = "../../../tests/support/portable_decode.rs"]
mod portable_decode;
#[path = "../../../tests/support/portable_encode.rs"]
mod portable_encode;
#[path = "../../../tests/support/portable_migrate.rs"]
mod portable_migrate;

use eutheto_export::{
    ApplicationMetadata, BackupSections, CHECKSUM_ALGORITHM, CHECKSUMS_PATH, Checksums,
    MANIFEST_PATH, PORTABLE_LIMITS, ScenarioExportSnapshot, assemble_scenario_export,
    canonical_json, sha256_hex,
};
use eutheto_import::{
    ImportError, InspectedBundle, InspectionPolicy, MigrationRegistries, PortableMigrationStep,
    VersionSpace, inspect_bundle, inspect_unopened_bundle_for_exact_reexport,
    preflight_bundle_metadata,
};
use eutheto_types::{BundleId, Revision, ScenarioSnapshotV1};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::io::{Cursor, Read, Write};
use std::sync::Arc;
use uuid::Uuid;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

const PORTABLE_V1: &str =
    include_str!("../../../tests/migration/fixtures/portable_v1_scenario.json");
const PORTABLE_V2: &str =
    include_str!("../../../tests/migration/fixtures/portable_v2_scenario.json");
const PORTABLE_UNKNOWN_NEWER: &str =
    include_str!("../../../tests/migration/fixtures/portable_unknown_newer.json");
const ADVERSARIAL_CASES: &str =
    include_str!("../../../tests/security/fixtures/portable/adversarial_cases.json");

type TestResult<T = ()> = Result<T, Box<dyn Error>>;

struct ArchiveEntry {
    path: String,
    bytes: Vec<u8>,
    unix_mode: u32,
}

fn fixture(source: &str, expected_format: &str) -> TestResult<Value> {
    let wrapper: Value = serde_json::from_str(source)?;
    assert_eq!(required_str(&wrapper, "format")?, expected_format);
    assert_eq!(required_u64(&wrapper, "schemaVersion")?, 1);
    assert_eq!(required_u64(&wrapper, "version")?, 1);
    let id = Uuid::parse_str(required_str(&wrapper, "id")?)?;
    assert_eq!(id.get_version_num(), 7);
    Ok(wrapper)
}

fn required_str<'a>(value: &'a Value, field: &str) -> TestResult<&'a str> {
    value
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| std::io::Error::other(format!("missing string field {field}")).into())
}

fn required_u64(value: &Value, field: &str) -> TestResult<u64> {
    value
        .get(field)
        .and_then(Value::as_u64)
        .ok_or_else(|| std::io::Error::other(format!("missing integer field {field}")).into())
}

fn portable_input(wrapper: &Value) -> TestResult<ScenarioSnapshotV1> {
    Ok(serde_json::from_value(
        wrapper
            .get("input")
            .cloned()
            .ok_or_else(|| std::io::Error::other("portable fixture has no input"))?,
    )?)
}

fn production_bundle(
    scenario: ScenarioSnapshotV1,
    bundle_id: &str,
    created_at: &str,
    title: &str,
) -> TestResult<Vec<u8>> {
    Ok(assemble_scenario_export(
        &ScenarioExportSnapshot {
            bundle_id: bundle_id.parse::<BundleId>()?,
            created_at: created_at.to_owned(),
            application: ApplicationMetadata {
                name: "Eutheto".to_owned(),
                version: "0.1.0".to_owned(),
            },
            title: title.to_owned(),
            scenario,
            scenario_revisions: Vec::new(),
            sections: BackupSections::default(),
            nonsemantic_extensions: BTreeSet::new(),
            manifest_extensions: BTreeMap::new(),
        },
        &portable_encode::encode_fixture_domain,
    )?)
}

fn inspect_supported(
    bytes: &[u8],
    policy: &InspectionPolicy,
) -> Result<InspectedBundle, ImportError> {
    let migrations = MigrationRegistries::new(
        Vec::new(),
        vec![PortableMigrationStep {
            from_version: 1,
            to_version: 2,
            name: "portable-v1-to-v2",
            migrate: Arc::new(portable_migrate::migrate_fixture_v1),
        }],
    )?;
    inspect_bundle(
        bytes,
        policy,
        &migrations,
        &portable_decode::decode_fixture_domain,
    )
}

fn archive_entries(bytes: &[u8]) -> TestResult<BTreeMap<String, Vec<u8>>> {
    let mut archive = ZipArchive::new(Cursor::new(bytes))?;
    let mut entries = BTreeMap::new();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        let name = entry.name()?.into_owned();
        let mut content = Vec::new();
        entry.read_to_end(&mut content)?;
        entries.insert(name, content);
    }
    Ok(entries)
}

fn standard_zip(
    entries: impl IntoIterator<Item = ArchiveEntry>,
    compression: CompressionMethod,
) -> TestResult<Vec<u8>> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    for entry in entries {
        let options = SimpleFileOptions::default()
            .compression_method(compression)
            .unix_permissions(entry.unix_mode & 0o777);
        writer.start_file(entry.path, options)?;
        writer.write_all(&entry.bytes)?;
    }
    Ok(writer.finish()?.into_inner())
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & 0_u32.wrapping_sub(crc & 1));
        }
    }
    !crc
}

fn raw_stored_zip(entries: &[ArchiveEntry]) -> TestResult<Vec<u8>> {
    const UTF8_NAME: u16 = 1 << 11;
    const VERSION_NEEDED: u16 = 20;
    const UNIX_VERSION_MADE_BY: u16 = (3 << 8) | VERSION_NEEDED;
    const DOS_DATE_1980_01_01: u16 = (1 << 5) | 1;

    let mut local_entries = Vec::new();
    let mut central_directory = Vec::new();
    for entry in entries {
        let path = entry.path.as_bytes();
        let path_len = u16::try_from(path.len())?;
        let content_len = u32::try_from(entry.bytes.len())?;
        let local_offset = u32::try_from(local_entries.len())?;
        let checksum = crc32(&entry.bytes);

        local_entries.extend_from_slice(&0x0403_4b50_u32.to_le_bytes());
        local_entries.extend_from_slice(&VERSION_NEEDED.to_le_bytes());
        local_entries.extend_from_slice(&UTF8_NAME.to_le_bytes());
        local_entries.extend_from_slice(&0_u16.to_le_bytes());
        local_entries.extend_from_slice(&0_u16.to_le_bytes());
        local_entries.extend_from_slice(&DOS_DATE_1980_01_01.to_le_bytes());
        local_entries.extend_from_slice(&checksum.to_le_bytes());
        local_entries.extend_from_slice(&content_len.to_le_bytes());
        local_entries.extend_from_slice(&content_len.to_le_bytes());
        local_entries.extend_from_slice(&path_len.to_le_bytes());
        local_entries.extend_from_slice(&0_u16.to_le_bytes());
        local_entries.extend_from_slice(path);
        local_entries.extend_from_slice(&entry.bytes);

        central_directory.extend_from_slice(&0x0201_4b50_u32.to_le_bytes());
        central_directory.extend_from_slice(&UNIX_VERSION_MADE_BY.to_le_bytes());
        central_directory.extend_from_slice(&VERSION_NEEDED.to_le_bytes());
        central_directory.extend_from_slice(&UTF8_NAME.to_le_bytes());
        central_directory.extend_from_slice(&0_u16.to_le_bytes());
        central_directory.extend_from_slice(&0_u16.to_le_bytes());
        central_directory.extend_from_slice(&DOS_DATE_1980_01_01.to_le_bytes());
        central_directory.extend_from_slice(&checksum.to_le_bytes());
        central_directory.extend_from_slice(&content_len.to_le_bytes());
        central_directory.extend_from_slice(&content_len.to_le_bytes());
        central_directory.extend_from_slice(&path_len.to_le_bytes());
        central_directory.extend_from_slice(&0_u16.to_le_bytes());
        central_directory.extend_from_slice(&0_u16.to_le_bytes());
        central_directory.extend_from_slice(&0_u16.to_le_bytes());
        central_directory.extend_from_slice(&0_u16.to_le_bytes());
        central_directory.extend_from_slice(&(entry.unix_mode << 16).to_le_bytes());
        central_directory.extend_from_slice(&local_offset.to_le_bytes());
        central_directory.extend_from_slice(path);
    }

    let entry_count = u16::try_from(entries.len())?;
    let central_offset = u32::try_from(local_entries.len())?;
    let central_size = u32::try_from(central_directory.len())?;
    local_entries.extend_from_slice(&central_directory);
    local_entries.extend_from_slice(&0x0605_4b50_u32.to_le_bytes());
    local_entries.extend_from_slice(&0_u16.to_le_bytes());
    local_entries.extend_from_slice(&0_u16.to_le_bytes());
    local_entries.extend_from_slice(&entry_count.to_le_bytes());
    local_entries.extend_from_slice(&entry_count.to_le_bytes());
    local_entries.extend_from_slice(&central_size.to_le_bytes());
    local_entries.extend_from_slice(&central_offset.to_le_bytes());
    local_entries.extend_from_slice(&0_u16.to_le_bytes());
    Ok(local_entries)
}

fn parse_octal(value: &str) -> TestResult<u32> {
    let digits = value
        .strip_prefix("0o")
        .ok_or_else(|| std::io::Error::other(format!("invalid octal mode {value}")))?;
    Ok(u32::from_str_radix(digits, 8)?)
}

fn declared_bytes(value: &Value) -> TestResult<Vec<u8>> {
    match required_str(value, "encoding")? {
        "utf-8" => Ok(required_str(value, "value")?.as_bytes().to_vec()),
        "repeat-byte" => {
            let byte = u8::try_from(required_u64(value, "byte")?)?;
            let count = usize::try_from(required_u64(value, "count")?)?;
            Ok(vec![byte; count])
        }
        encoding => Err(std::io::Error::other(format!("unknown byte encoding {encoding}")).into()),
    }
}

fn declared_archive_entries(input: &Value) -> TestResult<Vec<ArchiveEntry>> {
    let default_mode = input
        .get("options")
        .and_then(|options| options.get("unixMode"))
        .and_then(Value::as_str)
        .map(parse_octal)
        .transpose()?
        .unwrap_or(0o100_600);
    let entries = input
        .get("entries")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("archive case has no entries"))?;
    entries
        .iter()
        .map(|entry| {
            let mode = entry
                .get("unixMode")
                .and_then(Value::as_str)
                .map(parse_octal)
                .transpose()?
                .unwrap_or(default_mode);
            Ok(ArchiveEntry {
                path: required_str(entry, "path")?.to_owned(),
                bytes: declared_bytes(
                    entry
                        .get("bytes")
                        .ok_or_else(|| std::io::Error::other("archive entry has no bytes"))?,
                )?,
                unix_mode: mode,
            })
        })
        .collect()
}

fn recompute_checksums(entries: &mut BTreeMap<String, Vec<u8>>) -> TestResult {
    let files = entries
        .iter()
        .filter(|(path, _)| path.as_str() != CHECKSUMS_PATH)
        .map(|(path, bytes)| (path.clone(), sha256_hex(bytes)))
        .collect();
    entries.insert(
        CHECKSUMS_PATH.to_owned(),
        canonical_json(&Checksums {
            algorithm: CHECKSUM_ALGORITHM.to_owned(),
            files,
        })?,
    );
    Ok(())
}

fn mutate_json_entry(
    entries: &mut BTreeMap<String, Vec<u8>>,
    path: &str,
    mutate: impl FnOnce(&mut Value) -> TestResult,
) -> TestResult {
    let bytes = entries
        .get(path)
        .ok_or_else(|| std::io::Error::other(format!("archive has no {path}")))?;
    let mut value: Value = serde_json::from_slice(bytes)?;
    mutate(&mut value)?;
    entries.insert(path.to_owned(), canonical_json(&value)?);
    Ok(())
}

fn declared_compression(input: &Value) -> TestResult<CompressionMethod> {
    match input
        .get("options")
        .and_then(|options| options.get("compression"))
        .and_then(Value::as_str)
    {
        Some("deflated") => Ok(CompressionMethod::Deflated),
        Some("stored") => Ok(CompressionMethod::Stored),
        other => Err(std::io::Error::other(format!("unsupported compression {other:?}")).into()),
    }
}

fn apply_capability_mutation(
    entries: &mut BTreeMap<String, Vec<u8>>,
    scenario_path: &str,
    mutation: &Value,
) -> TestResult {
    let capability = json!({
        "id": required_str(mutation, "id")?,
        "version": required_u64(mutation, "version")?
    });
    if mutation.get("manifest").and_then(Value::as_bool) == Some(true) {
        mutate_json_entry(entries, MANIFEST_PATH, |manifest| {
            manifest["requiredCapabilities"]
                .as_array_mut()
                .ok_or_else(|| {
                    Box::<dyn Error>::from(std::io::Error::other(
                        "manifest requiredCapabilities is not an array",
                    ))
                })?
                .push(capability.clone());
            Ok(())
        })?;
    }
    if mutation.get("scenario").and_then(Value::as_bool) == Some(true) {
        mutate_json_entry(entries, scenario_path, |scenario| {
            scenario["requiredCapabilities"]
                .as_array_mut()
                .ok_or_else(|| {
                    Box::<dyn Error>::from(std::io::Error::other(
                        "scenario requiredCapabilities is not an array",
                    ))
                })?
                .push(capability);
            Ok(())
        })?;
    }
    Ok(())
}

fn apply_semantic_extension_mutation(
    entries: &mut BTreeMap<String, Vec<u8>>,
    scenario_path: &str,
    mutation: &Value,
) -> TestResult {
    let namespace = required_str(mutation, "namespace")?.to_owned();
    let extension = mutation
        .get("value")
        .cloned()
        .ok_or_else(|| std::io::Error::other("extension mutation has no value"))?;
    mutate_json_entry(entries, scenario_path, |scenario| {
        scenario["semanticExtensions"]
            .as_object_mut()
            .ok_or_else(|| std::io::Error::other("semanticExtensions is not an object"))?
            .insert(namespace, extension);
        Ok(())
    })
}

fn apply_document_extension_mutation(
    entries: &mut BTreeMap<String, Vec<u8>>,
    scenario_path: &str,
    mutation: &Value,
) -> TestResult {
    let namespace = required_str(mutation, "namespace")?.to_owned();
    let field = required_str(mutation, "field")?.to_owned();
    let value = mutation
        .get("value")
        .cloned()
        .ok_or_else(|| std::io::Error::other("extension mutation has no value"))?;
    mutate_json_entry(entries, scenario_path, |scenario| {
        let mut extension = serde_json::Map::new();
        extension.insert(field, value);
        scenario["document"]["extensions"]
            .as_object_mut()
            .ok_or_else(|| std::io::Error::other("document extensions is not an object"))?
            .insert(namespace.clone(), Value::Object(extension));
        Ok(())
    })?;
    mutate_json_entry(entries, MANIFEST_PATH, |manifest| {
        manifest["nonsemanticExtensions"]
            .as_array_mut()
            .ok_or_else(|| std::io::Error::other("manifest nonsemanticExtensions is not an array"))?
            .push(Value::String(namespace));
        Ok(())
    })
}

fn apply_asset_mutation(entries: &mut BTreeMap<String, Vec<u8>>, mutation: &Value) -> TestResult {
    let name = required_str(mutation, "name")?;
    let path = format!("assets/{name}");
    entries.insert(
        path,
        declared_bytes(
            mutation
                .get("bytes")
                .ok_or_else(|| std::io::Error::other("asset mutation has no bytes"))?,
        )?,
    );
    let media_type = required_str(mutation, "mediaType")?.to_owned();
    mutate_json_entry(entries, MANIFEST_PATH, |manifest| {
        manifest["counts"]["assets"] = Value::from(1);
        manifest["assetMetadata"]
            .as_object_mut()
            .ok_or_else(|| std::io::Error::other("assetMetadata is not an object"))?
            .insert(
                name.to_owned(),
                json!({
                    "mediaType": media_type,
                    "redistributionPermitted": true
                }),
            );
        Ok(())
    })
}

fn apply_archive_mutation(
    entries: &mut BTreeMap<String, Vec<u8>>,
    scenario_path: &str,
    mutation: &Value,
) -> TestResult {
    match required_str(mutation, "operation")? {
        "add-entry" => {
            entries.insert(
                required_str(mutation, "path")?.to_owned(),
                declared_bytes(
                    mutation
                        .get("bytes")
                        .ok_or_else(|| std::io::Error::other("add-entry mutation has no bytes"))?,
                )?,
            );
        }
        "require-semantic-capability" => {
            apply_capability_mutation(entries, scenario_path, mutation)?;
        }
        "set-semantic-extension" => {
            apply_semantic_extension_mutation(entries, scenario_path, mutation)?;
        }
        "set-document-extension" => {
            apply_document_extension_mutation(entries, scenario_path, mutation)?;
        }
        "add-declared-asset" => {
            apply_asset_mutation(entries, mutation)?;
        }
        "recompute-checksums" => recompute_checksums(entries)?,
        operation => {
            return Err(
                std::io::Error::other(format!("unknown archive mutation {operation}")).into(),
            );
        }
    }
    Ok(())
}

fn materialize_scenario_export(
    input: &Value,
    portable: &ScenarioSnapshotV1,
) -> TestResult<Vec<u8>> {
    assert_eq!(
        input.get("portableFixture"),
        Some(&json!({
            "path": "../../../migration/fixtures/portable_v1_scenario.json",
            "jsonPointer": "/input"
        }))
    );
    let options = input
        .get("bundleOptions")
        .ok_or_else(|| std::io::Error::other("bundle case has no options"))?;
    assert_eq!(
        options.get("application"),
        Some(&json!({"name": "Eutheto", "version": "0.1.0"}))
    );
    let base = production_bundle(
        portable.clone(),
        required_str(options, "bundleId")?,
        required_str(options, "createdAt")?,
        required_str(options, "title")?,
    )?;
    let mut entries = archive_entries(&base)?;
    let scenario_path = format!("scenarios/{}.json", portable.document.scenario_id);
    // The released adversarial corpus exercises the genuine global-v1 representation.
    entries.insert(scenario_path.clone(), canonical_json(portable)?);
    mutate_json_entry(&mut entries, MANIFEST_PATH, |manifest| {
        manifest["schemaVersion"] = json!(1);
        manifest["requiredCapabilities"] = serde_json::to_value(&portable.required_capabilities)?;
        Ok(())
    })?;
    recompute_checksums(&mut entries)?;
    let mutations = input
        .get("mutations")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("bundle case has no mutations"))?;
    for mutation in mutations {
        apply_archive_mutation(&mut entries, &scenario_path, mutation)?;
    }
    standard_zip(
        entries.into_iter().map(|(path, bytes)| ArchiveEntry {
            path,
            bytes,
            unix_mode: 0o100_600,
        }),
        CompressionMethod::Stored,
    )
}

fn materialize_case(case: &Value, portable: &ScenarioSnapshotV1) -> TestResult<Vec<u8>> {
    let input = case
        .get("input")
        .ok_or_else(|| std::io::Error::other("adversarial case has no input"))?;
    match required_str(input, "archiveBuilder")? {
        "zip32-standard" => standard_zip(
            declared_archive_entries(input)?,
            declared_compression(input)?,
        ),
        "zip32-raw" => raw_stored_zip(&declared_archive_entries(input)?),
        "scenario-export-v1" => materialize_scenario_export(input, portable),
        builder => Err(std::io::Error::other(format!("unknown archive builder {builder}")).into()),
    }
}

fn assert_expected_error(error: ImportError, expected: &Value) -> TestResult {
    assert_eq!(expected.get("status"), Some(&json!("rejected")));
    assert_eq!(expected.get("authoritativeMutation"), Some(&json!(false)));
    let kind = required_str(expected, "errorKind")?;
    match (kind, error) {
        ("UnsafePath", ImportError::UnsafePath { path, reason }) => {
            assert_eq!(path, required_str(expected, "path")?);
            assert_eq!(reason, required_str(expected, "reason")?);
        }
        ("CaseCollision", ImportError::CaseCollision(path))
        | ("DuplicatePath", ImportError::DuplicatePath(path))
        | ("UndeclaredEntry", ImportError::UndeclaredEntry(path))
        | ("ChecksumMismatch", ImportError::ChecksumMismatch(path))
        | ("NonRegularEntry", ImportError::NonRegularEntry { path })
        | ("ProhibitedContent", ImportError::ProhibitedContent { path })
        | ("CompressionRatio", ImportError::CompressionRatio { path }) => {
            assert_eq!(path, required_str(expected, "path")?);
        }
        ("UnsupportedCapability", ImportError::UnsupportedCapability { id, version }) => {
            assert_eq!(id, required_str(expected, "capabilityId")?);
            assert_eq!(
                u64::from(version),
                required_u64(expected, "capabilityVersion")?
            );
        }
        ("InvalidManifest", ImportError::InvalidManifest(_)) => {}
        (expected_kind, actual) => {
            return Err(std::io::Error::other(format!(
                "expected {expected_kind}, received {actual:?}"
            ))
            .into());
        }
    }
    if kind == "CompressionRatio" {
        assert_eq!(
            required_u64(expected, "maximumRatio")?,
            PORTABLE_LIMITS.max_compression_ratio
        );
    }
    if kind == "ChecksumMismatch" {
        assert_eq!(required_str(expected, "algorithm")?, CHECKSUM_ALGORITHM);
    }
    Ok(())
}

#[test]
fn current_and_unknown_newer_portable_wrappers_are_executable_contracts() -> TestResult {
    let legacy = fixture(PORTABLE_V1, "eutheto.test/portable-scenario-fixture")?;
    let current = fixture(PORTABLE_V2, "eutheto.test/portable-scenario-fixture")?;
    let portable = portable_input(&legacy)?;
    let bundle = production_bundle(
        portable.clone(),
        "018f47f2-e880-7000-8000-000000000300",
        "2026-08-29T00:00:00Z",
        "Deterministic fixture",
    )?;
    let scenario_path = format!("scenarios/{}.json", portable.document.scenario_id);
    let mut entries = archive_entries(&bundle)?;
    let current_wire: Value = serde_json::from_slice(
        entries
            .get(&scenario_path)
            .ok_or("current scenario entry is missing")?,
    )?;
    assert_eq!(current_wire, current["input"]);
    let inspected = inspect_bundle(
        &bundle,
        &InspectionPolicy::default(),
        &MigrationRegistries::current_only(),
        &portable_decode::decode_fixture_domain,
    )?;
    assert_eq!(inspected.scenarios, vec![portable.clone()]);
    assert_eq!(inspected.scenarios[0].revision, Revision::new(7));
    assert_eq!(
        inspected.scenarios[0].document.extensions,
        BTreeMap::from([(
            "vendor.example".to_owned(),
            json!({"opaque": [1, "two", true]})
        )])
    );
    assert_eq!(
        inspected.manifest.nonsemantic_extensions,
        ["vendor.example".to_owned()].into_iter().collect()
    );
    assert!(inspected.applied_migrations.is_empty());
    entries.insert(scenario_path.clone(), canonical_json(&legacy["input"])?);
    mutate_json_entry(&mut entries, MANIFEST_PATH, |manifest| {
        manifest["schemaVersion"] = json!(1);
        manifest["requiredCapabilities"] = legacy["input"]["requiredCapabilities"].clone();
        Ok(())
    })?;
    recompute_checksums(&mut entries)?;
    let legacy_bundle = standard_zip(
        entries.into_iter().map(|(path, bytes)| ArchiveEntry {
            path,
            bytes,
            unix_mode: 0o100_600,
        }),
        CompressionMethod::Stored,
    )?;
    let migrated = inspect_supported(&legacy_bundle, &InspectionPolicy::default())?;
    assert_eq!(migrated.scenarios, vec![portable]);
    assert_eq!(migrated.original_schema_version, 1);
    assert_eq!(migrated.manifest.schema_version, 2);
    assert_eq!(
        migrated
            .applied_migrations
            .iter()
            .map(|step| (step.from_version, step.to_version, step.subject.as_ref()))
            .collect::<Vec<_>>(),
        vec![(1, 2, None)],
    );

    let newer = fixture(
        PORTABLE_UNKNOWN_NEWER,
        "eutheto.test/portable-scenario-fixture",
    )?;
    let mut entries = archive_entries(&bundle)?;
    entries.insert(
        scenario_path,
        canonical_json(
            newer
                .get("input")
                .ok_or_else(|| std::io::Error::other("newer fixture has no input"))?,
        )?,
    );
    mutate_json_entry(&mut entries, MANIFEST_PATH, |manifest| {
        manifest["schemaVersion"] = json!(3);
        Ok(())
    })?;
    recompute_checksums(&mut entries)?;
    let newer_bundle = standard_zip(
        entries.into_iter().map(|(path, bytes)| ArchiveEntry {
            path,
            bytes,
            unix_mode: 0o100_600,
        }),
        CompressionMethod::Stored,
    )?;
    let refusal = inspect_supported(&newer_bundle, &InspectionPolicy::default());
    assert!(matches!(
        refusal,
        Err(ImportError::UnsupportedNewerVersion {
            space: VersionSpace::PortableSchema,
            found: 3,
            current: 2
        })
    ));
    Ok(())
}

#[test]
fn every_declarative_adversarial_case_reaches_its_typed_refusal() -> TestResult {
    let wrapper = fixture(
        ADVERSARIAL_CASES,
        "eutheto.test/archive-adversarial-fixture",
    )?;
    assert_eq!(
        wrapper.get("expectedOutcome"),
        Some(&json!({
            "status": "all-cases-rejected",
            "caseCount": 10,
            "authoritativeMutation": false
        }))
    );
    assert_eq!(
        wrapper.get("inputContract"),
        Some(&json!({
            "bundleFormat": "eutheto-bundle",
            "bundleFormatVersion": 1,
            "portableFormat": "eutheto/scenario",
            "portableSchemaVersion": 1,
            "archiveFormat": "zip32"
        }))
    );
    assert_eq!(
        wrapper.get("centralLimits"),
        Some(&json!({
            "maxArchiveBytes": PORTABLE_LIMITS.max_archive_bytes,
            "maxTotalUncompressedBytes": PORTABLE_LIMITS.max_total_uncompressed_bytes,
            "maxEntryBytes": PORTABLE_LIMITS.max_entry_bytes,
            "maxEntries": PORTABLE_LIMITS.max_entries,
            "maxCompressionRatio": PORTABLE_LIMITS.max_compression_ratio,
            "maxPathBytes": PORTABLE_LIMITS.max_path_bytes,
            "maxJsonBytes": PORTABLE_LIMITS.max_json_bytes,
            "maxJsonDepth": PORTABLE_LIMITS.max_json_depth,
            "maxStringBytes": PORTABLE_LIMITS.max_string_bytes,
            "maxCollectionItems": PORTABLE_LIMITS.max_collection_items
        }))
    );
    assert_eq!(
        wrapper.get("defaultInspectionOptions"),
        Some(&json!({"limits": "centralLimits", "supportedCapabilities": {}}))
    );

    let portable_wrapper = fixture(PORTABLE_V1, "eutheto.test/portable-scenario-fixture")?;
    let portable = portable_input(&portable_wrapper)?;
    let cases = wrapper
        .get("cases")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("adversarial fixture has no cases"))?;
    assert_eq!(cases.len(), 10);
    for case in cases {
        let id = Uuid::parse_str(required_str(case, "id")?)?;
        assert_eq!(id.get_version_num(), 7);
        let bytes = materialize_case(case, &portable)?;
        let result = inspect_supported(&bytes, &InspectionPolicy::default());
        let Err(error) = result else {
            return Err(std::io::Error::other(format!(
                "{} unexpectedly passed inspection",
                required_str(case, "name")?
            ))
            .into());
        };
        assert_expected_error(
            error,
            case.get("expectedOutcome")
                .ok_or_else(|| std::io::Error::other("case has no expected outcome"))?,
        )?;
    }
    Ok(())
}

fn replace_scenario_extension_with_tiny_array(
    entries: &mut BTreeMap<String, Vec<u8>>,
    path: &str,
    count: usize,
) -> TestResult {
    let mut scenario: Value = serde_json::from_slice(
        entries
            .get(path)
            .ok_or_else(|| std::io::Error::other(format!("missing {path}")))?,
    )?;
    scenario["extensions"]["vendor.mass"] = json!("__TINY_ARRAY__");
    let encoded = String::from_utf8(canonical_json(&scenario)?)?;
    let mut tiny = String::with_capacity(
        count
            .checked_mul(2)
            .ok_or_else(|| std::io::Error::other("tiny-array capacity overflow"))?,
    );
    for index in 0..count {
        if index != 0 {
            tiny.push(',');
        }
        tiny.push('0');
    }
    let encoded = encoded.replace(
        "\"vendor.mass\":\"__TINY_ARRAY__\"",
        &format!("\"vendor.mass\":[{tiny}]"),
    );
    entries.insert(path.to_owned(), encoded.into_bytes());
    Ok(())
}

#[test]
fn unopened_preflight_streams_unknown_domain_without_constructing_it() -> TestResult {
    let wrapper = fixture(PORTABLE_V1, "eutheto.test/portable-scenario-fixture")?;
    let bundle = production_bundle(
        portable_input(&wrapper)?,
        "018f1e2d-3c4b-7a69-8def-0123456789aa",
        "2026-08-31T00:00:00Z",
        "Streaming preflight",
    )?;
    let mut entries = archive_entries(&bundle)?;
    let scenario_path = entries
        .keys()
        .find(|path| path.starts_with("scenarios/"))
        .cloned()
        .ok_or("fixture scenario is missing")?;
    let mut scenario: Value = serde_json::from_slice(
        entries
            .get(&scenario_path)
            .ok_or("fixture scenario is missing")?,
    )?;
    scenario["domain"]["packId"] = json!("vendor.unknown");
    scenario["domain"]["payload"] = json!("__SKIPPED_DOMAIN__");
    let encoded = String::from_utf8(canonical_json(&scenario)?)?;
    let padding = std::iter::repeat_n("null", 100_000)
        .collect::<Vec<_>>()
        .join(",");
    let hostile_domain =
        format!("\"payload\":{{\"entities\":[],\"entities\":{{}},\"padding\":[{padding}]}}");
    let encoded = encoded.replace("\"payload\":\"__SKIPPED_DOMAIN__\"", &hostile_domain);
    assert!(encoded.contains("\"entities\":[],\"entities\":{}"));
    entries.insert(scenario_path.clone(), encoded.into_bytes());
    recompute_checksums(&mut entries)?;
    let bytes = standard_zip(
        entries.into_iter().map(|(path, bytes)| ArchiveEntry {
            path,
            bytes,
            unix_mode: 0o100_600,
        }),
        CompressionMethod::Stored,
    )?;

    let metadata = preflight_bundle_metadata(&bytes, &InspectionPolicy::default())?;
    assert_eq!(metadata.scenarios.len(), 1);
    assert_eq!(
        metadata.scenarios[0].pack_id.as_deref(),
        Some("vendor.unknown")
    );
    let unopened =
        inspect_unopened_bundle_for_exact_reexport(&bytes, &InspectionPolicy::default())?;
    assert_eq!(unopened.metadata(), &metadata);
    assert_eq!(unopened.exact_bytes(), bytes.as_slice());
    assert!(unopened.retained_memory_bytes() >= bytes.len());
    let full_inspection = inspect_bundle(
        &bytes,
        &InspectionPolicy::default(),
        &MigrationRegistries::current_only(),
        &portable_decode::decode_fixture_domain,
    );
    assert!(
        matches!(
            full_inspection,
            Err(ImportError::InvalidJson { .. } | ImportError::InvalidManifest(_))
        ),
        "{full_inspection:?}"
    );
    Ok(())
}

#[test]
fn hostile_capability_identifier_is_never_reflected_by_public_errors() -> TestResult {
    let wrapper = fixture(PORTABLE_V1, "eutheto.test/portable-scenario-fixture")?;
    let bundle = production_bundle(
        portable_input(&wrapper)?,
        "018f1e2d-3c4b-7a69-8def-0123456789ab",
        "2026-08-31T00:00:00Z",
        "Capability diagnostics",
    )?;
    let mut entries = archive_entries(&bundle)?;
    mutate_json_entry(&mut entries, MANIFEST_PATH, |manifest| {
        manifest["requiredCapabilities"] =
            json!([{ "id": "vendor.bad\n\u{1b}]0;owned\u{7}", "version": 1 }]);
        Ok(())
    })?;
    recompute_checksums(&mut entries)?;
    let bytes = standard_zip(
        entries.into_iter().map(|(path, bytes)| ArchiveEntry {
            path,
            bytes,
            unix_mode: 0o100_600,
        }),
        CompressionMethod::Deflated,
    )?;
    let Err(error) = preflight_bundle_metadata(&bytes, &InspectionPolicy::default()) else {
        return Err("hostile capability namespace must be rejected".into());
    };
    let diagnostic = error.to_string();
    assert!(!diagnostic.contains("vendor.bad"));
    assert!(!diagnostic.contains('\n'));
    assert!(!diagnostic.contains('\u{1b}'));
    Ok(())
}

#[test]
fn known_current_pack_rejects_compact_multi_million_tiny_nodes_before_value_growth() -> TestResult {
    let wrapper = fixture(PORTABLE_V1, "eutheto.test/portable-scenario-fixture")?;
    let bundle = production_bundle(
        portable_input(&wrapper)?,
        "018f1e2d-3c4b-7a69-8def-0123456789ac",
        "2026-08-31T00:00:00Z",
        "Aggregate nodes",
    )?;
    let mut entries = archive_entries(&bundle)?;
    let scenario_path = entries
        .keys()
        .find(|path| path.starts_with("scenarios/"))
        .cloned()
        .ok_or("fixture scenario is missing")?;
    replace_scenario_extension_with_tiny_array(&mut entries, &scenario_path, 2_000_000)?;
    recompute_checksums(&mut entries)?;
    let bytes = standard_zip(
        entries.into_iter().map(|(path, bytes)| ArchiveEntry {
            path,
            bytes,
            unix_mode: 0o100_600,
        }),
        CompressionMethod::Stored,
    )?;
    assert!(matches!(
        inspect_bundle(
            &bytes,
            &InspectionPolicy::default(),
            &MigrationRegistries::current_only(),
            &portable_decode::decode_fixture_domain,
        ),
        Err(ImportError::InvalidManifest(_))
    ));
    Ok(())
}

#[test]
fn cumulative_multi_entry_representation_is_rejected_incrementally() -> TestResult {
    let wrapper = fixture(PORTABLE_V1, "eutheto.test/portable-scenario-fixture")?;
    let current = portable_input(&wrapper)?;
    let mut historical = current.clone();
    historical.revision = Revision::new(6);
    let bundle = assemble_scenario_export(
        &ScenarioExportSnapshot {
            bundle_id: "018f1e2d-3c4b-7a69-8def-0123456789ad".parse::<BundleId>()?,
            created_at: "2026-08-31T00:00:00Z".to_owned(),
            application: ApplicationMetadata {
                name: "Eutheto".to_owned(),
                version: "0.1.0".to_owned(),
            },
            title: "Cumulative representation".to_owned(),
            scenario: current,
            scenario_revisions: vec![historical],
            sections: BackupSections::default(),
            nonsemantic_extensions: BTreeSet::from(["vendor.mass".to_owned()]),
            manifest_extensions: BTreeMap::new(),
        },
        &portable_encode::encode_fixture_domain,
    )?;
    let mut entries = archive_entries(&bundle)?;
    let scenario_paths = entries
        .keys()
        .filter(|path| path.starts_with("scenarios/") || path.starts_with("scenario-revisions/"))
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(scenario_paths.len(), 2);
    for path in &scenario_paths {
        replace_scenario_extension_with_tiny_array(&mut entries, path, 400_000)?;
    }
    recompute_checksums(&mut entries)?;
    let bytes = standard_zip(
        entries.into_iter().map(|(path, bytes)| ArchiveEntry {
            path,
            bytes,
            unix_mode: 0o100_600,
        }),
        CompressionMethod::Stored,
    )?;
    let inspection = inspect_bundle(
        &bytes,
        &InspectionPolicy::default(),
        &MigrationRegistries::current_only(),
        &portable_decode::decode_fixture_domain,
    );
    assert!(
        matches!(
            inspection,
            Err(ImportError::JsonLimit {
                limit: "representation bytes",
                ..
            })
        ),
        "{inspection:?}"
    );
    Ok(())
}
