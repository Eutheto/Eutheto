mod support;

use eutheto_types::{
    CancellationToken, GapPolicy, Horizon, OverlapPolicy, ScenarioDocument,
    TimeResolutionFailureKind,
};
use eutheto_workforce::{
    commands,
    ids::ShiftId,
    temporal::{
        GenerationPreview, PriorShift, ResolvedShiftOrigin, ShiftChangeKind, TemporalError,
        TemporalIssueKind, preview_generation, resolve_shifts,
    },
};
use jiff::SignedDuration;
use serde_json::{Value, json};
use std::error::Error;
use support::{fixture, id};

type Result<T = ()> = std::result::Result<T, Box<dyn Error>>;

fn template(document: &mut ScenarioDocument) -> Result<&mut Value> {
    document
        .domain
        .entities
        .get_mut(&id(6).parse()?)
        .ok_or_else(|| "missing template".into())
}

fn day(date: &str, start: &str, end: &str, timing: Value) -> Result<ScenarioDocument> {
    let mut document = fixture()?;
    document.domain.locked_assignments.clear();
    document.domain.entities.remove(&id(8).parse()?);
    document.settings.horizon = Horizon::new(start.parse()?, end.parse()?)?;
    let record = template(&mut document)?;
    record["recurrence"] = json!({"weekdays":["monday","tuesday","wednesday","thursday","friday","saturday","sunday"],"effectiveRange":{"startDate":"2026-01-01","endDateExclusive":"2027-01-01"},"excludedDates":[]});
    record["timing"] = timing;
    record["occurrenceIdentities"] = json!({(id(7)):{"id":id(7),"localStartDate":date}});
    Ok(document)
}

fn november(timing: Value) -> Result<ScenarioDocument> {
    day(
        "2026-11-01",
        "2026-11-01T04:00:00Z",
        "2026-11-02T05:00:00Z",
        timing,
    )
}

fn preview(before: &ScenarioDocument, after: &ScenarioDocument) -> Result<GenerationPreview> {
    Ok(preview_generation(
        before,
        after,
        &CancellationToken::new(),
    )?)
}

fn assert_kind<T>(result: std::result::Result<T, TemporalError>, expected: TemporalIssueKind) {
    let actual = result.err().and_then(|error| match error {
        TemporalError::Issue(issue) => Some(issue.kind),
        TemporalError::InvalidDocument(_) | TemporalError::Cancelled => None,
    });
    assert_eq!(actual, Some(expected));
}

#[test]
fn final_inclusive_start_date_keeps_complete_overnight_and_effective_exclusions() -> Result {
    let mut document = november(
        json!({"kind":"localWindow","startTime":"22:00:00","endTime":"06:00:00","endDayOffset":1}),
    )?;
    document.settings.horizon.end = "2026-11-05T05:00:00Z".parse()?;
    let record = template(&mut document)?;
    record["recurrence"]["excludedDates"] = json!(["2026-11-02"]);
    record["recurrence"]["effectiveRange"]["endDateExclusive"] = json!("2026-11-04");
    let reviewed = preview(&document, &document)?;
    assert_eq!(
        reviewed
            .after
            .iter()
            .map(|shift| shift.reporting_date.to_string())
            .collect::<Vec<_>>(),
        ["2026-11-01", "2026-11-03"]
    );
    assert_eq!(
        reviewed.after[1].interval.ends_at.instant,
        "2026-11-04T11:00:00Z".parse()?
    );
    let batch = reviewed
        .reconciliation
        .ok_or("expected missing identity command")?;
    assert_eq!(batch.commands.len(), 1);
    let mutation = commands::apply_batch(&document, &batch)?;
    assert_eq!(
        resolve_shifts(&mutation.document, &CancellationToken::new())?,
        reviewed.after
    );
    assert_eq!(
        commands::apply_batch(&mutation.document, &mutation.inverse)?.document,
        document
    );
    // Now the third is the final included horizon date: its overnight end is still retained.
    document.settings.horizon.end = "2026-11-04T05:00:00Z".parse()?;
    assert_eq!(preview(&document, &document)?.after, reviewed.after);
    Ok(())
}

#[test]
fn gap_and_both_fold_choices_retain_intent_and_exact_elapsed_duration() -> Result {
    let timing = json!({"kind":"elapsedDuration","startTime":"02:30:00","durationMinutes":60});
    let mut spring = day(
        "2026-03-08",
        "2026-03-08T05:00:00Z",
        "2026-03-09T04:00:00Z",
        timing,
    )?;
    assert_kind(
        resolve_shifts(&spring, &CancellationToken::new()),
        TemporalIssueKind::Resolution(TimeResolutionFailureKind::Gap),
    );
    spring.settings.gap_policy = GapPolicy::MoveForward;
    let moved = resolve_shifts(&spring, &CancellationToken::new())?[0];
    assert_eq!(
        moved.interval.starts_at.local.as_datetime(),
        "2026-03-08T02:30:00".parse()?
    );
    assert_eq!(
        moved.interval.starts_at.instant,
        "2026-03-08T07:30:00Z".parse()?
    );
    assert_eq!(
        moved.interval.ends_at.local.as_datetime(),
        "2026-03-08T04:30:00".parse()?
    );
    assert_eq!(
        moved.interval.elapsed_duration(),
        SignedDuration::from_hours(1)
    );
    assert_eq!(
        moved.interval.scheduled_duration(),
        SignedDuration::from_hours(2)
    );
    let mut fall =
        november(json!({"kind":"elapsedDuration","startTime":"01:30:00","durationMinutes":60}))?;
    assert_kind(
        resolve_shifts(&fall, &CancellationToken::new()),
        TemporalIssueKind::Resolution(TimeResolutionFailureKind::Overlap),
    );
    fall.settings.overlap_policy = OverlapPolicy::Earlier;
    let earlier = resolve_shifts(&fall, &CancellationToken::new())?[0];
    fall.settings.overlap_policy = OverlapPolicy::Later;
    let later = resolve_shifts(&fall, &CancellationToken::new())?[0];
    assert_eq!(earlier.id, later.id);
    assert_eq!(
        earlier.interval.starts_at.instant,
        "2026-11-01T05:30:00Z".parse()?
    );
    assert_eq!(
        later.interval.starts_at.instant,
        "2026-11-01T06:30:00Z".parse()?
    );
    assert_eq!(earlier.interval.scheduled_duration(), SignedDuration::ZERO);
    assert_eq!(
        later.interval.scheduled_duration(),
        SignedDuration::from_hours(1)
    );
    assert_eq!(
        earlier.interval.elapsed_duration(),
        later.interval.elapsed_duration()
    );
    Ok(())
}

#[test]
fn wall_clock_windows_differ_from_elapsed_duration_across_both_transitions() -> Result {
    for (date, start, end, elapsed_hours) in [
        (
            "2026-03-08",
            "2026-03-08T05:00:00Z",
            "2026-03-09T04:00:00Z",
            2,
        ),
        (
            "2026-11-01",
            "2026-11-01T04:00:00Z",
            "2026-11-02T05:00:00Z",
            4,
        ),
    ] {
        let document = day(
            date,
            start,
            end,
            json!({"kind":"localWindow","startTime":"00:30:00","endTime":"03:30:00","endDayOffset":0}),
        )?;
        let shift = resolve_shifts(&document, &CancellationToken::new())?[0];
        assert_eq!(
            shift.interval.scheduled_duration(),
            SignedDuration::from_hours(3)
        );
        assert_eq!(
            shift.interval.elapsed_duration(),
            SignedDuration::from_hours(elapsed_hours)
        );
    }
    Ok(())
}

#[test]
fn repair_preserves_unresolved_prior_identity_without_inventing_old_instants() -> Result {
    let before =
        november(json!({"kind":"elapsedDuration","startTime":"01:30:00","durationMinutes":60}))?;
    let mut after = before.clone();
    after.settings.overlap_policy = OverlapPolicy::Later;
    let reviewed = preview(&before, &after)?;
    assert!(matches!(
        reviewed.before.as_slice(),
        [PriorShift::Unresolved {
            issue: TemporalIssueKind::Resolution(TimeResolutionFailureKind::Overlap),
            ..
        }]
    ));
    assert_eq!(reviewed.after[0].id, id(7).parse()?);
    assert_eq!(reviewed.changes[0].kind, ShiftChangeKind::Changed);
    assert!(reviewed.reconciliation.is_none());
    let mut hasher = blake3::Hasher::new();
    serde_json::to_writer(&mut hasher, &after)?;
    assert_eq!(reviewed.prospective_hash, *hasher.finalize().as_bytes());
    after.settings.locale = "fr-FR".parse()?;
    let localized = preview(&before, &after)?;
    assert_eq!(localized.after, reviewed.after);
    assert_ne!(localized.prospective_hash, reviewed.prospective_hash);
    Ok(())
}

#[test]
fn copied_or_imported_ids_are_reused_and_replacements_conflict() -> Result {
    let mut before =
        november(json!({"kind":"elapsedDuration","startTime":"08:00:00","durationMinutes":60}))?;
    let copied: ShiftId = id(987).parse()?;
    let raw_key = copied.to_string().to_uppercase();
    template(&mut before)?["occurrenceIdentities"] =
        json!({(raw_key.clone()):{"id":copied,"localStartDate":"2026-11-01"}});
    let stable = preview(&before, &before)?;
    assert_eq!(stable.after[0].id, copied);
    assert!(stable.reconciliation.is_none());
    let mut missing = before.clone();
    template(&mut missing)?["occurrenceIdentities"] = json!({});
    let reused = preview(&before, &missing)?;
    assert_eq!(reused.after[0].id, copied);
    assert!(reused.changes.is_empty());
    let mutation = commands::apply_batch(
        &missing,
        &reused.reconciliation.ok_or("expected restore command")?,
    )?;
    assert_eq!(mutation.document, before);
    template(&mut missing)?["occurrenceIdentities"] =
        json!({(id(988)):{"id":id(988),"localStartDate":"2026-11-01"}});
    assert_kind(
        preview_generation(&before, &missing, &CancellationToken::new()),
        TemporalIssueKind::IdentityTransition,
    );
    Ok(())
}

#[test]
fn derived_id_collisions_include_removed_and_extension_owned_identities() -> Result {
    let mut empty =
        november(json!({"kind":"elapsedDuration","startTime":"08:00:00","durationMinutes":60}))?;
    template(&mut empty)?["occurrenceIdentities"] = json!({});
    let candidate = preview(&empty, &empty)?.after[0].id;
    let mut occupied = empty.clone();
    occupied.extensions.insert(
        "nonsemantic.example.owned".parse()?,
        json!({(candidate.to_string()):{"id":candidate}}),
    );
    assert_kind(
        preview_generation(&occupied, &empty, &CancellationToken::new()),
        TemporalIssueKind::IdentityCollision,
    );
    assert_kind(
        preview_generation(&empty, &occupied, &CancellationToken::new()),
        TemporalIssueKind::IdentityCollision,
    );
    let mut before = empty.clone();
    template(&mut before)?["occurrenceIdentities"] =
        json!({(id(7)):{"id":id(7),"localStartDate":"2026-11-01"}});
    let mut after = empty.clone();
    after.extensions.insert(
        "nonsemantic.example.owned".parse()?,
        json!({(id(7)):{"id":id(7)}}),
    );
    assert_kind(
        preview_generation(&before, &after, &CancellationToken::new()),
        TemporalIssueKind::IdentityCollision,
    );
    Ok(())
}

#[test]
fn manual_and_detached_shifts_keep_exact_intervals_and_start_membership() -> Result {
    let before = fixture()?;
    let instance = before
        .domain
        .entities
        .get(&id(8).parse()?)
        .ok_or("missing stored shift")?
        .clone();
    let mut after = before.clone();
    template(&mut after)?["timing"]["startTime"] = json!("09:00:00");
    let reviewed = preview(&before, &after)?;
    let manual_id: ShiftId = id(8).parse()?;
    let manual = reviewed
        .after
        .iter()
        .find(|shift| shift.id == manual_id)
        .ok_or("missing manual result")?;
    assert_eq!(
        manual.interval.starts_at.instant,
        "2026-11-01T05:30:00Z".parse()?
    );
    assert_eq!(
        manual.interval.ends_at.instant,
        "2026-11-01T07:30:00Z".parse()?
    );
    assert!(reviewed.changes.iter().all(|change| change.id != manual.id));
    let mut detached = instance;
    detached["id"] = json!(id(7));
    detached["origin"] =
        json!({"kind":"detached","templateId":id(6),"occurrenceDate":"2026-11-01"});
    let batch = eutheto_domain_api::DomainBatchCommand {
        schema_version: eutheto_domain_api::DOMAIN_BATCH_SCHEMA_VERSION,
        pack_id: before.domain_pack.id.clone(),
        scenario_schema_version: 1,
        label: None,
        commands: vec![eutheto_types::DomainCommandEnvelope {
            command_type: commands::DETACH_SHIFT.to_owned(),
            payload: json!({"templateId":id(6),"instance":detached}),
        }],
    };
    let mut detached_document = commands::apply_batch(&before, &batch)?.document;
    // An out-of-horizon edited one-off still suppresses its original occurrence date.
    let stored = detached_document
        .domain
        .entities
        .get_mut(&id(7).parse()?)
        .ok_or("missing detached")?;
    stored["startsAt"] = json!({"instant":"2026-11-02T05:00:00Z","local":"2026-11-02T00:00:00","offsetSeconds":-18000});
    stored["endsAt"] = json!({"instant":"2026-11-02T06:00:00Z","local":"2026-11-02T01:00:00","offsetSeconds":-18000});
    let reviewed = preview(&detached_document, &detached_document)?;
    assert!(reviewed.reconciliation.is_none());
    assert_eq!(
        reviewed
            .after
            .iter()
            .map(|shift| shift.id)
            .collect::<Vec<_>>(),
        [id(8).parse()?]
    );
    assert_eq!(reviewed.after[0].origin, ResolvedShiftOrigin::Manual);
    Ok(())
}

#[test]
fn set_reordering_and_locale_do_not_change_shift_semantics() -> Result {
    let mut before =
        november(json!({"kind":"elapsedDuration","startTime":"08:00:00","durationMinutes":60}))?;
    template(&mut before)?["tags"] = json!(["a", "b"]);
    let mut after = before.clone();
    after.settings.locale = "fr-FR".parse()?;
    template(&mut after)?["tags"] = json!(["b", "a"]);
    template(&mut after)?["recurrence"]["weekdays"]
        .as_array_mut()
        .ok_or("missing weekdays")?
        .reverse();
    let reviewed = preview(&before, &after)?;
    assert!(reviewed.changes.is_empty());
    assert_eq!(
        reviewed.after,
        resolve_shifts(&before, &CancellationToken::new())?
    );
    template(&mut after)?["coverage"]["count"] = json!(2);
    assert_eq!(
        preview(&before, &after)?.changes[0].kind,
        ShiftChangeKind::Changed
    );
    Ok(())
}

#[test]
fn dormant_definitions_survive_exclusion_and_horizon_repairs() -> Result {
    let before =
        november(json!({"kind":"elapsedDuration","startTime":"08:00:00","durationMinutes":60}))?;
    let mut excluded = before.clone();
    template(&mut excluded)?["recurrence"]["excludedDates"] = json!(["2026-11-01"]);
    let reviewed = preview(&before, &excluded)?;
    assert!(reviewed.after.is_empty());
    assert!(reviewed.reconciliation.is_none());
    assert_eq!(reviewed.changes[0].kind, ShiftChangeKind::Removed);
    assert_eq!(preview(&excluded, &before)?.after[0].id, id(7).parse()?);
    let mut oversized = before.clone();
    oversized.settings.horizon.end = "2040-01-01T05:00:00Z".parse()?;
    template(&mut oversized)?["recurrence"]["effectiveRange"]["endDateExclusive"] =
        json!("2040-01-01");
    // Prior state is bounded by recorded definitions, not missing historical intents.
    assert_eq!(preview(&oversized, &before)?.after[0].id, id(7).parse()?);
    assert_kind(
        preview_generation(&oversized, &oversized, &CancellationToken::new()),
        TemporalIssueKind::OccurrenceLimit,
    );
    Ok(())
}

#[test]
fn generation_observes_cancellation_and_scenario_identity() -> Result {
    let before =
        november(json!({"kind":"elapsedDuration","startTime":"08:00:00","durationMinutes":60}))?;
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    assert!(matches!(
        resolve_shifts(&before, &cancellation),
        Err(TemporalError::Cancelled)
    ));
    assert!(matches!(
        preview_generation(&before, &before, &cancellation),
        Err(TemporalError::Cancelled)
    ));
    let mut other = before.clone();
    other.scenario_id = id(101).parse()?;
    assert_kind(
        preview_generation(&before, &other, &CancellationToken::new()),
        TemporalIssueKind::DifferentScenario,
    );
    Ok(())
}

#[test]
fn stored_and_dormant_occurrence_owners_cannot_replace_prior_identity() -> Result {
    for detached in [true, false] {
        let mut before = november(
            json!({"kind":"elapsedDuration","startTime":"08:00:00","durationMinutes":60}),
        )?;
        let date = if detached { "2026-11-01" } else { "2026-11-02" };
        template(&mut before)?["occurrenceIdentities"] =
            json!({(id(7)):{"id":id(7),"localStartDate":date}});
        let mut after = before.clone();
        template(&mut after)?["occurrenceIdentities"] = json!({});
        if detached {
            let mut instance = fixture()?
                .domain
                .entities
                .remove(&id(8).parse()?)
                .ok_or("missing instance")?;
            instance["id"] = json!(id(988));
            instance["origin"] =
                json!({"kind":"detached","templateId":id(6),"occurrenceDate":date});
            after.domain.entities.insert(id(988).parse()?, instance);
        } else {
            template(&mut after)?["occurrenceIdentities"] =
                json!({(id(988)):{"id":id(988),"localStartDate":date}});
        }
        assert_kind(
            preview_generation(&before, &after, &CancellationToken::new()),
            TemporalIssueKind::IdentityTransition,
        );
    }
    Ok(())
}

#[test]
fn generated_date_membership_differs_from_stored_instant_membership_at_midnight_fold() -> Result {
    let mut document =
        november(json!({"kind":"elapsedDuration","startTime":"00:30:00","durationMinutes":60}))?;
    document.settings.time_zone = "America/Havana".parse()?;
    document.settings.horizon.start = "2026-11-01T05:00:00Z".parse()?;
    document.settings.overlap_policy = OverlapPolicy::Earlier;
    let generated = resolve_shifts(&document, &CancellationToken::new())?;
    assert_eq!(
        generated[0].interval.starts_at.instant,
        "2026-11-01T04:30:00Z".parse()?
    );
    assert_eq!(generated[0].reporting_date.to_string(), "2026-11-01");
    assert_eq!(preview(&document, &document)?.after, generated);
    let mut manual = fixture()?
        .domain
        .entities
        .remove(&id(8).parse()?)
        .ok_or("missing manual")?;
    manual["startsAt"] = serde_json::to_value(generated[0].interval.starts_at)?;
    manual["endsAt"] = serde_json::to_value(generated[0].interval.ends_at)?;
    document.domain.entities.insert(id(8).parse()?, manual);
    assert_eq!(
        resolve_shifts(&document, &CancellationToken::new())?,
        generated
    );
    Ok(())
}

#[test]
fn a_gap_move_beyond_the_final_local_start_date_is_rejected() -> Result {
    let mut document = day(
        "2011-12-30",
        "2011-12-29T10:00:00Z",
        "2011-12-30T10:00:00Z",
        json!({"kind":"elapsedDuration","startTime":"12:00:00","durationMinutes":60}),
    )?;
    document.settings.time_zone = "Pacific/Apia".parse()?;
    document.settings.gap_policy = GapPolicy::MoveForward;
    template(&mut document)?["recurrence"]["effectiveRange"] =
        json!({"startDate":"2011-12-30","endDateExclusive":"2011-12-31"});
    assert_kind(
        resolve_shifts(&document, &CancellationToken::new()),
        TemporalIssueKind::OutsideHorizon,
    );
    Ok(())
}

#[test]
fn stored_duration_preserves_fractional_negative_and_large_civil_spans() -> Result {
    let mut document = fixture()?;
    document.domain.locked_assignments.clear();
    document.domain.entities.remove(&id(6).parse()?);
    let manual = document
        .domain
        .entities
        .get_mut(&id(8).parse()?)
        .ok_or("missing manual")?;
    manual["startsAt"] = json!({"instant":"2026-11-01T05:50:00.123456789Z","local":"2026-11-01T01:50:00.123456789","offsetSeconds":-14400});
    manual["endsAt"] = json!({"instant":"2026-11-01T06:10:00.987654321Z","local":"2026-11-01T01:10:00.987654321","offsetSeconds":-18000});
    let interval = resolve_shifts(&document, &CancellationToken::new())?[0].interval;
    let fraction = SignedDuration::from_nanos(864_197_532);
    assert_eq!(
        interval.elapsed_duration(),
        SignedDuration::from_secs(1200) + fraction
    );
    assert_eq!(
        interval.scheduled_duration(),
        SignedDuration::from_secs(-2400) + fraction
    );

    document.settings.time_zone = "UTC".parse()?;
    document.settings.horizon = Horizon::new(
        "2026-01-01T00:00:00Z".parse()?,
        "2026-01-02T00:00:00Z".parse()?,
    )?;
    let manual = document
        .domain
        .entities
        .get_mut(&id(8).parse()?)
        .ok_or("missing manual")?;
    manual["startsAt"] =
        json!({"instant":"2026-01-01T08:00:00Z","local":"2026-01-01T08:00:00","offsetSeconds":0});
    manual["endsAt"] =
        json!({"instant":"2526-01-01T08:00:00Z","local":"2526-01-01T08:00:00","offsetSeconds":0});
    let interval = resolve_shifts(&document, &CancellationToken::new())?[0].interval;
    // Five hundred Gregorian years contain 182,621 days, exceeding an i64 nanosecond span.
    assert_eq!(
        interval.elapsed_duration(),
        SignedDuration::from_secs(15_778_454_400)
    );
    assert_eq!(interval.scheduled_duration(), interval.elapsed_duration());
    Ok(())
}

#[test]
fn qualification_minimum_set_order_does_not_create_false_generation_changes() -> Result {
    let mut before =
        november(json!({"kind":"elapsedDuration","startTime":"08:00:00","durationMinutes":60}))?;
    before.domain.entities.insert(
        id(12).parse()?,
        json!({"kind":"qualification","id":id(12),"name":"Second qualification","description":""}),
    );
    let minimums = json!([
        {"qualifications":{"allQualificationIds":[id(11),id(12)],"anyQualificationIds":[]},"minimum":1},
        {"qualifications":{"allQualificationIds":[],"anyQualificationIds":[id(11),id(12)]},"minimum":1}
    ]);
    template(&mut before)?["coverage"] =
        json!({"kind":"exact","count":2,"qualificationMinimums":minimums});
    let mut after = before.clone();
    let minimums = template(&mut after)?["coverage"]["qualificationMinimums"]
        .as_array_mut()
        .ok_or("missing minimums")?;
    minimums.reverse();
    for minimum in minimums {
        minimum["qualifications"]["allQualificationIds"]
            .as_array_mut()
            .ok_or("missing all set")?
            .reverse();
        minimum["qualifications"]["anyQualificationIds"]
            .as_array_mut()
            .ok_or("missing any set")?
            .reverse();
    }
    assert!(preview(&before, &after)?.changes.is_empty());
    template(&mut after)?["coverage"]["qualificationMinimums"][0]["minimum"] = json!(2);
    assert_eq!(
        preview(&before, &after)?.changes[0].kind,
        ShiftChangeKind::Changed
    );
    Ok(())
}
