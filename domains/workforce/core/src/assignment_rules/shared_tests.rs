use crate::test_support as support;

use super::{
    AssignmentRuleError, AssignmentRuleLimit, InstantInterval,
    budget::{MAX_WORK_STEPS, OperationBudget},
    intervals::availability_intervals,
};
use crate::{
    model::Availability,
    temporal::{TemporalIssue, TemporalIssueKind},
};
use eutheto_domain_api::DomainPackError;
use eutheto_planning_ir::PlanningIrLimitsV1;
use eutheto_types::{
    CancellationToken, GapPolicy, OverlapPolicy, ScenarioSettings, TimeResolutionFailureKind,
};
use serde::{Serialize, Serializer};
use serde_json::{Value, json};
use std::error::Error;

type Result<T = ()> = std::result::Result<T, Box<dyn Error>>;

fn interval(start: &str, end: &str) -> Result<InstantInterval> {
    Ok(InstantInterval {
        start: start.parse()?,
        end: end.parse()?,
    })
}

fn settings(zone: &str) -> Result<ScenarioSettings> {
    let mut settings = support::fixture()?.settings;
    settings.time_zone = zone.parse()?;
    Ok(settings)
}

fn availability(start: &str, end: &str, window: Value) -> Result<Availability> {
    let mut record = json!({
        "id":support::id(42), "personId":support::id(1), "availabilityKind":"availableOnly",
        "effectiveRange":{"startDate":start,"endDateExclusive":end},
        "source":"test", "note":""
    });
    record
        .as_object_mut()
        .ok_or("availability record")?
        .insert("timeWindow".to_owned(), window);
    Ok(serde_json::from_value(record)?)
}

#[test]
fn effective_end_clips_both_instant_and_overnight_weekly_windows() -> Result {
    let settings = settings("UTC")?;
    let shift = interval("2026-11-01T22:00:00Z", "2026-11-02T06:00:00Z")?;
    let expected = interval("2026-11-01T22:00:00Z", "2026-11-02T00:00:00Z")?;
    for window in [
        json!({"kind":"instant", "startsAt":"2026-11-01T22:00:00Z", "endsAt":"2026-11-02T06:00:00Z"}),
        json!({"kind":"weekly", "windows":[{"weekdays":["sunday"], "startTime":"22:00:00", "endTime":"06:00:00", "endDayOffset":1}]}),
    ] {
        let record = availability("2026-11-01", "2026-11-02", window)?;
        let result = availability_intervals(
            &record,
            shift,
            &settings,
            &mut OperationBudget::evaluation(None),
        )?;
        assert_eq!(result.query, Some(expected));
        assert_eq!(result.intervals, [expected]);
    }
    Ok(())
}

#[test]
fn pre_effective_overnight_tail_is_retained_but_touching_end_is_not() -> Result {
    let settings = settings("UTC")?;
    let record = availability(
        "2026-11-01",
        "2026-11-02",
        json!({"kind":"weekly", "windows":[{
            "weekdays":["saturday"], "startTime":"23:00:00", "endTime":"02:00:00", "endDayOffset":1
        }]}),
    )?;
    let result = availability_intervals(
        &record,
        interval("2026-11-01T00:00:00Z", "2026-11-01T03:00:00Z")?,
        &settings,
        &mut OperationBudget::evaluation(None),
    )?;
    assert_eq!(
        result.intervals,
        [interval("2026-11-01T00:00:00Z", "2026-11-01T02:00:00Z")?]
    );
    let touching = availability_intervals(
        &record,
        interval("2026-11-01T02:00:00Z", "2026-11-01T03:00:00Z")?,
        &settings,
        &mut OperationBudget::evaluation(None),
    )?;
    assert_eq!(touching.intervals, []);
    let outside = availability_intervals(
        &record,
        interval("2026-11-02T00:00:00Z", "2026-11-02T01:00:00Z")?,
        &settings,
        &mut OperationBudget::evaluation(None),
    )?;
    assert_eq!(outside.query, None);
    Ok(())
}

#[test]
fn skipped_friday_can_constrain_saturday_under_explicit_gap_movement() -> Result {
    let mut settings = settings("Pacific/Apia")?;
    settings.gap_policy = GapPolicy::MoveForward;
    let record = availability(
        "2011-12-29",
        "2012-01-02",
        json!({"kind":"weekly", "windows":[{
            "weekdays":["friday"], "startTime":"01:00:00", "endTime":"02:00:00", "endDayOffset":0
        }]}),
    )?;
    let query = interval("2011-12-30T11:30:00Z", "2011-12-30T11:45:00Z")?;
    let result = availability_intervals(
        &record,
        query,
        &settings,
        &mut OperationBudget::evaluation(None),
    )?;
    assert_eq!(result.intervals, [query]);
    Ok(())
}

#[test]
fn unrelated_gap_is_ignored_but_relevant_gap_requires_review() -> Result {
    let settings = settings("America/New_York")?;
    let record = availability(
        "2026-03-01",
        "2026-04-01",
        json!({"kind":"weekly", "windows":[{
            "weekdays":["sunday"], "startTime":"02:30:00", "endTime":"03:30:00", "endDayOffset":0
        }]}),
    )?;
    let irrelevant = availability_intervals(
        &record,
        interval("2026-03-09T16:00:00Z", "2026-03-09T17:00:00Z")?,
        &settings,
        &mut OperationBudget::evaluation(None),
    )?;
    assert_eq!(irrelevant.intervals, []);
    assert!(matches!(
        availability_intervals(
            &record,
            interval("2026-03-08T07:00:00Z", "2026-03-08T08:00:00Z")?,
            &settings,
            &mut OperationBudget::evaluation(None)
        ),
        Err(AssignmentRuleError::Temporal(TemporalIssue {
            kind: TemporalIssueKind::Resolution(TimeResolutionFailureKind::Gap),
            ..
        }))
    ));
    Ok(())
}

#[test]
fn full_u8_lookback_preserves_relevant_old_occurrence_end_ambiguity() -> Result {
    let mut settings = settings("America/New_York")?;
    let record = availability(
        "2026-10-31",
        "2026-11-02",
        json!({"kind":"weekly", "windows":[{
            "weekdays":["thursday"], "startTime":"00:00:00", "endTime":"01:30:00", "endDayOffset":255
        }]}),
    )?;
    let query = interval("2026-11-01T05:15:00Z", "2026-11-01T05:20:00Z")?;
    // The Thursday 2026-02-19 occurrence ends in the 2026-11-01 fold. A seven-day
    // lookback misses its unresolved endpoint even though newer long windows also overlap.
    assert!(matches!(
        availability_intervals(
            &record,
            query,
            &settings,
            &mut OperationBudget::evaluation(None)
        ),
        Err(AssignmentRuleError::Temporal(TemporalIssue {
            kind: TemporalIssueKind::Resolution(TimeResolutionFailureKind::Overlap),
            ..
        }))
    ));
    settings.overlap_policy = OverlapPolicy::Earlier;
    let resolved = availability_intervals(
        &record,
        query,
        &settings,
        &mut OperationBudget::evaluation(None),
    )?;
    assert_eq!(resolved.intervals, [query]);
    Ok(())
}

#[test]
fn exact_resource_limits_fail_safely_without_claiming_expired_deadlines() -> Result {
    let limits = PlanningIrLimitsV1 {
        max_provenance_records: 2,
        max_total_refs: 3,
        max_ir_bytes: 5,
        ..PlanningIrLimitsV1::DEFAULT
    };
    for (records, items, bytes, excess, expected) in [
        (2, 0, 0, (1, 0, 0), AssignmentRuleLimit::Records),
        (0, 3, 0, (0, 1, 0), AssignmentRuleLimit::References),
        (0, 0, 5, (0, 0, 1), AssignmentRuleLimit::Bytes),
    ] {
        let mut budget = OperationBudget::analysis(None, limits);
        budget.reserve(records, items, bytes)?;
        let error = budget
            .reserve(excess.0, excess.1, excess.2)
            .err()
            .ok_or("one-over must fail")?;
        assert_eq!(error, AssignmentRuleError::LimitExceeded(expected));
        assert_eq!(
            error.validation_issue().severity,
            eutheto_types::ValidationSeverity::Error
        );
        assert!(matches!(
            DomainPackError::from(error),
            DomainPackError::ResourceLimitExceeded
        ));
    }
    let mut budget = OperationBudget::analysis(None, limits);
    assert_eq!(budget.measure("abc")?, 5);
    budget.reserve(0, 0, 5)?;
    assert_eq!(
        budget.measure(&0),
        Err(AssignmentRuleError::LimitExceeded(
            AssignmentRuleLimit::Bytes
        ))
    );
    assert_eq!(budget.measure_ir(&1234)?, 4);
    budget.reserve_ir(1, 1, 4)?;
    assert_eq!(
        budget.measure_ir(&12),
        Err(AssignmentRuleError::LimitExceeded(
            AssignmentRuleLimit::Bytes
        ))
    );
    Ok(())
}

#[test]
fn cancellation_wins_at_checkpoint_and_none_retains_finite_work() -> Result {
    let cancellation = CancellationToken::new();
    let control = eutheto_types::OperationControl::Cancellation(cancellation.clone());
    let mut controlled = OperationBudget::evaluation(Some(&control));
    controlled.steps(MAX_WORK_STEPS)?;
    cancellation.cancel();
    assert_eq!(controlled.step(), Err(AssignmentRuleError::Cancelled));
    let mut bounded = OperationBudget::evaluation(None);
    bounded.steps(MAX_WORK_STEPS)?;
    assert_eq!(
        bounded.step(),
        Err(AssignmentRuleError::LimitExceeded(
            AssignmentRuleLimit::WorkSteps
        ))
    );
    Ok(())
}

#[test]
fn cancellation_during_serialization_is_not_retried_as_interrupted_io() {
    struct Cancel<'a>(&'a CancellationToken);
    impl Serialize for Cancel<'_> {
        fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
            self.0.cancel();
            serializer.serialize_str("bounded")
        }
    }
    let cancellation = CancellationToken::new();
    let control = eutheto_types::OperationControl::Cancellation(cancellation.clone());
    let budget = OperationBudget::evaluation(Some(&control));
    assert_eq!(
        budget.measure(&Cancel(&cancellation)),
        Err(AssignmentRuleError::Cancelled)
    );
}

#[test]
fn selection_identity_rejection_does_not_depend_on_active_authored_rules() -> Result {
    use super::{SelectionIssueKind, input::AssignmentInput};
    use crate::model::AssignmentPair;
    let mut document = support::fixture()?;
    document.settings.overlap_policy = OverlapPolicy::Earlier;
    let template = document
        .domain
        .entities
        .get_mut(&support::id(6).parse()?)
        .ok_or("missing template")?;
    template["occurrenceIdentities"][support::id(55)] = json!({
        "id":support::id(55), "localStartDate":"2026-11-08"
    });
    let mut budget = OperationBudget::evaluation(None);
    let input = AssignmentInput::new(&document, &mut budget)?;
    let expected_hash = blake3::hash(&serde_json::to_vec(&document)?)
        .to_hex()
        .to_string();
    assert_eq!(input.source_document_hash, expected_hash);
    let valid = AssignmentPair {
        person_id: support::id(1).parse()?,
        shift_id: support::id(7).parse()?,
    };
    input.validate_selection(&[valid], &mut budget)?;
    for (pairs, expected) in [
        (vec![valid, valid], SelectionIssueKind::DuplicatePair),
        (
            vec![AssignmentPair {
                person_id: support::id(99).parse()?,
                ..valid
            }],
            SelectionIssueKind::MissingPerson,
        ),
        (
            vec![AssignmentPair {
                person_id: support::id(4).parse()?,
                ..valid
            }],
            SelectionIssueKind::WrongKindPerson,
        ),
        (
            vec![AssignmentPair {
                shift_id: support::id(55).parse()?,
                ..valid
            }],
            SelectionIssueKind::UnresolvedShift,
        ),
        (
            vec![AssignmentPair {
                shift_id: support::id(56).parse()?,
                ..valid
            }],
            SelectionIssueKind::UnresolvedShift,
        ),
    ] {
        let result = input.validate_selection(&pairs, &mut OperationBudget::evaluation(None));
        assert!(
            matches!(result, Err(AssignmentRuleError::InvalidSelection { kind, .. }) if kind == expected)
        );
    }
    Ok(())
}

#[test]
fn dormant_recurrence_indexes_obey_aggregate_retention_limits() -> Result {
    let mut document = support::fixture()?;
    document.domain.entities.remove(&support::id(8).parse()?);
    document.domain.locked_assignments.clear();
    let template = document
        .domain
        .entities
        .get_mut(&support::id(6).parse()?)
        .ok_or("missing template")?;
    template["recurrence"]["excludedDates"] = json!(
        (1..=10)
            .map(|day| format!("2026-11-{day:02}"))
            .collect::<Vec<_>>()
    );
    let domain = crate::validation::validate_document(&document)?;
    // One occurrence owner and eleven auxiliary keys, even though no shift is emitted.
    let exact = PlanningIrLimitsV1 {
        max_provenance_records: 12,
        max_total_refs: 13,
        max_ir_bytes: 304,
        ..PlanningIrLimitsV1::DEFAULT
    };
    let resolve = |limits| {
        let mut budget = OperationBudget::analysis(None, limits);
        crate::temporal::resolve_validated_shifts(&domain, &document.settings, &mut |step| {
            budget.resolution_step(step)
        })
    };
    assert!(resolve(exact)?.is_empty());
    for (limits, kind) in [
        (
            PlanningIrLimitsV1 {
                max_provenance_records: 11,
                ..exact
            },
            AssignmentRuleLimit::Records,
        ),
        (
            PlanningIrLimitsV1 {
                max_total_refs: 12,
                ..exact
            },
            AssignmentRuleLimit::References,
        ),
        (
            PlanningIrLimitsV1 {
                max_ir_bytes: 303,
                ..exact
            },
            AssignmentRuleLimit::Bytes,
        ),
    ] {
        assert_eq!(
            resolve(limits),
            Err(AssignmentRuleError::LimitExceeded(kind))
        );
    }
    Ok(())
}

#[test]
fn weekly_expansion_observes_real_cancellation_inside_date_iteration() -> Result {
    let record = availability(
        "2026-11-01",
        "2026-11-02",
        json!({
            "kind":"weekly", "windows":[{"weekdays":["friday"], "startTime":"08:00:00",
            "endTime":"10:00:00", "endDayOffset":255}]
        }),
    )?;
    let settings = settings("UTC")?;
    let shift = interval("2026-11-01T08:00:00Z", "2026-11-01T10:00:00Z")?;
    let token = CancellationToken::new();
    let control = eutheto_types::OperationControl::Cancellation(token.clone());
    let mut budget = OperationBudget::evaluation(Some(&control));
    budget.cancel_after_steps(5)?;
    let result = availability_intervals(&record, shift, &settings, &mut budget);
    assert!(matches!(result, Err(AssignmentRuleError::Cancelled)));
    assert!(token.is_cancelled());
    Ok(())
}

#[test]
fn negative_numeric_ceilings_are_invalid_configuration_not_exhaustion() {
    for limits in [
        PlanningIrLimitsV1 {
            max_abs_value: -1,
            ..PlanningIrLimitsV1::DEFAULT
        },
        PlanningIrLimitsV1 {
            max_abs_coefficient: -1,
            ..PlanningIrLimitsV1::DEFAULT
        },
    ] {
        assert!(matches!(
            super::budget::effective_limits(limits),
            Err(AssignmentRuleError::InvalidConstruction(
                super::AssignmentConstructionIssue::InvalidRecord
            ))
        ));
    }
}

#[test]
fn resource_bounded_validation_never_calls_the_document_malformed() -> Result {
    let mut document = support::fixture()?;
    document.metadata.description =
        "x".repeat(eutheto_domain_api::ContractJsonLimits::DEFAULT.max_string_bytes + 1);
    let report = eutheto_domain_api::DomainPack::validate_fast(&crate::WorkforcePack, &document);
    assert_eq!(report.issues.len(), 1);
    assert_eq!(report.issues[0].code, "official.workforce.limit.resource");
    assert_eq!(
        report.issues[0].severity,
        eutheto_types::ValidationSeverity::Error
    );
    let control = eutheto_types::OperationControl::Cancellation(CancellationToken::new());
    assert_eq!(
        eutheto_domain_api::DomainPack::validate_full(&crate::WorkforcePack, &document, &control),
        Err(DomainPackError::ResourceLimitExceeded)
    );
    Ok(())
}
