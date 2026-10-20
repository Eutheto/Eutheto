mod support;

use eutheto_domain_ir::{
    AcceptedResult, AssignmentValue, DomainAssignment, DomainAssignmentId, DomainEntityId,
    DomainEntityKindId, DomainEntityRef, NormalizedSolution, PortableAcceptedResult, RunInput,
    RunManifest, RunPhaseTimingsV1, RunTerminalOutcomeV1, ScoreLevelValue, ScoreVector,
    VerificationContextV1, VerificationReport, VerificationScope, VerifiedCheckpoint,
};
use eutheto_types::{
    BackendId, BackendSelection, DurationMillis, ExplanationMode, IanaTimeZone, OperationControl,
    PackId, PreservationPolicy, ReproducibilityMode, ResourceLimits, SolveMode, SolveOptions,
    SolveRunId, SolveStatus, WorkerThreadPolicy,
};
use eutheto_workforce::{
    export_source::WorkforceExportSource,
    summary_csv::{SummaryCsvErrorCode, encode_workforce_summary_csv},
};
use serde_json::json;
use std::{collections::BTreeMap, error::Error};

type Result<T = ()> = std::result::Result<T, Box<dyn Error>>;

const KIND: &str = "official.workforce.assignment";

fn id(index: u32) -> String {
    support::id(index)
}

fn assignment(person: u32, shift: u32, selected: bool) -> Result<DomainAssignment> {
    let pair = format!("{}.{}", id(person), id(shift));
    Ok(DomainAssignment {
        id: DomainAssignmentId::new(format!("{KIND}.{pair}"))?,
        entity: DomainEntityRef {
            kind: DomainEntityKindId::new(KIND)?,
            id: DomainEntityId::new(pair)?,
        },
        value: AssignmentValue::Boolean(selected),
        evidence: Vec::new(),
    })
}

fn solve_options() -> Result<SolveOptions> {
    Ok(SolveOptions {
        backend: BackendSelection::Auto,
        mode: SolveMode::Balanced,
        time_limit_milliseconds: DurationMillis::new(30_000)?,
        memory_limit_bytes: Some(64 * 1024 * 1024),
        worker_threads: WorkerThreadPolicy::Exact(1),
        random_seed: 7,
        solution_limit: Some(1),
        stop_after_first_feasible: true,
        collect_intermediate_solutions: false,
        explanation_mode: ExplanationMode::Standard,
        preserve_existing: PreservationPolicy::None,
        reproducibility: ReproducibilityMode::Deterministic,
        resource_limits: ResourceLimits {
            max_entities: 1_000,
            max_rules: 1_000,
            max_variables: 10_000,
            max_constraints: 10_000,
        },
    })
}

fn portable(assignments: Vec<DomainAssignment>) -> Result<PortableAcceptedResult> {
    let scenario_id = id(100).parse()?;
    let solution = NormalizedSolution {
        schema_version: 1,
        pack_id: PackId::new("official.workforce")?,
        scenario_id,
        scenario_revision: 1,
        projection_version: 1,
        solution_id: id(101).parse()?,
        assignments,
    };
    let verification_scope = VerificationScope::new(scenario_id, 1, Vec::new())?;
    let context = VerificationContextV1::new(
        scenario_id,
        1,
        eutheto_domain_ir::blake3_hex(b"document"),
        eutheto_domain_ir::blake3_hex(b"planning"),
        solution.canonical_hash()?,
        verification_scope.checksum.clone(),
    )?;
    let score = ScoreVector {
        feasibility: 0,
        levels: vec![ScoreLevelValue {
            level_id: eutheto_domain_ir::ScoreLevelId::new("score.preference")?,
            value: 0,
            direction: eutheto_domain_ir::OptimizationDirection::Minimize,
            category_breakdown: BTreeMap::new(),
        }],
    };
    let verification =
        VerificationReport::new(&context, Vec::new(), score, Vec::new(), BTreeMap::new())?;
    let accepted = AcceptedResult::new(solution.clone(), verification.clone())?;
    let run_id: SolveRunId = id(102).parse()?;
    let input = RunInput::new(
        run_id,
        id(103).parse()?,
        scenario_id,
        solution.scenario_revision,
        id(104).parse()?,
        eutheto_domain_ir::blake3_hex(b"document"),
        "2026-11-01T00:00:00Z".parse()?,
        solution.pack_id.clone(),
        1,
        1,
        "1.2.3".to_owned(),
        "0.1.0".to_owned(),
        BackendId::new("ortools.cp-sat")?,
        "9.15.6755".to_owned(),
        "0.1.0".to_owned(),
        "0.1.0".to_owned(),
        "9.15.6755".to_owned(),
        1,
        0,
        eutheto_domain_ir::blake3_hex(b"planning"),
        eutheto_domain_ir::blake3_hex(b"objective"),
        solve_options()?,
        "America/New_York".parse::<IanaTimeZone>()?,
        None,
        None,
    )?;
    let outcome = RunTerminalOutcomeV1::Accepted {
        status: SolveStatus::Feasible,
        solution_id: solution.solution_id,
        accepted_result_checksum: accepted.checksum.clone(),
        verification_checksum: verification.checksum.clone(),
    };
    let manifest = RunManifest::new(
        run_id,
        input.checksum.clone(),
        outcome,
        "2026-11-01T00:00:00Z".parse()?,
        "2026-11-01T00:00:01Z".parse()?,
        Some(DurationMillis::new(1_000)?),
        Some(DurationMillis::new(100)?),
        Some(DurationMillis::new(500)?),
        RunPhaseTimingsV1 {
            compile_milliseconds: Some(DurationMillis::new(10)?),
            backend_milliseconds: Some(DurationMillis::new(400)?),
            projection_milliseconds: Some(DurationMillis::new(20)?),
            structural_validation_milliseconds: Some(DurationMillis::new(15)?),
            score_recomputation_milliseconds: Some(DurationMillis::new(15)?),
            required_rule_verification_milliseconds: Some(DurationMillis::new(30)?),
            evidence_persistence_milliseconds: Some(DurationMillis::new(10)?),
            optional_explanation_milliseconds: None,
        },
        Vec::new(),
    )?;
    Ok(PortableAcceptedResult::new(
        input,
        manifest,
        accepted,
        BTreeMap::new(),
    )?)
}

fn checkpoint(portable: &PortableAcceptedResult) -> Result<VerifiedCheckpoint> {
    Ok(VerifiedCheckpoint::new(
        portable.run_input.clone(),
        DurationMillis::new(100)?,
        DurationMillis::new(500)?,
        "2026-11-01T00:00:01Z".parse()?,
        portable.accepted_result.clone(),
        portable.evidence.clone(),
    )?)
}

fn control() -> OperationControl {
    OperationControl::Cancellation(eutheto_types::CancellationToken::new())
}

fn document_with_two_people() -> Result<eutheto_types::ScenarioDocument> {
    let mut document = support::fixture()?;
    document.settings.overlap_policy = eutheto_types::OverlapPolicy::Earlier;
    document.domain.entities.insert(
        id(12).parse()?,
        json!({
            "kind": "person", "id": id(12), "name": "Second",
            "activeRange": {"kind": "always"},
            "qualificationGrants": [{"qualificationId": id(11)}],
            "eligibleAssignmentTypeIds": [id(4)],
            "workloadWeight": {"numerator": 1, "denominator": 1},
            "tags": [], "teamIds": []
        }),
    );
    Ok(document)
}

fn decode_summary(bytes: &[u8]) -> Result<Vec<Vec<String>>> {
    let mut reader = csv_core::Reader::new();
    let mut input = bytes;
    let mut decoded = vec![0; bytes.len()];
    let mut ends = [0; 18];
    let mut written = 0;
    let mut fields = 0;
    let mut records = Vec::new();
    loop {
        let (status, consumed, produced, ended) =
            reader.read_record(input, &mut decoded[written..], &mut ends[fields..]);
        input = &input[consumed..];
        written += produced;
        fields += ended;
        match status {
            csv_core::ReadRecordResult::Record => {
                let mut start = 0;
                let mut record = Vec::new();
                for &end in &ends[..fields] {
                    record.push(std::str::from_utf8(&decoded[start..end])?.to_owned());
                    start = end;
                }
                records.push(record);
                written = 0;
                fields = 0;
            }
            csv_core::ReadRecordResult::InputEmpty => {}
            csv_core::ReadRecordResult::End => break,
            csv_core::ReadRecordResult::OutputFull | csv_core::ReadRecordResult::OutputEndsFull => {
                return Err("summary record exceeds the consumer's byte or column buffer".into());
            }
        }
    }
    assert_eq!(
        records.len(),
        6,
        "framing, header, two people and two shifts"
    );
    assert_eq!(records[0], ["eutheto/workforce-summary", "1"]);
    assert_eq!(
        records[1],
        [
            "kind",
            "person_id",
            "person_name",
            "shift_id",
            "shift_name",
            "source_timezone",
            "scenario_revision",
            "revision_as_of",
            "revision_status",
            "selected_count",
            "required",
            "minimum",
            "maximum",
            "margin",
            "shift_starts_local",
            "shift_ends_local",
            "shift_starts_instant",
            "shift_ends_instant",
        ]
    );
    drop(records.drain(..2));
    assert!(records.iter().all(|record| record.len() == 18));
    Ok(records)
}

#[test]
fn summary_consumer_preserves_identity_counts_revision_dst_and_safe_names() -> Result {
    let mut document = document_with_two_people()?;
    // Names accept Unicode, quotes and commas, but source validation forbids control
    // characters, including embedded CR/LF record boundaries.
    for (entity, name) in [
        (1, "=River, \"hostile\" 雪"),
        (12, "-Second, \"Zoë\""),
        (6, "+Sunday, \"診療\""),
        (4, "@Clinic, \"é\""),
    ] {
        document
            .domain
            .entities
            .get_mut(&id(entity).parse()?)
            .ok_or("named entity missing")?["name"] = json!(name);
    }
    let person_id = id(1);
    let second_id = id(12);
    let generated_id = id(7);
    let manual_id = id(8);
    let timezone = "America/New_York".parse::<IanaTimeZone>()?;
    let zone = jiff::tz::TimeZone::get(timezone.as_str())?;
    for (selected, revision_as_of, revision_status) in
        [(true, 2, "old-revision"), (false, 1, "current")]
    {
        // This synthetic portable fixture tests the codec, not native acceptance.
        let result = portable(vec![assignment(1, 8, selected)?])?;
        let output = encode_workforce_summary_csv(
            &document,
            WorkforceExportSource::Terminal(&result),
            revision_as_of,
            &control(),
        )?;
        let rows = decode_summary(&output)?;
        let count = i64::from(selected);
        let identities = [
            ["person", &person_id, "'=River, \"hostile\" 雪", "", ""],
            ["person", &second_id, "'-Second, \"Zoë\"", "", ""],
            ["shift", "", "", &generated_id, "'+Sunday, \"診療\""],
            ["shift", "", "", &manual_id, "'@Clinic, \"é\""],
        ];
        for ((row, identity), selected_count) in
            rows.iter().zip(identities).zip([count, 0, 0, count])
        {
            assert_eq!(row[..5], identity, "complete source identity census");
            assert_eq!(row[5].parse::<IanaTimeZone>()?, timezone);
            assert_eq!(row[6].parse::<u64>()?, 1);
            assert_eq!(row[7].parse::<u64>()?, revision_as_of);
            assert_eq!(row[8], revision_status);
            assert_eq!(row[9].parse::<i64>()?, selected_count);
        }
        for row in &rows[..2] {
            assert!(row[10..].iter().all(String::is_empty));
        }
        for (row, selected_count) in rows[2..].iter().zip([0, count]) {
            let coverage = row[10..14]
                .iter()
                .map(|cell| cell.parse::<i64>())
                .collect::<std::result::Result<Vec<_>, _>>()?;
            assert_eq!(coverage, [1, 1, 1, selected_count - 1]);
        }
        for (row, end_local, end_instant, elapsed, wall) in [
            (
                &rows[2],
                "2026-11-01T01:30:00",
                "2026-11-01T06:30:00Z",
                3600,
                0,
            ),
            (
                &rows[3],
                "2026-11-01T02:30:00",
                "2026-11-01T07:30:00Z",
                7200,
                3600,
            ),
        ] {
            let starts_local = row[14].parse::<jiff::civil::DateTime>()?;
            let ends_local = row[15].parse::<jiff::civil::DateTime>()?;
            let starts_instant = row[16].parse::<jiff::Timestamp>()?;
            let ends_instant = row[17].parse::<jiff::Timestamp>()?;
            assert_eq!(
                starts_local,
                "2026-11-01T01:30:00".parse::<jiff::civil::DateTime>()?
            );
            assert_eq!(ends_local, end_local.parse::<jiff::civil::DateTime>()?);
            assert_eq!(
                starts_instant,
                "2026-11-01T05:30:00Z".parse::<jiff::Timestamp>()?
            );
            assert_eq!(ends_instant, end_instant.parse::<jiff::Timestamp>()?);
            assert_eq!(
                starts_instant.to_zoned(zone.clone()).datetime(),
                starts_local
            );
            assert_eq!(ends_instant.to_zoned(zone.clone()).datetime(), ends_local);
            assert_eq!(
                ends_instant.as_second() - starts_instant.as_second(),
                elapsed
            );
            assert_eq!(ends_local.duration_since(starts_local).as_secs(), wall);
        }
    }
    Ok(())
}

#[test]
fn checkpoint_consumer_preserves_summary_bytes_and_complete_source_census() -> Result {
    let document = document_with_two_people()?;
    for selected in [true, false] {
        let terminal = portable(vec![assignment(1, 8, selected)?])?;
        let checkpoint = checkpoint(&terminal)?;
        for revision in [1, 2] {
            let bytes = encode_workforce_summary_csv(
                &document,
                WorkforceExportSource::Checkpoint(&checkpoint),
                revision,
                &control(),
            )?;
            let terminal_bytes = encode_workforce_summary_csv(
                &document,
                WorkforceExportSource::Terminal(&terminal),
                revision,
                &control(),
            )?;
            assert_eq!(bytes, terminal_bytes);
            let rows = decode_summary(&bytes)?;
            assert_eq!(rows.len(), 4);
            assert_eq!(rows[0][1], id(1));
            assert_eq!(rows[1][1], id(12));
            assert_eq!(rows[0][9].parse::<u32>()?, u32::from(selected));
            assert_eq!(rows[1][9].parse::<u32>()?, 0);
        }
    }
    Ok(())
}

#[test]
fn malformed_checkpoint_and_terminal_bindings_refuse_summary_output() -> Result {
    let document = document_with_two_people()?;
    let terminal = portable(vec![assignment(1, 8, true)?])?;
    let checkpoint = checkpoint(&terminal)?;
    let mut bad_checksum = checkpoint.clone();
    bad_checksum.checksum = eutheto_domain_ir::blake3_hex(b"wrong");
    let mut bad_input = checkpoint.clone();
    bad_input.run_input.scenario_revision = 2;
    let mut bad_result = checkpoint.clone();
    bad_result.accepted_result.solution.scenario_revision = 2;
    let mut bad_version = checkpoint.clone();
    bad_version.schema_version = 999;
    let mut bad_identity = terminal.clone();
    bad_identity.result_id = id(201).parse()?;
    let mut bad_revision = terminal.clone();
    bad_revision.scenario_revision = 2;
    let mut bad_acceptance = terminal.clone();
    bad_acceptance.accepted_result.checksum = eutheto_domain_ir::blake3_hex(b"wrong");
    let mut bad_verification = terminal.clone();
    bad_verification.accepted_result.verification.checksum =
        eutheto_domain_ir::blake3_hex(b"wrong");
    let mut bad_outcome = terminal.clone();
    bad_outcome.run_manifest.outcome = RunTerminalOutcomeV1::Interrupted;
    let mut bad_status = terminal.clone();
    if let RunTerminalOutcomeV1::Accepted { status, .. } = &mut bad_status.run_manifest.outcome {
        *status = SolveStatus::Cancelled;
    }
    for source in [
        WorkforceExportSource::Checkpoint(&bad_checksum),
        WorkforceExportSource::Checkpoint(&bad_input),
        WorkforceExportSource::Checkpoint(&bad_result),
        WorkforceExportSource::Checkpoint(&bad_version),
        WorkforceExportSource::Terminal(&bad_identity),
        WorkforceExportSource::Terminal(&bad_revision),
        WorkforceExportSource::Terminal(&bad_acceptance),
        WorkforceExportSource::Terminal(&bad_verification),
        WorkforceExportSource::Terminal(&bad_outcome),
        WorkforceExportSource::Terminal(&bad_status),
    ] {
        let error = encode_workforce_summary_csv(&document, source, 1, &control())
            .err()
            .ok_or("malformed source must not produce a summary")?;
        assert_eq!(error.code, SummaryCsvErrorCode::InvalidResult);
    }
    let mut other_scenario = document.clone();
    other_scenario.scenario_id = id(200).parse()?;
    let mut other_timezone = document.clone();
    other_timezone.settings.time_zone = "UTC".parse()?;
    for source_document in [&other_scenario, &other_timezone] {
        for source in [
            WorkforceExportSource::Checkpoint(&checkpoint),
            WorkforceExportSource::Terminal(&terminal),
        ] {
            let error = encode_workforce_summary_csv(source_document, source, 1, &control())
                .err()
                .ok_or("source mismatch must not produce a summary")?;
            assert_eq!(error.code, SummaryCsvErrorCode::InvalidResult);
        }
    }
    Ok(())
}
