// SPDX-License-Identifier: Apache-2.0
//! Canonical recipient output contract. No desktop transport imports enter the report renderer.

use anyhow::{Context, Result, bail};
use base64::Engine as _;
use eutheto_types::{
    MAX_RECIPIENT_HTML_BYTES, MAX_RECIPIENT_PRESENTATION_COST, MAX_RECIPIENT_REPORT_BYTES,
    MAX_RECIPIENT_REPORT_ROWS, RECIPIENT_REPORT_SCHEMA_VERSION, REVISION_MAX_V1,
};
use serde_json::{Map, Value, json};
use sha2::{Digest as _, Sha256};
use std::fmt::Write as _;
use std::path::Path;
use std::process::Command;

const TYPESCRIPT_PATH: &str = "apps/desktop/src/api/generated-recipient-report.ts";
const SCHEMA_PATH: &str = "schemas/generated/recipient-report.schema.json";

fn reference(name: &str) -> Value {
    json!({"$ref": format!("#/$defs/{name}")})
}
fn text() -> Value {
    json!({"type": "string", "maxLength": MAX_RECIPIENT_REPORT_BYTES})
}
fn integer(minimum: i64, maximum: i64) -> Value {
    json!({"type": "integer", "minimum": minimum, "maximum": maximum})
}
fn array(name: &str) -> Value {
    json!({"type": "array", "maxItems": MAX_RECIPIENT_REPORT_ROWS, "items": reference(name)})
}
fn nullable(schema: Value) -> Value {
    Value::Object(Map::from_iter([(
        "oneOf".to_owned(),
        Value::Array(vec![schema, json!({"type": "null"})]),
    )]))
}
fn choices(values: &[&str]) -> Value {
    json!({"oneOf": values.iter().map(|value| json!({"const": value})).collect::<Vec<_>>()})
}
fn object(fields: &[(&str, Value)]) -> Value {
    let properties: Map<String, Value> = fields
        .iter()
        .map(|(key, value)| ((*key).to_owned(), value.clone()))
        .collect();
    json!({"type": "object", "additionalProperties": false, "required": properties.keys().collect::<Vec<_>>(), "properties": properties})
}

#[allow(clippy::too_many_lines)]
fn definitions() -> Map<String, Value> {
    let mut definitions = Map::new();
    let mut define = |name: &str, schema| {
        definitions.insert(name.to_owned(), schema);
    };
    define(
        "RecipientId",
        json!({"type": "string", "format": "uuid", "minLength": 36, "maxLength": 36}),
    );
    define(
        "RecipientRevision",
        integer(0, REVISION_MAX_V1.cast_signed()),
    );
    define(
        "RecipientLabelV1",
        json!({"oneOf": [
            object(&[("kind", json!({"const": "generated"})), ("text", text())]),
            object(&[("kind", json!({"const": "authored"})), ("text", text())]),
        ]}),
    );
    define(
        "RecipientReportOptionsV1",
        object(&[
            ("includePersonNames", json!({"type": "boolean"})),
            ("includeAuthoredLabels", json!({"type": "boolean"})),
            ("includeStaffing", json!({"type": "boolean"})),
            ("includeChanges", json!({"type": "boolean"})),
            ("includeFairnessActuals", json!({"type": "boolean"})),
        ]),
    );
    define(
        "RecipientRendererV1",
        object(&[
            ("version", integer(1, u32::MAX.into())),
            ("javascriptSha256", text()),
            ("cssSha256", text()),
        ]),
    );
    define(
        "RecipientSolverV1",
        object(&[("backendId", text()), ("recordedVersion", nullable(text()))]),
    );
    define(
        "RecipientSourceV1",
        object(&[
            ("scenarioId", reference("RecipientId")),
            ("scenarioRevision", reference("RecipientRevision")),
            ("solutionId", reference("RecipientId")),
            ("resultSchemaVersion", integer(1, u32::MAX.into())),
            ("acceptedResultChecksum", text()),
            ("verificationReportChecksum", text()),
            ("sourceDocumentHash", text()),
            (
                "origin",
                choices(&["storedTerminal", "storedCheckpoint", "verifiedFile"]),
            ),
            (
                "observedCurrentRevision",
                nullable(reference("RecipientRevision")),
            ),
            ("verification", json!({"const": "verifiedFeasible"})),
            ("solver", reference("RecipientSolverV1")),
        ]),
    );
    define(
        "RecipientResolvedLocalTime",
        object(&[
            ("instant", text()),
            ("local", text()),
            ("offsetSeconds", integer(i32::MIN.into(), i32::MAX.into())),
        ]),
    );
    define(
        "RecipientEndpointV1",
        object(&[
            ("resolved", reference("RecipientResolvedLocalTime")),
            ("actual", text()),
        ]),
    );
    define(
        "RecipientIntervalV1",
        object(&[
            ("startsAt", reference("RecipientEndpointV1")),
            ("endsAt", reference("RecipientEndpointV1")),
        ]),
    );
    define(
        "RecipientPersonV1",
        object(&[
            ("personId", reference("RecipientId")),
            ("label", reference("RecipientLabelV1")),
        ]),
    );
    define(
        "RecipientEntityV1",
        object(&[
            ("entityId", reference("RecipientId")),
            ("label", reference("RecipientLabelV1")),
        ]),
    );
    define(
        "RecipientShiftV1",
        object(&[
            ("shiftId", reference("RecipientId")),
            ("assignmentTypeId", reference("RecipientId")),
            ("locationId", nullable(reference("RecipientId"))),
            ("interval", reference("RecipientIntervalV1")),
            ("reportingDate", text()),
        ]),
    );
    define(
        "RecipientAssignmentV1",
        object(&[
            ("assignmentId", text()),
            ("personId", reference("RecipientId")),
            ("shiftId", reference("RecipientId")),
        ]),
    );
    define(
        "RecipientStaffingV1",
        object(&[
            ("shiftId", reference("RecipientId")),
            ("assignedCount", integer(0, u32::MAX.into())),
        ]),
    );
    define(
        "RecipientFractionV1",
        object(&[
            (
                "numerator",
                json!({"type": "string", "minLength": 1, "maxLength": 40}),
            ),
            (
                "denominator",
                json!({"type": "string", "minLength": 1, "maxLength": 39}),
            ),
        ]),
    );
    define(
        "RecipientActualV1",
        object(&[
            ("personId", reference("RecipientId")),
            ("actual", reference("RecipientFractionV1")),
        ]),
    );
    define(
        "RecipientActualPeriodV1",
        object(&[
            ("interval", reference("RecipientIntervalV1")),
            ("actuals", array("RecipientActualV1")),
        ]),
    );
    define(
        "RecipientActualGroupV1",
        object(&[
            (
                "measure",
                choices(&["assignmentCount", "elapsedMinutes", "scheduledMinutes"]),
            ),
            (
                "membership",
                choices(&["reportingDate", "startInstant", "intersection"]),
            ),
            ("overlap", choices(&["sum", "union"])),
            ("people", array("RecipientId")),
            ("qualifyingShifts", array("RecipientId")),
            ("periods", array("RecipientActualPeriodV1")),
        ]),
    );
    define(
        "RecipientOriginalV1",
        object(&[
            ("scenarioId", reference("RecipientId")),
            ("scenarioRevision", reference("RecipientRevision")),
            ("solutionId", reference("RecipientId")),
            ("acceptedResultChecksum", text()),
            (
                "admission",
                choices(&["freshlyReadmittedNative", "notEstablished"]),
            ),
        ]),
    );
    define(
        "RecipientHistoricalFactsV1",
        object(&[
            ("personLabel", reference("RecipientLabelV1")),
            ("assignmentType", reference("RecipientEntityV1")),
            ("location", nullable(reference("RecipientEntityV1"))),
            ("interval", reference("RecipientIntervalV1")),
            ("reportingDate", text()),
        ]),
    );
    define(
        "RecipientHistoricalEndpointV1",
        json!({"oneOf": [
            object(&[("availability", json!({"const": "available"})), ("facts", reference("RecipientHistoricalFactsV1"))]),
            object(&[("availability", json!({"const": "unavailable"}))]),
        ]}),
    );
    define(
        "RecipientChangeRowV1",
        object(&[
            ("personId", reference("RecipientId")),
            ("shiftId", reference("RecipientId")),
            ("membership", choices(&["added", "removed", "retained"])),
            (
                "previous",
                nullable(reference("RecipientHistoricalEndpointV1")),
            ),
        ]),
    );
    define(
        "RecipientChangesV1",
        object(&[
            (
                "basis",
                choices(&["recordedOriginal", "recordedManual", "externalStartingPlan"]),
            ),
            ("original", nullable(reference("RecipientOriginalV1"))),
            ("rows", array("RecipientChangeRowV1")),
        ]),
    );
    define(
        "WorkforceRecipientV1",
        object(&[
            ("people", array("RecipientPersonV1")),
            ("assignmentTypes", array("RecipientEntityV1")),
            ("locations", array("RecipientEntityV1")),
            ("shifts", array("RecipientShiftV1")),
            ("assignments", array("RecipientAssignmentV1")),
            ("staffing", nullable(array("RecipientStaffingV1"))),
            ("changes", nullable(array("RecipientChangesV1"))),
            ("fairnessActuals", nullable(array("RecipientActualGroupV1"))),
        ]),
    );
    define(
        "RecipientContentV1",
        object(&[
            ("kind", json!({"const": "workforce"})),
            ("schedule", reference("WorkforceRecipientV1")),
        ]),
    );
    define(
        "RecipientReportV1",
        object(&[
            ("format", json!({"const": "eutheto/share-result"})),
            (
                "schemaVersion",
                json!({"const": RECIPIENT_REPORT_SCHEMA_VERSION}),
            ),
            ("profile", json!({"const": "participant-friendly"})),
            ("generatedAt", text()),
            ("applicationVersion", text()),
            ("renderer", reference("RecipientRendererV1")),
            ("title", reference("RecipientLabelV1")),
            ("planVersion", text()),
            ("locale", text()),
            ("timeZone", text()),
            ("horizon", object(&[("start", text()), ("end", text())])),
            ("source", reference("RecipientSourceV1")),
            ("privacy", reference("RecipientReportOptionsV1")),
            ("content", reference("RecipientContentV1")),
        ]),
    );
    definitions
}

pub(crate) fn generated_files(
    repo_root: &Path,
) -> Result<Vec<crate::protocol_generate::GeneratedOutput>> {
    let definitions = definitions();
    let mut typescript = String::from(
        "// SPDX-License-Identifier: Apache-2.0\n// @generated by `cargo xtask generate` from xtask/src/recipient_generate.rs; do not edit.\n// Structural guards do not establish native acceptance or publication custody.\n",
    );
    writeln!(
        typescript,
        "export const MAX_RECIPIENT_REPORT_BYTES = {MAX_RECIPIENT_REPORT_BYTES};\nexport const MAX_RECIPIENT_REPORT_ROWS = {MAX_RECIPIENT_REPORT_ROWS};\nexport const MAX_RECIPIENT_HTML_BYTES = {MAX_RECIPIENT_HTML_BYTES};\nexport const MAX_RECIPIENT_PRESENTATION_COST = {MAX_RECIPIENT_PRESENTATION_COST};"
    )?;
    crate::phase02_generate::write_typescript_wire_helpers(&mut typescript, "");
    for (name, schema) in &definitions {
        writeln!(
            typescript,
            "export type {name} = {};",
            crate::phase02_generate::typescript_schema(schema, "", 0)?
        )?;
        writeln!(
            typescript,
            "export function is{name}(value: unknown): value is {name} {{ return {}; }}",
            crate::phase02_generate::typescript_guard(schema, "", "value", 0, None)?
        )?;
    }
    let schema = json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$comment": "Generated by cargo xtask generate from xtask/src/recipient_generate.rs; canonical recipient output, not native acceptance authority.",
        "$ref": "#/$defs/RecipientReportV1", "$defs": definitions,
    });
    let mut schema = serde_json::to_string_pretty(&schema)?;
    schema.push('\n');
    let typescript =
        crate::phase02_generate::format_typescript(repo_root, TYPESCRIPT_PATH, &typescript)?;
    let mut files = renderer_files(repo_root, &typescript)?;
    files.push((TYPESCRIPT_PATH.into(), typescript.into_bytes()));
    files.push((SCHEMA_PATH.into(), schema.into_bytes()));
    Ok(files)
}

#[allow(clippy::too_many_lines)] // Keep staged inputs, exact outputs and their digest custody together.
fn renderer_files(
    repo_root: &Path,
    guards: &str,
) -> Result<Vec<crate::protocol_generate::GeneratedOutput>> {
    let staging = tempfile::Builder::new()
        .prefix("eutheto-recipient-renderer-")
        .tempdir()?;
    let source = staging.path();
    std::fs::create_dir(source.join("api"))?;
    std::fs::write(source.join("api/generated-recipient-report.ts"), guards)?;
    for name in [
        "recipient-report.ts",
        "recipient-report-standalone.ts",
        "recipient-report.css",
    ] {
        std::fs::copy(
            repo_root.join("apps/desktop/src").join(name),
            source.join(name),
        )?;
    }
    let output = source.join("output");
    let config = json!({
        "root": source, "publicDir": false, "logLevel": "error",
        "css": {"postcss": {"plugins": []}},
        "build": {
            "outDir": output, "emptyOutDir": true, "target": "es2022", "sourcemap": false,
            "lib": {
                "entry": source.join("recipient-report-standalone.ts"),
                "name": "EuthetoRecipient", "formats": ["iife"], "cssFileName": "recipient-renderer"
            }
        }
    });
    let config_path = source.join("vite.config.mjs");
    std::fs::write(
        &config_path,
        format!(
            "const config = {config};\nconfig.build.lib.fileName = () => 'recipient-renderer.js';\nexport default config;\n"
        ),
    )?;
    let built = Command::new(if cfg!(windows) { "pnpm.cmd" } else { "pnpm" })
        .current_dir(repo_root.join("apps/desktop"))
        .args(["exec", "vite", "build", "--config"])
        .arg(&config_path)
        .output()
        .context("failed to start the pinned recipient renderer build")?;
    if !built.status.success() {
        bail!(
            "recipient renderer build failed: {}",
            String::from_utf8_lossy(&built.stderr)
        );
    }
    let mut names = std::fs::read_dir(&output)?
        .map(|entry| entry.map(|entry| entry.file_name()))
        .collect::<std::io::Result<Vec<_>>>()?;
    names.sort();
    if names
        != [
            std::ffi::OsString::from("recipient-renderer.css"),
            std::ffi::OsString::from("recipient-renderer.js"),
        ]
    {
        bail!("recipient renderer must produce exactly one JavaScript and one CSS artifact");
    }
    let mut files = Vec::new();
    let mut metadata = String::from(
        "// SPDX-License-Identifier: Apache-2.0\n// @generated by cargo xtask generate; do not edit.\n\npub const RENDERER_VERSION: u32 = 1;\n",
    );
    for (filename, symbol, terminator) in [
        (
            "recipient-renderer.js",
            "JAVASCRIPT",
            b"</script".as_slice(),
        ),
        ("recipient-renderer.css", "CSS", b"</style".as_slice()),
    ] {
        let bytes = std::fs::read(output.join(filename))?;
        if bytes
            .windows(terminator.len())
            .any(|window| window.eq_ignore_ascii_case(terminator))
        {
            bail!("recipient renderer artifact contains an unsafe HTML container terminator");
        }
        let digest = Sha256::digest(&bytes);
        for (suffix, value) in [
            ("SHA256", crate::hex_bytes(&digest)),
            (
                "CSP",
                format!(
                    "sha256-{}",
                    base64::engine::general_purpose::STANDARD.encode(digest)
                ),
            ),
        ] {
            let declaration = format!("pub const {symbol}_{suffix}: &str =");
            let value = format!("{value:?};");
            // Match the existing generator's rustfmt 100-column constant layout.
            if declaration.len() + 1 + value.len() > 100 {
                writeln!(metadata, "{declaration}\n    {value}")?;
            } else {
                writeln!(metadata, "{declaration} {value}")?;
            }
        }
        files.push((
            format!("crates/eutheto-export/src/generated/{filename}"),
            bytes,
        ));
    }
    files.push((
        "crates/eutheto-export/src/generated/recipient_renderer.rs".into(),
        metadata.into_bytes(),
    ));
    Ok(files)
}
