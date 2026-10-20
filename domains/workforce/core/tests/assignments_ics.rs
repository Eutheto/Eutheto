mod support;

use eutheto_domain_ir::{
    AcceptedResult, AssignmentValue, DomainAssignment, DomainAssignmentId, DomainEntityId,
    DomainEntityKindId, DomainEntityRef, NormalizedSolution, PortableAcceptedResult, RunInput,
    RunManifest, RunPhaseTimingsV1, RunTerminalOutcomeV1, ScoreLevelValue, ScoreVector,
    VerificationContextV1, VerificationReport, VerificationScope, VerifiedCheckpoint,
};
use eutheto_types::{
    BackendId, BackendSelection, DurationMillis, ExplanationMode, IanaTimeZone, OperationControl,
    PackId, PreservationPolicy, ReproducibilityMode, ResourceLimits, Rfc3339Timestamp, SolveMode,
    SolveOptions, SolveRunId, SolveStatus, WorkerThreadPolicy,
};
use eutheto_workforce::assignments_ics::{AssignmentsIcsErrorCode, encode_workforce_ics};
use eutheto_workforce::export_source::WorkforceExportSource;
use ical::{IcalParser, parser::ical::component::IcalCalendar, property::Property};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    io::Cursor,
};

const KIND: &str = "official.workforce.assignment";

const EVENT_PROPERTIES: &[&str] = &[
    "UID",
    "DTSTAMP",
    "DTSTART",
    "DTEND",
    "SUMMARY",
    "TRANSP",
    "X-EUTHETO-ASSIGNMENT-ID",
    "X-EUTHETO-PERSON-ID",
    "X-EUTHETO-SHIFT-ID",
    "LOCATION",
];

type Result<T = ()> = std::result::Result<T, Box<dyn Error>>;

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

fn portable() -> Result<PortableAcceptedResult> {
    portable_with_assignments(101, 1, vec![assignment(1, 8, true)?])
}

// Synthetic codec input only; native acceptance is exercised by Core's worker fixtures.
fn portable_with_assignments(
    solution_index: u32,
    revision: u64,
    assignments: Vec<DomainAssignment>,
) -> Result<PortableAcceptedResult> {
    let scenario_id = id(100).parse()?;
    let solution = NormalizedSolution {
        schema_version: 1,
        pack_id: PackId::new("official.workforce")?,
        scenario_id,
        scenario_revision: revision,
        projection_version: 1,
        solution_id: id(solution_index).parse()?,
        assignments,
    };
    let verification_scope = VerificationScope::new(scenario_id, revision, Vec::new())?;
    let context = VerificationContextV1::new(
        scenario_id,
        revision,
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
    let run_id: SolveRunId = id(solution_index + 1).parse()?;
    let input = RunInput::new(
        run_id,
        id(solution_index + 2).parse()?,
        scenario_id,
        solution.scenario_revision,
        id(solution_index + 3).parse()?,
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

fn export_time() -> Result<Rfc3339Timestamp> {
    Ok(Rfc3339Timestamp::parse("2026-11-02T12:34:56Z")?)
}

fn parse_calendar(bytes: &[u8]) -> Result<IcalCalendar> {
    let mut parser = IcalParser::new(Cursor::new(bytes));
    let calendar = parser.next().ok_or("calendar missing")??;
    assert!(parser.next().is_none(), "unexpected second calendar");
    assert!(calendar.alarms.is_empty());
    assert!(calendar.todos.is_empty());
    assert!(calendar.journals.is_empty());
    assert!(calendar.free_busys.is_empty());
    assert!(calendar.timezones.is_empty(), "events use UTC, not TZID");
    Ok(calendar)
}

fn property_values<'a>(
    properties: &'a [Property],
    expected_names: &[&str],
) -> Result<BTreeMap<&'a str, &'a str>> {
    let mut values = BTreeMap::new();
    for property in properties {
        assert!(property.params.is_none(), "unexpected property parameters");
        let value = property.value.as_deref().ok_or("property value missing")?;
        assert!(
            values.insert(property.name.as_str(), value).is_none(),
            "duplicate property: {}",
            property.name
        );
    }
    assert_eq!(
        values.keys().copied().collect::<BTreeSet<_>>(),
        expected_names.iter().copied().collect::<BTreeSet<_>>(),
        "unexpected or missing property, including private authority fields"
    );
    Ok(values)
}

fn calendar_values(
    calendar: &IcalCalendar,
    stale: bool,
    filtered: bool,
) -> Result<BTreeMap<&str, &str>> {
    let mut names = vec![
        "PRODID",
        "VERSION",
        "CALSCALE",
        "X-EUTHETO-FORMAT",
        "NAME",
        "X-EUTHETO-SCENARIO-ID",
        "X-EUTHETO-SCENARIO-REVISION",
        "X-EUTHETO-REVISION-AS-OF",
        "X-EUTHETO-STALE-AS-OF",
        "X-EUTHETO-STATUS",
        "X-EUTHETO-HISTORICAL-PROOF",
        "X-EUTHETO-SCENARIO-TIMEZONE",
    ];
    if stale {
        names.push("X-EUTHETO-STALE");
    }
    if filtered {
        names.push("X-EUTHETO-PERSON-ID");
    }
    property_values(&calendar.properties, &names)
}

fn utc_timestamp(value: &str) -> Result<jiff::Timestamp> {
    assert_eq!(value.len(), 16, "UTC DATE-TIME must use basic seconds");
    Ok(jiff::civil::DateTime::strptime("%Y%m%dT%H%M%SZ", value)?
        .to_zoned(jiff::tz::TimeZone::UTC)?
        .timestamp())
}

type EventIdentity = (String, String, String);

fn event_uids(calendar: &IcalCalendar) -> Result<BTreeMap<EventIdentity, String>> {
    let mut events = BTreeMap::new();
    let mut uids = BTreeSet::new();
    for event in &calendar.events {
        assert!(
            event.alarms.is_empty(),
            "unexpected injected event component"
        );
        let values = property_values(&event.properties, EVENT_PROPERTIES)?;
        let start = utc_timestamp(values["DTSTART"])?;
        let end = utc_timestamp(values["DTEND"])?;
        assert_eq!(start, "2026-11-01T05:30:00Z".parse::<jiff::Timestamp>()?);
        let (expected_end, duration) = if values["X-EUTHETO-SHIFT-ID"] == id(7) {
            ("2026-11-01T06:30:00Z", 3_600)
        } else {
            assert_eq!(values["X-EUTHETO-SHIFT-ID"], id(8));
            ("2026-11-01T07:30:00Z", 7_200)
        };
        assert_eq!(end, expected_end.parse::<jiff::Timestamp>()?);
        assert_eq!(end.as_second() - start.as_second(), duration);
        utc_timestamp(values["DTSTAMP"])?;
        let identity = (
            values["X-EUTHETO-ASSIGNMENT-ID"].to_owned(),
            values["X-EUTHETO-PERSON-ID"].to_owned(),
            values["X-EUTHETO-SHIFT-ID"].to_owned(),
        );
        let uid = values["UID"];
        let opaque = uid
            .strip_prefix("eutheto-")
            .and_then(|value| value.strip_suffix("@eutheto"))
            .ok_or("event UID is not in the opaque namespace")?;
        assert_eq!(opaque.len(), 64);
        assert!(opaque.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert!(uids.insert(uid), "duplicate event UID");
        assert!(
            events.insert(identity, uid.to_owned()).is_none(),
            "duplicate assignment event"
        );
    }
    Ok(events)
}

#[test]
fn consumer_reads_source_bound_utc_events_and_stale_metadata() -> Result {
    let mut document = support::fixture()?;
    document.settings.overlap_policy = eutheto_types::OverlapPolicy::Earlier;
    let result = portable()?;
    let bytes = encode_workforce_ics(
        &document,
        WorkforceExportSource::Terminal(&result),
        None,
        2,
        export_time()?,
        &control(),
    )?;
    let historical = parse_calendar(&bytes)?;
    let metadata = calendar_values(&historical, true, false)?;
    assert_eq!(metadata["VERSION"], "2.0");
    assert_eq!(metadata["CALSCALE"], "GREGORIAN");
    assert_eq!(metadata["X-EUTHETO-SCENARIO-ID"], id(100));
    assert_eq!(metadata["X-EUTHETO-SCENARIO-REVISION"], "1");
    assert_eq!(metadata["X-EUTHETO-REVISION-AS-OF"], "2");
    assert_eq!(metadata["X-EUTHETO-STALE-AS-OF"], "TRUE");
    assert_eq!(metadata["X-EUTHETO-STALE"], "TRUE");
    assert_eq!(metadata["X-EUTHETO-STATUS"], "feasible");
    assert_eq!(metadata["X-EUTHETO-HISTORICAL-PROOF"], "UNVERIFIED");
    assert_eq!(metadata["X-EUTHETO-SCENARIO-TIMEZONE"], "America/New_York");
    assert_eq!(historical.events.len(), 1);
    let event = historical
        .events
        .first()
        .ok_or("assignment event missing")?;
    let values = property_values(&event.properties, EVENT_PROPERTIES)?;
    let start = utc_timestamp(values["DTSTART"])?;
    let end = utc_timestamp(values["DTEND"])?;
    assert_eq!(start, "2026-11-01T05:30:00Z".parse::<jiff::Timestamp>()?);
    assert_eq!(end, "2026-11-01T07:30:00Z".parse::<jiff::Timestamp>()?);
    assert_eq!(end.as_second() - start.as_second(), 7_200);
    assert_eq!(
        utc_timestamp(values["DTSTAMP"])?,
        "2026-11-02T12:34:56Z".parse::<jiff::Timestamp>()?
    );
    let start_local = start.in_tz(metadata["X-EUTHETO-SCENARIO-TIMEZONE"])?;
    let end_local = end.in_tz(metadata["X-EUTHETO-SCENARIO-TIMEZONE"])?;
    assert_eq!(
        start_local.datetime(),
        jiff::civil::date(2026, 11, 1).at(1, 30, 0, 0)
    );
    assert_eq!(
        end_local.datetime(),
        jiff::civil::date(2026, 11, 1).at(2, 30, 0, 0)
    );
    assert_eq!(start_local.offset().seconds(), -14_400);
    assert_eq!(end_local.offset().seconds(), -18_000);
    assert_eq!(values["TRANSP"], "OPAQUE");
    assert_eq!(values["LOCATION"], "North");
    assert!(values["SUMMARY"].contains("River"));
    assert!(values["SUMMARY"].contains("Clinic"));
    assert!(!values["UID"].contains(&result.result_id.to_string()));

    let current_bytes = encode_workforce_ics(
        &document,
        WorkforceExportSource::Terminal(&result),
        None,
        1,
        export_time()?,
        &control(),
    )?;
    let current = parse_calendar(&current_bytes)?;
    let current_metadata = calendar_values(&current, false, false)?;
    assert_eq!(current_metadata["X-EUTHETO-SCENARIO-REVISION"], "1");
    assert_eq!(current_metadata["X-EUTHETO-REVISION-AS-OF"], "1");
    assert_eq!(current_metadata["X-EUTHETO-STALE-AS-OF"], "FALSE");
    assert_eq!(current_metadata["X-EUTHETO-STATUS"], "feasible");
    assert_eq!(current_metadata["X-EUTHETO-HISTORICAL-PROOF"], "UNVERIFIED");
    assert_eq!(event_uids(&current)?, event_uids(&historical)?);
    let current_event = current.events.first().ok_or("current event missing")?;
    let current_values = property_values(&current_event.properties, EVENT_PROPERTIES)?;
    assert!(current_values["SUMMARY"].contains("River"));
    assert!(current_values["SUMMARY"].contains("Clinic"));
    Ok(())
}

#[test]
#[allow(clippy::too_many_lines)]
fn consumer_preserves_combined_and_per_person_identities_and_uid_namespaces() -> Result {
    let mut document = support::fixture()?;
    document.settings.overlap_policy = eutheto_types::OverlapPolicy::Earlier;
    let mut second_person = document
        .domain
        .entities
        .get(&id(1).parse()?)
        .ok_or("fixture person missing")?
        .clone();
    second_person["id"] = Value::String(id(13));
    second_person["externalId"] = Value::String("staff-02".to_owned());
    document
        .domain
        .entities
        .insert(id(13).parse()?, second_person);
    let assignments = vec![
        assignment(1, 7, true)?,
        assignment(1, 8, true)?,
        assignment(13, 7, false)?,
        assignment(13, 8, true)?,
    ];
    let result = portable_with_assignments(101, 1, assignments.clone())?;
    let expected: BTreeSet<_> = [(1, 7), (1, 8), (13, 8)]
        .into_iter()
        .map(|(person, shift)| {
            (
                format!("{KIND}.{}.{}", id(person), id(shift)),
                id(person),
                id(shift),
            )
        })
        .collect();
    let bytes = encode_workforce_ics(
        &document,
        WorkforceExportSource::Terminal(&result),
        None,
        2,
        export_time()?,
        &control(),
    )?;
    let combined = parse_calendar(&bytes)?;
    calendar_values(&combined, true, false)?;
    let combined_uids = event_uids(&combined)?;
    assert_eq!(
        combined_uids.keys().cloned().collect::<BTreeSet<_>>(),
        expected
    );
    for person in [1, 13] {
        let person_identity = id(person);
        let filtered_bytes = encode_workforce_ics(
            &document,
            WorkforceExportSource::Terminal(&result),
            Some(id(person).parse()?),
            2,
            "2026-11-03T00:00:00Z".parse()?,
            &control(),
        )?;
        let filtered = parse_calendar(&filtered_bytes)?;
        let metadata = calendar_values(&filtered, true, true)?;
        assert_eq!(metadata["X-EUTHETO-PERSON-ID"], person_identity);
        let expected_person: BTreeMap<_, _> = combined_uids
            .iter()
            .filter(|((_, person_id, _), _)| person_id == &person_identity)
            .map(|(identity, uid)| (identity.clone(), uid.clone()))
            .collect();
        assert_eq!(event_uids(&filtered)?, expected_person);
        for event in &filtered.events {
            let values = property_values(&event.properties, EVENT_PROPERTIES)?;
            assert_eq!(
                utc_timestamp(values["DTSTAMP"])?,
                "2026-11-03T00:00:00Z".parse::<jiff::Timestamp>()?
            );
        }
    }
    for (solution_index, revision) in [(201, 1), (301, 2)] {
        let newer = portable_with_assignments(solution_index, revision, assignments.clone())?;
        let newer_bytes = encode_workforce_ics(
            &document,
            WorkforceExportSource::Terminal(&newer),
            None,
            revision,
            export_time()?,
            &control(),
        )?;
        let newer_calendar = parse_calendar(&newer_bytes)?;
        let metadata = calendar_values(&newer_calendar, false, false)?;
        assert_eq!(
            metadata["X-EUTHETO-SCENARIO-REVISION"],
            revision.to_string()
        );
        let newer_uids = event_uids(&newer_calendar)?;
        assert_eq!(
            newer_uids.keys().cloned().collect::<BTreeSet<_>>(),
            expected
        );
        assert!(
            combined_uids
                .values()
                .all(|old| newer_uids.values().all(|new| old != new)),
            "new accepted results, including later revisions, need distinct UID namespaces"
        );
        assert!(
            newer_uids
                .values()
                .all(|uid| !uid.contains(&newer.result_id.to_string()))
        );
    }
    Ok(())
}

#[test]
fn historical_optimal_status_is_exported_as_accepted_not_proof() -> Result {
    let mut document = support::fixture()?;
    document.settings.overlap_policy = eutheto_types::OverlapPolicy::Earlier;
    let mut result = portable()?;
    let manifest = &result.run_manifest;
    let RunTerminalOutcomeV1::Accepted {
        solution_id,
        accepted_result_checksum,
        verification_checksum,
        ..
    } = &manifest.outcome
    else {
        return Err("portable result has no accepted outcome".into());
    };
    let run_manifest = RunManifest::new(
        manifest.run_id,
        manifest.run_input_checksum.clone(),
        RunTerminalOutcomeV1::Accepted {
            status: SolveStatus::Optimal,
            solution_id: *solution_id,
            accepted_result_checksum: accepted_result_checksum.clone(),
            verification_checksum: verification_checksum.clone(),
        },
        manifest.started_at,
        manifest.finished_at,
        manifest.elapsed_milliseconds,
        manifest.first_incumbent_milliseconds,
        manifest.first_verified_feasible_milliseconds,
        manifest.phase_timings,
        manifest.verification_warnings.clone(),
    )?;
    result = PortableAcceptedResult::new(
        result.run_input.clone(),
        run_manifest,
        result.accepted_result.clone(),
        result.evidence.clone(),
    )?;
    result.validate()?;
    let optimal_bytes = encode_workforce_ics(
        &document,
        WorkforceExportSource::Terminal(&result),
        None,
        1,
        export_time()?,
        &control(),
    )?;
    let calendar = parse_calendar(&optimal_bytes)?;
    let metadata = calendar_values(&calendar, false, false)?;
    assert_eq!(metadata["X-EUTHETO-STATUS"], "accepted");
    assert_eq!(metadata["X-EUTHETO-HISTORICAL-PROOF"], "UNVERIFIED");
    Ok(())
}

#[test]
fn unknown_person_filter_is_rejected_without_partial_calendar() -> Result {
    let mut document = support::fixture()?;
    document.settings.overlap_policy = eutheto_types::OverlapPolicy::Earlier;
    let result = portable()?;
    let unknown = id(12).parse()?;
    let error = encode_workforce_ics(
        &document,
        WorkforceExportSource::Terminal(&result),
        Some(unknown),
        1,
        export_time()?,
        &control(),
    )
    .err()
    .ok_or("unknown person must not produce a calendar")?;
    assert_eq!(error.code, AssignmentsIcsErrorCode::MissingPerson);
    Ok(())
}

#[test]
fn hostile_person_text_is_escaped_and_folded_without_new_properties() -> Result {
    let mut document = support::fixture()?;
    document.settings.overlap_policy = eutheto_types::OverlapPolicy::Earlier;
    let person_id = id(1).parse()?;
    let person = document
        .domain
        .entities
        .get_mut(&person_id)
        .ok_or("fixture person missing")?;
    *person
        .get_mut("name")
        .ok_or("fixture person name missing")? = Value::String(format!(
        "{} ,;\\\\ Ω:BEGIN:VEVENT UID:injected X-EUTHETO-RESULT-ID:private",
        "界".repeat(40)
    ));
    let bytes = encode_workforce_ics(
        &document,
        WorkforceExportSource::Terminal(&portable()?),
        None,
        1,
        export_time()?,
        &control(),
    )?;
    assert!(bytes.windows(2).all(|pair| pair != &b"\n\n"[..]));
    for line in bytes.split(|byte| *byte == b'\n') {
        std::str::from_utf8(line)?;
        assert!(line.len() <= 76, "CRLF content line exceeded 75 octets");
    }
    let text = std::str::from_utf8(&bytes)?;
    assert!(text.contains("\r\n 界"), "multibyte TEXT must cross a fold");
    let calendar = parse_calendar(&bytes)?;
    calendar_values(&calendar, false, false)?;
    assert_eq!(calendar.events.len(), 1);
    let event = calendar
        .events
        .first()
        .ok_or("hostile-text event missing")?;
    let values = property_values(&event.properties, EVENT_PROPERTIES)?;
    // IcalParser unfolds but does not unescape TEXT: compare the escaped value explicitly.
    assert!(values["SUMMARY"].contains(&format!(
        "{} \\,\\;\\\\\\\\ Ω:BEGIN:VEVENT UID:injected X-EUTHETO-RESULT-ID:private",
        "界".repeat(40)
    )));
    assert!(values["SUMMARY"].contains("Clinic"));
    let identities = event_uids(&calendar)?;
    assert_eq!(
        identities.keys().cloned().collect::<BTreeSet<_>>(),
        BTreeSet::from([(format!("{KIND}.{}.{}", id(1), id(8)), id(1), id(8))])
    );
    Ok(())
}

#[test]
fn newline_person_text_is_rejected_by_source_validation() -> Result {
    let mut document = support::fixture()?;
    document.settings.overlap_policy = eutheto_types::OverlapPolicy::Earlier;
    let person_id = id(1).parse()?;
    let person = document
        .domain
        .entities
        .get_mut(&person_id)
        .ok_or("fixture person missing")?;
    *person
        .get_mut("name")
        .ok_or("fixture person name missing")? = Value::String("A\nB".to_owned());
    let error = encode_workforce_ics(
        &document,
        WorkforceExportSource::Terminal(&portable()?),
        None,
        1,
        export_time()?,
        &control(),
    )
    .err()
    .ok_or("person display names reject newlines before codec escaping")?;
    assert_eq!(error.code, AssignmentsIcsErrorCode::InvalidSource);
    Ok(())
}

#[test]
fn checkpoint_consumer_preserves_feasible_calendar_bytes_and_person_scope() -> Result {
    let mut document = support::fixture()?;
    document.settings.overlap_policy = eutheto_types::OverlapPolicy::Earlier;
    let terminal = portable()?;
    let checkpoint = checkpoint(&terminal)?;
    for revision in [1, 2] {
        for person in [None, Some(id(1).parse()?)] {
            let bytes = encode_workforce_ics(
                &document,
                WorkforceExportSource::Checkpoint(&checkpoint),
                person,
                revision,
                export_time()?,
                &control(),
            )?;
            let terminal_bytes = encode_workforce_ics(
                &document,
                WorkforceExportSource::Terminal(&terminal),
                person,
                revision,
                export_time()?,
                &control(),
            )?;
            assert_eq!(bytes, terminal_bytes);
            let calendar = parse_calendar(&bytes)?;
            let metadata = calendar_values(&calendar, revision != 1, person.is_some())?;
            assert_eq!(metadata["X-EUTHETO-STATUS"], "feasible");
            assert_eq!(calendar.events.len(), 1);
            assert_eq!(
                event_uids(&calendar)?
                    .keys()
                    .cloned()
                    .collect::<BTreeSet<_>>(),
                BTreeSet::from([(format!("{KIND}.{}.{}", id(1), id(8)), id(1), id(8))])
            );
        }
    }
    Ok(())
}

#[test]
fn malformed_checkpoint_and_terminal_bindings_refuse_calendar_output() -> Result {
    let document = support::fixture()?;
    let terminal = portable()?;
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
        let error = encode_workforce_ics(&document, source, None, 1, export_time()?, &control())
            .err()
            .ok_or("malformed source must not produce a calendar")?;
        assert_eq!(error.code, AssignmentsIcsErrorCode::InvalidResult);
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
            let error =
                encode_workforce_ics(source_document, source, None, 1, export_time()?, &control())
                    .err()
                    .ok_or("source mismatch must not produce a calendar")?;
            assert_eq!(error.code, AssignmentsIcsErrorCode::InvalidResult);
        }
    }
    Ok(())
}
