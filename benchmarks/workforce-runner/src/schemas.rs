//! Closed schemas for the private corpus records, not new application contracts.
//! Schemas check shape and local bounds. Rust additionally checks source semantics,
//! filesystem custody, exact byte hashes, metric relationships and result authority.
use crate::contract::{
    CLI_EVIDENCE_FORMAT, CORPUS_FORMAT, CORPUS_VERSION, CaseId, EVIDENCE_FORMAT, EXPECTED_FORMAT,
    EXPECTED_PATH, FIXTURE_CLOCK, FORMAT_VERSION, MAX_EXPECTED_BYTES, MAX_INDEX_BYTES,
    MeasurementProfile, REPAIR_FORMAT, REPAIR_PATH,
};
use eutheto_workforce::commands;
use serde_json::{Value, json};

fn object(properties: &[(&str, Value)]) -> Value {
    let fields: serde_json::Map<String, Value> = properties
        .iter()
        .map(|(name, schema)| ((*name).to_owned(), schema.clone()))
        .collect();
    let required: Vec<_> = properties.iter().map(|(name, _)| *name).collect();
    json!({"type":"object","properties":fields,"required":required,"additionalProperties":false})
}
fn text(minimum: usize, maximum: usize) -> Value {
    json!({"type":"string","minLength":minimum,"maxLength":maximum})
}
fn token() -> Value {
    json!({"type":"string","minLength":1,"maxLength":128,"pattern":"^[A-Za-z][A-Za-z0-9._-]*$"})
}
fn unsigned(maximum: u64) -> Value {
    json!({"type":"integer","minimum":0,"maximum":maximum})
}
fn positive(maximum: u64) -> Value {
    json!({"type":"integer","minimum":1,"maximum":maximum})
}
fn signed() -> Value {
    json!({"type":"integer","minimum":i64::MIN,"maximum":i64::MAX})
}
fn constant(value: impl Into<Value>) -> Value {
    Value::Object(serde_json::Map::from_iter([(
        "const".to_owned(),
        value.into(),
    )]))
}
fn choices(values: &[&str]) -> Value {
    json!({"type":"string","enum":values})
}
fn nullable(schema: Value) -> Value {
    Value::Object(serde_json::Map::from_iter([(
        "anyOf".to_owned(),
        Value::Array(vec![schema, json!({"type":"null"})]),
    )]))
}
fn array(schema: Value, minimum: usize, maximum: usize) -> Value {
    Value::Object(serde_json::Map::from_iter([
        ("type".to_owned(), Value::from("array")),
        ("items".to_owned(), schema),
        ("minItems".to_owned(), Value::from(minimum)),
        ("maxItems".to_owned(), Value::from(maximum)),
    ]))
}
fn digest() -> Value {
    json!({"type":"string","minLength":64,"maxLength":64,"pattern":"^[0-9a-f]{64}$"})
}
fn uuid() -> Value {
    json!({"type":"string","minLength":36,"maxLength":36,"format":"uuid","pattern":"^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$"})
}
fn path() -> Value {
    json!({"type":"string","minLength":1,"maxLength":256,"pattern":"^(?!/)(?!.*(?:^|/)\\.{1,2}(?:/|$))[A-Za-z0-9_.-]+(?:/[A-Za-z0-9_.-]+)*$"})
}
fn string_map(values: Value, maximum: usize) -> Value {
    Value::Object(serde_json::Map::from_iter([
        ("type".to_owned(), Value::from("object")),
        ("propertyNames".to_owned(), token()),
        ("additionalProperties".to_owned(), values),
        ("maxProperties".to_owned(), Value::from(maximum)),
    ]))
}
fn case_id() -> Value {
    json!({"type":"string","enum":CaseId::ALL.map(CaseId::slug)})
}
fn process_state() -> Value {
    choices(&["firstOperation", "postWarmup"])
}
fn status() -> Value {
    choices(&[
        "optimal",
        "feasible",
        "infeasible",
        "unbounded",
        "noSolutionWithinLimit",
        "cancelled",
        "invalidModel",
        "backendUnavailable",
        "backendFailed",
    ])
}
fn version_fields(format: &str) -> [(&str, Value); 3] {
    [
        ("format", constant(format)),
        ("schemaVersion", constant(FORMAT_VERSION)),
        ("corpusVersion", constant(CORPUS_VERSION)),
    ]
}
fn file_binding(path_schema: Value, maximum_bytes: u64) -> Value {
    object(&[
        ("path", path_schema),
        ("sha256", digest()),
        ("bytes", positive(maximum_bytes)),
    ])
}
fn profile() -> Value {
    let reviewed = MeasurementProfile::REVIEWED;
    object(&[
        ("budgetMilliseconds", constant(reviewed.budget_milliseconds)),
        ("mode", json!({"const":reviewed.mode})),
        ("randomSeed", constant(reviewed.random_seed)),
        ("workerThreads", constant(reviewed.worker_threads)),
        (
            "stopAfterFirstFeasible",
            constant(reviewed.stop_after_first_feasible),
        ),
        (
            "firstOperationSamples",
            constant(reviewed.first_operation_samples),
        ),
        ("warmupRuns", constant(reviewed.warmup_runs)),
        ("postWarmupSamples", constant(reviewed.post_warmup_samples)),
    ])
}
fn generator() -> Value {
    object(&[
        ("method", text(1, 256)),
        ("sourceSha256", digest()),
        (
            "sources",
            array(file_binding(path(), 16 * 1024 * 1024), 1, 128),
        ),
        ("seed", constant(0_u64)),
        ("license", text(1, 128)),
    ])
}
fn case_manifest() -> Value {
    let variants: Vec<_> = CaseId::ALL
        .into_iter()
        .map(|case| {
            let family = match case {
                CaseId::AppendixF => Value::Null,
                CaseId::ClinicTiny | CaseId::ClinicInitial | CaseId::ClinicFull => {
                    json!("clinic-small")
                }
                CaseId::ClinicOvernight => json!("clinic-overnight"),
                CaseId::RollingHours => json!("rolling-hours"),
                CaseId::SpecialistCoverage => json!("specialist-coverage"),
                CaseId::RepairBefore => json!("repair-callout"),
                CaseId::InfeasibleCoverage => json!("infeasible-coverage"),
                CaseId::DstSpring | CaseId::DstFall => json!("dst"),
                CaseId::LargeSupported | CaseId::LargePressure => json!("large-synthetic"),
            };
            let class = match case {
                CaseId::ClinicTiny
                | CaseId::InfeasibleCoverage
                | CaseId::DstSpring
                | CaseId::DstFall => "A",
                CaseId::LargeSupported => "C",
                CaseId::LargePressure => "D",
                _ => "B",
            };
            object(&[
                ("id", constant(case.slug())),
                ("family", constant(family)),
                ("workloadClass", constant(class)),
                (
                    "input",
                    file_binding(constant(case.fixture_path()), 16 * 1024 * 1024),
                ),
            ])
        })
        .collect();
    json!({"oneOf":variants})
}
fn inventory(items: Value, nested_expectation: bool) -> Value {
    let mut schema = array(items, 13, 13);
    let assertions: Vec<_> = CaseId::ALL.into_iter().map(|case| {
        let identity = json!({"type":"object","required":["id"],"properties":{"id":{"const":case.slug()}}});
        let contains = if nested_expectation {
            json!({"type":"object","required":["expected"],"properties":{"expected":identity}})
        } else { identity };
        json!({"contains":contains,"minContains":1,"maxContains":1})
    }).collect();
    schema["allOf"] = json!(assertions);
    schema
}
fn corpus() -> Value {
    let mut properties = version_fields(CORPUS_FORMAT).to_vec();
    properties.extend([
        ("generator", generator()),
        ("fixtureClock", constant(FIXTURE_CLOCK)),
        ("profile", profile()),
        ("cases", inventory(case_manifest(), false)),
        (
            "expectations",
            file_binding(constant(EXPECTED_PATH), MAX_EXPECTED_BYTES),
        ),
        (
            "repair",
            file_binding(constant(REPAIR_PATH), MAX_INDEX_BYTES),
        ),
    ]);
    object(&properties)
}
fn obligation() -> Value {
    json!({"oneOf":[
        object(&[("kind",constant("rule")),("ruleId",uuid()),("ruleKind",choices(&["maximumHours","maximumConsecutive"]))]),
        object(&[("kind",constant("preference")),("ruleId",uuid()),("preferenceKind",choices(&["workloadBalance","time"]))]),
        object(&[("kind",constant("lock")),("assignmentId",uuid())])
    ]})
}
fn expected_case() -> Value {
    object(&[
        ("id", case_id()),
        (
            "disposition",
            choices(&["accepted", "infeasible", "compileRejected", "resourceLimit"]),
        ),
        ("people", positive(256)),
        ("horizonDays", positive(64)),
        ("resolvedShifts", positive(128)),
        ("selectedAssignments", nullable(unsigned(32768))),
        (
            "score",
            nullable(object(&[
                ("feasibility", signed()),
                ("minimumStableRank", signed()),
                ("maximumStableRank", signed()),
            ])),
        ),
        (
            "modelEnvelope",
            nullable(object(&[
                ("minimumVariables", unsigned(u64::from(u32::MAX))),
                ("maximumVariables", unsigned(u64::from(u32::MAX))),
                ("maximumConstraints", unsigned(u64::from(u32::MAX))),
            ])),
        ),
        ("deferredObligations", array(obligation(), 0, 16)),
        ("semantics", array(text(1, 2048), 1, 32)),
    ])
}
fn expectations() -> Value {
    let mut properties = version_fields(EXPECTED_FORMAT).to_vec();
    properties.extend([
        ("generatorSourceSha256", digest()),
        (
            "cases",
            inventory(
                object(&[("inputSha256", digest()), ("expected", expected_case())]),
                true,
            ),
        ),
    ]);
    object(&properties)
}
fn repair() -> Value {
    // This is a deliberately narrower closed subset of the landed Availability schema:
    // absent assignmentTypeIds/locationIds are omitted, NOT serialized as null. All other
    // EntityPayload variants are excluded, so no BaseSchedule/solution authority is possible.
    // Typed source-ID equality and the real baseline's acceptance/selection remain Rust/runtime gates.
    let availability = object(&[
        ("kind", constant("availability")),
        ("id", uuid()),
        ("personId", uuid()),
        ("availabilityKind", constant("unavailable")),
        (
            "timeWindow",
            object(&[
                ("kind", constant("instant")),
                ("startsAt", constant("2026-09-02T05:00:00Z")),
                ("endsAt", constant("2026-09-03T05:00:00Z")),
            ]),
        ),
        (
            "effectiveRange",
            object(&[
                ("startDate", constant("2026-09-01")),
                ("endDateExclusive", constant("2026-09-08")),
            ]),
        ),
        ("source", constant("synthetic-callout")),
        ("note", constant("Synthetic unexpected call-out")),
    ]);
    let mut properties = version_fields(REPAIR_FORMAT).to_vec();
    properties.extend([
        ("beforeCase", constant("repair-before")), ("executionPhase", constant(7)),
        ("baselineRequirement", constant("Phase07: obtain a genuine independently accepted solution of repair-before containing person-0 on the September2 clinic and select it as the baseline before applying this call-out")),
        ("commandId", constant(commands::ADD_ENTITY)), ("payload", object(&[("entity", availability)])),
        ("repairRequirement", constant("Phase07: repair from that accepted and selected baseline after this single unavailable-person mutation; pure mutation validation is not repair execution")),
    ]);
    object(&properties)
}
fn timings() -> Value {
    object(&[
        ("milliseconds", string_map(unsigned(u64::MAX), 64)),
        ("unavailable", string_map(text(1, 512), 64)),
    ])
}
fn model() -> Value {
    object(&[
        ("variables", unsigned(u64::MAX)),
        ("constraints", unsigned(u64::MAX)),
        ("canonicalIrBlake3", digest()),
        ("domainFilterCounts", string_map(unsigned(u64::MAX), 16)),
    ])
}
fn backend() -> Value {
    object(&[
        ("backendId", token()),
        ("termination", token()),
        ("versions", string_map(text(1, 256), 32)),
        ("remainingAtDispatchMilliseconds", unsigned(u64::MAX)),
        ("backendLimitMilliseconds", unsigned(u64::MAX)),
        ("timings", timings()),
        ("translatedVariables", unsigned(u64::MAX)),
        ("translatedConstraints", unsigned(u64::MAX)),
        ("appliedParametersSha256", digest()),
        ("modelFingerprintSha256", digest()),
        ("backendObjectiveValues", nullable(array(signed(), 0, 64))),
        ("backendBestBoundValues", nullable(array(signed(), 0, 64))),
        ("infeasibilityCore", nullable(text(1, 1024))),
    ])
}
fn acceptance() -> Value {
    object(&[
        ("portableSha256", digest()),
        ("selectedAssignments", unsigned(u64::from(u32::MAX))),
        ("feasibility", signed()),
        ("stableRank", signed()),
        ("consumerChecks", array(text(1, 256), 1, 64)),
    ])
}
fn sample() -> Value {
    object(&[
        ("caseId", case_id()),
        ("runnerProcessState", process_state()),
        ("ordinal", unsigned(u64::from(u16::MAX))),
        ("status", status()),
        ("preparationErrorCode", nullable(token())),
        ("model", nullable(model())),
        ("timings", timings()),
        ("backend", nullable(backend())),
        ("acceptance", nullable(acceptance())),
    ])
}
fn sample_batch() -> Value {
    object(&[
        ("schemaVersion", constant(FORMAT_VERSION)),
        ("corpusSha256", digest()),
        ("workerManifestSha256", digest()),
        ("runnerSha256", digest()),
        ("samples", array(sample(), 1, 52)),
    ])
}
fn aggregate() -> Value {
    object(&[
        ("caseId", case_id()),
        ("runnerProcessState", process_state()),
        ("metric", token()),
        ("samples", positive(u64::from(u16::MAX))),
        ("minimumMilliseconds", unsigned(u64::MAX)),
        ("medianMilliseconds", unsigned(u64::MAX)),
        ("maximumMilliseconds", unsigned(u64::MAX)),
    ])
}
fn runner() -> Value {
    object(&[
        ("sourceSha256", digest()),
        ("executableSha256", digest()),
        ("target", text(1, 128)),
        ("rustc", text(1, 512)),
        ("buildProfile", text(1, 64)),
        ("runnerClass", choices(&["local", "githubActions"])),
        (
            "logicalParallelism",
            nullable(positive(u64::from(u32::MAX))),
        ),
        (
            "checkoutCommit",
            json!({"type":"string","minLength":40,"maxLength":64,"pattern":"^(?:[0-9a-f]{40}|[0-9a-f]{64})$"}),
        ),
        ("checkoutDirty", json!({"type":"boolean"})),
    ])
}
fn evidence() -> Value {
    let mut properties = version_fields(EVIDENCE_FORMAT).to_vec();
    properties.extend([
        ("corpusSha256", digest()),
        ("workerManifestSha256", digest()),
        ("runner", runner()),
        ("profile", profile()),
        ("workerLifecycle", constant("spawnPerSolve")),
        ("osCacheState", constant("unmanaged")),
        ("enrichmentState", constant("notApplicable")),
        ("evidenceClass", text(1, 256)),
        ("aggregationMethod", text(1, 512)),
        ("variancePolicy", text(1, 1024)),
        ("samples", array(sample(), 1, 52)),
        ("aggregates", array(aggregate(), 0, 1664)),
    ]);
    object(&properties)
}
fn cli_evidence() -> Value {
    let check = object(&[
        ("caseId", case_id()),
        ("operation", text(1, 256)),
        (
            "exitCode",
            json!({"type":"integer","enum":[0,2,3,4,5,6,7,8,10,130]}),
        ),
        ("observedStatus", nullable(status())),
        ("observedCode", nullable(token())),
        ("artifactSha256", nullable(digest())),
    ]);
    let mut properties = version_fields(CLI_EVIDENCE_FORMAT).to_vec();
    properties.extend([
        ("corpusSha256", digest()),
        ("cliSha256", digest()),
        ("workerManifestSha256", digest()),
        ("checks", array(check, 1, 256)),
    ]);
    object(&properties)
}

pub(crate) fn documents() -> Vec<(&'static str, Value)> {
    [
        ("corpus", corpus()), ("expectations", expectations()), ("repair", repair()),
        ("evidence", evidence()), ("cli-evidence", cli_evidence()), ("sample-batch", sample_batch()),
    ].into_iter().map(|(name, mut schema)| {
        schema["$schema"] = json!("https://json-schema.org/draft/2020-12/schema");
        schema["title"] = json!(format!("Workforce corpus v1 {name}"));
        schema["description"] = json!("Private synthetic corpus record. Structural validation does not prove source fidelity, safe filesystem custody, cross-file digest binding, accepted-result authority, historical execution authenticity or timing relationships; the corpus runner performs those applicable checks.");
        (name, schema)
    }).collect()
}
