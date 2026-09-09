use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result, ensure};
use eutheto_core::HeadlessService;
use eutheto_types::{AppError, FixedClock, FixedIdGenerator, SystemMonotonicClock};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::contract::{
    CORPUS_FORMAT, CORPUS_PATH, CORPUS_VERSION, CaseId, CaseManifest, CorpusManifest,
    EXPECTED_FORMAT, EXPECTED_PATH, ExpectedCase, ExpectedCaseBinding, ExpectedCorpus,
    ExpectedDisposition, FIXTURE_CLOCK, FIXTURE_ROOT, FORMAT_VERSION, FileBinding,
    GeneratorIdentity, LargeParameters, LoadedCorpus, LoadedFixture, MAX_EXPECTED_BYTES,
    MAX_INDEX_BYTES, MeasurementProfile, REPAIR_PATH, RepairFixture,
};
use crate::{cases, files, schemas};

include!(concat!(env!("OUT_DIR"), "/source_inventory.rs"));
const RUNNER_ROOT: &str = "benchmarks/workforce-runner";

pub(crate) fn app<T>(result: Result<T, AppError>) -> Result<T> {
    result
        .map_err(|error| anyhow::anyhow!("application operation rejected: {}", error_code(&error)))
}

pub(crate) fn error_code(error: &AppError) -> &str {
    match error {
        AppError::Protocol(failure) => &failure.code,
        AppError::Solver(failure) => &failure.code,
        AppError::Verification(failure) => &failure.code,
        AppError::Storage(failure) => &failure.code,
        AppError::Ai(failure) => &failure.code,
        AppError::Validation(_) => "application.validation",
        AppError::Conflict { .. } => "application.conflict",
        AppError::NotFound(_) => "application.not_found",
        AppError::Unsupported(_) => "application.unsupported",
        AppError::Internal { .. } => "application.internal",
    }
}

pub(crate) fn codec() -> Result<HeadlessService> {
    app(HeadlessService::new(
        Arc::new(FixedClock::new(FIXTURE_CLOCK.parse()?)),
        Arc::new(SystemMonotonicClock::new()),
        Arc::new(FixedIdGenerator::new(std::iter::empty::<uuid::Uuid>())),
    ))
}

fn collect_sources(root: &Path, directory: &Path, paths: &mut BTreeSet<String>) -> Result<()> {
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        ensure!(!kind.is_symlink(), "symlink runner source");
        if kind.is_dir() {
            collect_sources(root, &entry.path(), paths)?;
        } else if entry
            .path()
            .extension()
            .is_some_and(|extension| extension == "rs")
        {
            paths.insert(
                entry
                    .path()
                    .strip_prefix(root)?
                    .to_str()
                    .context("non-UTF8 source path")?
                    .replace('\\', "/"),
            );
        }
    }
    Ok(())
}

/// Exact embedded source bytes, sorted paths and u64 big-endian length framing.
pub(crate) fn identity(root: &Path) -> Result<GeneratorIdentity> {
    let runner = root.join(RUNNER_ROOT);
    let mut disk_paths = BTreeSet::from(["Cargo.toml".to_owned(), "build.rs".to_owned()]);
    collect_sources(&runner, &runner.join("src"), &mut disk_paths)?;
    ensure!(
        disk_paths
            .iter()
            .map(String::as_str)
            .eq(BUILD_SOURCES.iter().map(|(path, _)| *path)),
        "runner source inventory differs from built executable"
    );
    let mut hash = Sha256::new();
    hash.update(b"eutheto/workforce-generator-source/v1\0");
    let mut sources = Vec::with_capacity(BUILD_SOURCES.len());
    for (relative, embedded) in BUILD_SOURCES {
        let path = format!("{RUNNER_ROOT}/{relative}");
        let current = files::read_bounded(&files::contained(root, &path)?, 4 * 1024 * 1024)?;
        ensure!(
            current == *embedded,
            "runner source bytes differ from built executable: {relative}"
        );
        hash.update(u64::try_from(path.len())?.to_be_bytes());
        hash.update(path.as_bytes());
        hash.update(u64::try_from(embedded.len())?.to_be_bytes());
        hash.update(embedded);
        sources.push(binding(path, embedded)?);
    }
    Ok(GeneratorIdentity {
        method: "role-keyed-uuidv7-fixed-clock;source-sha256-framing-v1".to_owned(),
        source_sha256: format!("{:x}", hash.finalize()),
        sources,
        seed: 0,
        license: "Apache-2.0".to_owned(),
    })
}

fn binding(path: String, bytes: &[u8]) -> Result<FileBinding> {
    Ok(FileBinding {
        path,
        sha256: files::sha256(bytes),
        bytes: u64::try_from(bytes.len())?,
    })
}

fn json(value: &impl Serialize) -> Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    Ok(bytes)
}

pub(crate) fn outputs(root: &Path) -> Result<BTreeMap<String, Vec<u8>>> {
    let generator = identity(root)?;
    let definitions = cases::build()?;
    ensure!(
        definitions
            .fixtures
            .iter()
            .map(|case| case.id)
            .eq(CaseId::ALL),
        "constructor case inventory changed"
    );
    let codec = codec()?;
    let control = files::control();
    let mut output = BTreeMap::new();
    let mut manifests = Vec::new();
    let mut expectations = Vec::new();
    for fixture in &definitions.fixtures {
        cases::validate_semantics(fixture.id, &fixture.snapshot, &fixture.expected)?;
        let bytes = app(codec.encode_scenario(&fixture.snapshot, &control))?;
        let roundtrip = app(codec.decode_scenario(&bytes, &control))?;
        ensure!(
            roundtrip.scenario == fixture.snapshot,
            "portable generation changed source meaning"
        );
        let input = binding(fixture.id.fixture_path(), &bytes)?;
        expectations.push(ExpectedCaseBinding {
            input_sha256: input.sha256.clone(),
            expected: fixture.expected.clone(),
        });
        manifests.push(CaseManifest {
            id: fixture.id,
            family: fixture.family,
            workload_class: fixture.workload_class,
            input,
        });
        output.insert(fixture.id.fixture_path(), bytes);
    }
    let before = definitions
        .fixtures
        .iter()
        .find(|case| case.id == CaseId::RepairBefore)
        .context("repair before case absent")?;
    cases::validate_repair(&definitions.repair, &before.snapshot)?;
    let repair = json(&definitions.repair)?;
    let expected = json(&ExpectedCorpus {
        format: EXPECTED_FORMAT.to_owned(),
        schema_version: FORMAT_VERSION,
        corpus_version: CORPUS_VERSION,
        generator_source_sha256: generator.source_sha256.clone(),
        cases: expectations,
    })?;
    let manifest = CorpusManifest {
        format: CORPUS_FORMAT.to_owned(),
        schema_version: FORMAT_VERSION,
        corpus_version: CORPUS_VERSION,
        generator,
        fixture_clock: FIXTURE_CLOCK.to_owned(),
        profile: MeasurementProfile::REVIEWED,
        cases: manifests,
        expectations: binding(EXPECTED_PATH.to_owned(), &expected)?,
        repair: binding(REPAIR_PATH.to_owned(), &repair)?,
    };
    output.insert(CORPUS_PATH.to_owned(), json(&manifest)?);
    output.insert(EXPECTED_PATH.to_owned(), expected);
    output.insert(REPAIR_PATH.to_owned(), repair);
    for (name, schema) in schemas::documents() {
        output.insert(
            format!("benchmarks/corpus/workforce/{name}.schema.json"),
            json(&schema)?,
        );
    }
    Ok(output)
}

pub(crate) fn generate(root: &Path, check: bool) -> Result<()> {
    let output = outputs(root)?;
    for (relative, bytes) in output {
        if check {
            let current = files::read_bounded(
                &files::contained(root, &relative)?,
                eutheto_export::PORTABLE_LIMITS.max_json_bytes,
            )?;
            ensure!(
                current == bytes,
                "generated Workforce corpus drift: {relative}"
            );
        } else {
            let path = files::generated_destination(root, &relative)?;
            let mut staged = tempfile::NamedTempFile::new_in(
                path.parent().context("missing generated parent")?,
            )?;
            staged.write_all(&bytes)?;
            staged.flush()?;
            // Replacing this owned generated leaf avoids modifying any outside hard-link target.
            // Benchmark reports instead use the existing no-clobber application publication.
            staged.persist(path)?;
        }
    }
    check_fixture_inventory(root)
}

pub(crate) fn bound_bytes(
    root: &Path,
    binding: &FileBinding,
    expected_path: &str,
    limit: u64,
) -> Result<Vec<u8>> {
    ensure!(
        binding.path == expected_path && binding.bytes <= limit,
        "unexpected corpus file binding"
    );
    files::digest(&binding.sha256)?;
    let bytes = files::read_bounded(&files::contained(root, expected_path)?, limit)?;
    ensure!(
        u64::try_from(bytes.len())? == binding.bytes && files::sha256(&bytes) == binding.sha256,
        "corpus file hash/size mismatch"
    );
    Ok(bytes)
}

fn check_fixture_inventory(root: &Path) -> Result<()> {
    let directory = files::contained(root, FIXTURE_ROOT)?;
    let mut actual = BTreeSet::new();
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        ensure!(
            entry.file_type()?.is_file(),
            "fixture inventory contains a nonregular entry"
        );
        actual.insert(
            entry
                .file_name()
                .into_string()
                .map_err(|_| anyhow::anyhow!("non-UTF8 fixture filename"))?,
        );
    }
    let mut expected: BTreeSet<_> = CaseId::ALL
        .into_iter()
        .map(|id| format!("{}.json", id.slug()))
        .collect();
    expected.insert("repair-callout.json".to_owned());
    ensure!(actual == expected, "unindexed or missing Workforce fixture");
    Ok(())
}

pub(crate) fn load(root: &Path) -> Result<LoadedCorpus> {
    let source = identity(root)?;
    check_fixture_inventory(root)?;
    let bytes = files::read_bounded(&files::contained(root, CORPUS_PATH)?, MAX_INDEX_BYTES)?;
    let manifest: CorpusManifest = serde_json::from_slice(&bytes)?;
    ensure!(
        manifest.format == CORPUS_FORMAT
            && manifest.schema_version == FORMAT_VERSION
            && manifest.corpus_version == CORPUS_VERSION,
        "unsupported corpus version"
    );
    ensure!(
        manifest.generator == source
            && manifest.fixture_clock == FIXTURE_CLOCK
            && manifest.profile == MeasurementProfile::REVIEWED,
        "corpus generation/profile identity mismatch"
    );
    ensure!(
        manifest.cases.iter().map(|case| case.id).eq(CaseId::ALL),
        "duplicate, missing or extra corpus cases"
    );
    let expected_bytes = bound_bytes(
        root,
        &manifest.expectations,
        EXPECTED_PATH,
        MAX_EXPECTED_BYTES,
    )?;
    let expected: ExpectedCorpus = serde_json::from_slice(&expected_bytes)?;
    ensure!(
        expected.format == EXPECTED_FORMAT
            && expected.schema_version == FORMAT_VERSION
            && expected.corpus_version == CORPUS_VERSION
            && expected.generator_source_sha256 == source.source_sha256,
        "unsupported or mismatched expectation identity"
    );
    ensure!(
        expected
            .cases
            .iter()
            .map(|case| case.expected.id)
            .eq(CaseId::ALL),
        "duplicate, missing or extra expectation cases"
    );
    let codec = codec()?;
    let control = files::control();
    let mut fixtures = Vec::with_capacity(CaseId::ALL.len());
    for (case, expected) in manifest.cases.iter().zip(expected.cases) {
        ensure!(
            expected.input_sha256 == case.input.sha256,
            "expectation binds another input"
        );
        validate_expected(&expected.expected)?;
        let bytes = bound_bytes(
            root,
            &case.input,
            &case.id.fixture_path(),
            eutheto_export::PORTABLE_LIMITS.max_json_bytes,
        )?;
        let snapshot = app(codec.decode_scenario(&bytes, &control))?.scenario;
        cases::validate_semantics(case.id, &snapshot, &expected.expected)?;
        ensure!(
            app(codec.encode_scenario(&snapshot, &control))? == bytes,
            "noncanonical corpus input"
        );
        fixtures.push(LoadedFixture {
            manifest: case.clone(),
            expected: expected.expected,
            bytes,
            snapshot,
        });
    }
    let repair: RepairFixture = serde_json::from_slice(&bound_bytes(
        root,
        &manifest.repair,
        REPAIR_PATH,
        MAX_EXPECTED_BYTES,
    )?)?;
    let before = fixtures
        .iter()
        .find(|case| case.manifest.id == CaseId::RepairBefore)
        .context("repair before case absent")?;
    cases::validate_repair(&repair, &before.snapshot)?;
    Ok(LoadedCorpus {
        manifest,
        manifest_sha256: files::sha256(&bytes),
        fixtures,
    })
}

fn validate_expected(expected: &ExpectedCase) -> Result<()> {
    ensure!(
        (1..=512).contains(&expected.people)
            && (1..=366).contains(&expected.horizon_days)
            && (1..=512).contains(&expected.resolved_shifts),
        "invalid expected dimensions"
    );
    ensure!(
        expected.semantics.len() <= 64
            && expected
                .semantics
                .iter()
                .all(|value| !value.is_empty() && value.len() <= 1024),
        "invalid semantic descriptions"
    );
    ensure!(
        expected.deferred_obligations.len() <= 64,
        "excess deferred obligations"
    );
    if let Some(score) = expected.score {
        ensure!(
            score.feasibility == 0
                && score.minimum_stable_rank >= 0
                && score.minimum_stable_rank <= score.maximum_stable_rank,
            "invalid score expectation"
        );
    }
    if let Some(model) = expected.model_envelope {
        ensure!(
            model.minimum_variables <= model.maximum_variables
                && model.maximum_variables > 0
                && model.maximum_constraints > 0,
            "invalid model envelope"
        );
    }
    ensure!(
        (expected.disposition == ExpectedDisposition::Accepted)
            == (expected.selected_assignments.is_some() && expected.score.is_some()),
        "acceptance expectation incomplete"
    );
    Ok(())
}

pub(crate) fn synthetic(root: &Path, parameters: LargeParameters, output: &Path) -> Result<()> {
    identity(root)?;
    let snapshot = cases::build_large(parameters)?;
    let codec = codec()?;
    let bytes = app(codec.encode_scenario(&snapshot, &files::control()))?;
    ensure!(
        app(codec.decode_scenario(&bytes, &files::control()))?.scenario == snapshot,
        "synthetic portable roundtrip changed meaning"
    );
    let value: serde_json::Value = serde_json::from_slice(&bytes)?;
    files::publish_json(output, &value)
}

#[cfg(test)]
#[path = "generation_tests.rs"]
mod tests;
