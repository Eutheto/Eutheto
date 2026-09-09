//! Original-domain assertions against fixed source intent, never a rebuilt-document echo.
use super::{
    BASELINE_REQUIREMENT, EVERY_DAY, REPAIR_REQUIREMENT, WEEKDAYS, all, date_range, expected, id,
    selected, type_scope, wall,
};
use crate::contract::{
    CORPUS_VERSION, CaseId, ExpectedCase, FIXTURE_CLOCK, FORMAT_VERSION, PreservedObligation,
    REPAIR_FORMAT, RepairFixture,
};
use anyhow::{Context, Result, bail, ensure};
use eutheto_domain_api::{DOMAIN_BATCH_SCHEMA_VERSION, DomainBatchCommand};
use eutheto_types::{
    CancellationToken, DomainCommandEnvelope, EntityId, GapPolicy, OverlapPolicy, PersonId,
    Revision, RuleId, ScenarioDocument, ScenarioSnapshotV1, TimeResolutionFailureKind, UnitSystem,
};
use eutheto_workforce::{
    commands,
    ids::{
        AssignmentTypeId, LocationId, QualificationId, ShiftTemplateId, WorkloadBucketId,
        WorkloadPolicyId,
    },
    model::{
        ActiveRange, AssignmentLock, AvailabilityKind, CalendarPeriod, ConsecutiveMode, Coverage,
        DeviationPenalty, LocalInterval, LocationBehavior, LockState, OverlappingContribution,
        PeerGroup, PersonSelection, PreferenceDirection, PreferencePriority,
        QualificationExpression, ReportingAttribution, RequiredStrength, Scope, ShiftTiming,
        TieBreakPolicy, TimeBehavior, TimeWindow, Weekday, WeeklyWindow, WindowMembership,
        WorkWindow, WorkforceDomainV1, WorkforceEntity, WorkforcePreference, WorkforceRule,
        WorkforceScorePolicy, WorkloadMeasurement, WorkloadTargetMode, WorkloadWeight,
        planning_dates,
    },
    temporal,
    validation::validate_document,
};
use jiff::{Span, civil::Date};
use std::collections::BTreeSet;

fn entity<'a>(
    case: CaseId,
    domain: &'a WorkforceDomainV1,
    role: &str,
) -> Result<&'a WorkforceEntity> {
    domain
        .entities
        .get(&id::<EntityId>(case, role)?)
        .with_context(|| format!("missing source entity {role}"))
}
fn source_rule<'a>(
    case: CaseId,
    domain: &'a WorkforceDomainV1,
    role: &str,
) -> Result<&'a WorkforceRule> {
    let value = domain
        .rules
        .get(&id::<RuleId>(case, role)?)
        .with_context(|| format!("missing source rule {role}"))?;
    ensure!(value.header().1, "source rule {role} must remain active");
    Ok(value)
}
fn source_preference<'a>(
    case: CaseId,
    domain: &'a WorkforceDomainV1,
    role: &str,
) -> Result<&'a WorkforcePreference> {
    let value = domain
        .preferences
        .get(&id::<RuleId>(case, role)?)
        .with_context(|| format!("missing source preference {role}"))?;
    ensure!(
        value.header().1,
        "source preference {role} must remain active"
    );
    Ok(value)
}
fn check_basic(case: CaseId, domain: &WorkforceDomainV1) -> Result<()> {
    ensure!(
        matches!(source_rule(case, domain, "rule-eligibility")?, WorkforceRule::Eligibility { strength: RequiredStrength::Required, scope, .. } if *scope == all()),
        "Required all-population eligibility changed"
    );
    ensure!(
        matches!(source_rule(case, domain, "rule-availability")?, WorkforceRule::Availability { strength: RequiredStrength::Required, scope, .. } if *scope == all()),
        "Required all-population availability changed"
    );
    ensure!(
        matches!(source_rule(case, domain, "rule-coverage")?, WorkforceRule::Coverage { strength: RequiredStrength::Required, scope, .. } if *scope == all()),
        "Required all-population coverage changed"
    );
    ensure!(
        matches!(source_rule(case, domain, "rule-no-overlap")?, WorkforceRule::NoOverlap { strength: RequiredStrength::Required, scope, compatible_category_pairs, .. } if *scope == all() && compatible_category_pairs.is_empty()),
        "Required no-overlap compatibility changed"
    );
    Ok(())
}
fn check_person(
    case: CaseId,
    domain: &WorkforceDomainV1,
    role: &str,
    name: &str,
    types: &[&str],
    qualifications: &[&str],
    weight: (u32, u32),
) -> Result<()> {
    let WorkforceEntity::Person(person) = entity(case, domain, role)? else {
        bail!("{role} is not a person")
    };
    ensure!(
        person.name == name && person.active_range == ActiveRange::Always {},
        "source person name/activity changed"
    );
    ensure!(
        person.external_id.is_none()
            && person.home_location_id.is_none()
            && person.workload_target.is_none()
            && person.display.is_none()
            && person.tags.is_empty()
            && person.team_ids.is_empty(),
        "invented optional person semantics"
    );
    ensure!(
        person.workload_weight
            == WorkloadWeight {
                numerator: weight.0,
                denominator: weight.1
            },
        "source participation weight changed"
    );
    let expected_types: BTreeSet<AssignmentTypeId> = types
        .iter()
        .map(|role| id(case, role))
        .collect::<Result<_>>()?;
    ensure!(
        person
            .eligible_assignment_type_ids
            .iter()
            .copied()
            .collect::<BTreeSet<_>>()
            == expected_types,
        "source type eligibility changed"
    );
    let expected_grants: BTreeSet<QualificationId> = qualifications
        .iter()
        .map(|role| id(case, role))
        .collect::<Result<_>>()?;
    ensure!(
        person
            .qualification_grants
            .iter()
            .map(|grant| grant.qualification_id)
            .collect::<BTreeSet<_>>()
            == expected_grants,
        "source qualification grants changed"
    );
    ensure!(
        person
            .qualification_grants
            .iter()
            .all(|grant| grant.effective_from.is_none() && grant.expires_at.is_none()),
        "source-absent grant bounds invented"
    );
    Ok(())
}
fn check_type(
    case: CaseId,
    domain: &WorkforceDomainV1,
    role: &str,
    name: &str,
    category: &str,
    duration: u32,
    buckets: &[&str],
) -> Result<()> {
    let WorkforceEntity::AssignmentType(record) = entity(case, domain, role)? else {
        bail!("{role} is not an assignment type")
    };
    let bucket_ids: BTreeSet<WorkloadBucketId> = buckets
        .iter()
        .map(|role| id(case, role))
        .collect::<Result<_>>()?;
    ensure!(
        record.name == name
            && record.category == category
            && record.default_duration_minutes == duration,
        "source assignment-type meaning changed"
    );
    ensure!(
        record.time_behavior == TimeBehavior::LocalWallClock
            && record.location_behavior == LocationBehavior::Optional {}
            && record.qualifications == QualificationExpression::Unconstrained {},
        "source-absent assignment defaults changed"
    );
    ensure!(
        record
            .workload_bucket_ids
            .iter()
            .copied()
            .collect::<BTreeSet<_>>()
            == bucket_ids,
        "countsToward membership changed"
    );
    Ok(())
}
fn check_coverage(
    case: CaseId,
    coverage: &Coverage,
    count_expected: u16,
    minima: &[(&str, u16)],
) -> Result<()> {
    let Coverage::Exact {
        count,
        qualification_minimums,
    } = coverage
    else {
        bail!("source exact coverage changed")
    };
    ensure!(
        *count == count_expected && qualification_minimums.len() == minima.len(),
        "source coverage count/minimum inventory changed"
    );
    for (role, minimum) in minima {
        let qualification_id = id::<QualificationId>(case, role)?;
        ensure!(
            qualification_minimums
                .iter()
                .any(|entry| entry.minimum == *minimum
                    && entry.qualifications.all_qualification_ids == [qualification_id]
                    && entry.qualifications.any_qualification_ids.is_empty()),
            "source qualification minimum changed"
        );
    }
    Ok(())
}
// Independent source expectations: deliberately not the generation TemplateSpec.
struct TemplateExpectation<'a> {
    role: &'a str,
    type_role: &'a str,
    start: &'a str,
    end: &'a str,
    weekdays: &'a [Weekday],
    timing: ShiftTiming,
    count: u16,
    minima: &'a [(&'a str, u16)],
    location: bool,
    occurrences: usize,
}

fn check_template(
    case: CaseId,
    domain: &WorkforceDomainV1,
    expectation: &TemplateExpectation<'_>,
) -> Result<()> {
    let TemplateExpectation {
        role,
        type_role,
        start,
        end,
        weekdays,
        ref timing,
        count,
        minima,
        location,
        occurrences,
    } = *expectation;
    let WorkforceEntity::ShiftTemplate(record) = entity(case, domain, role)? else {
        bail!("source template replaced by cached intervals")
    };
    let name = match role {
        "template-weekday-clinic" => "Weekday morning clinic",
        "template-nightly-call" => "Nightly overnight call",
        "template-transition" => "Sunday transition duty",
        "template-synthetic" => "Daily unpruned duty",
        _ => bail!("unknown source template role"),
    };
    ensure!(record.name == name, "source template name changed");
    ensure!(
        record.assignment_type_id == id(case, type_role)?,
        "template type changed"
    );
    let expected_location = if location {
        Some(id::<LocationId>(case, "loc-main")?)
    } else {
        None
    };
    ensure!(
        record.location_id == expected_location
            && record.tags.is_empty()
            && record.reporting_attribution == ReportingAttribution::StartLocalDate,
        "template location/tags/reporting changed"
    );
    ensure!(
        record.recurrence.effective_range == date_range(start, end)?
            && record.recurrence.weekdays == weekdays
            && record.recurrence.excluded_dates.is_empty(),
        "source recurrence changed"
    );
    ensure!(record.timing == *timing, "source local timing changed");
    ensure!(
        record.occurrence_identities.len() == occurrences,
        "source occurrence population changed"
    );
    check_coverage(case, &record.coverage, count, minima)
}
fn check_day_off(
    case: CaseId,
    domain: &WorkforceDomainV1,
    appendix: bool,
    end: &str,
) -> Result<()> {
    let (role, person_role, starts, ends, source, note) = if appendix {
        (
            "availability-smith-thursday",
            "person-smith",
            "2026-09-10T05:00:00Z",
            "2026-09-11T05:00:00Z",
            "synthetic-appendix-f",
            "Requested day off",
        )
    } else {
        (
            "availability-day-off",
            "person-0",
            "2026-09-03T05:00:00Z",
            "2026-09-04T05:00:00Z",
            "synthetic-corpus",
            "Synthetic requested day off",
        )
    };
    let WorkforceEntity::Availability(value) = entity(case, domain, role)? else {
        bail!("missing concrete day off")
    };
    ensure!(
        value.person_id == id(case, person_role)?
            && value.availability_kind == AvailabilityKind::Unavailable,
        "day-off population/kind changed"
    );
    ensure!(
        value.time_window
            == TimeWindow::Instant {
                starts_at: starts.parse()?,
                ends_at: ends.parse()?
            }
            && value.effective_range == date_range("2026-09-01", end)?,
        "day-off local-day intersection changed"
    );
    ensure!(
        value.assignment_type_ids.is_none()
            && value.location_ids.is_none()
            && value.source == source
            && value.note == note,
        "day-off restrictions/source changed"
    );
    Ok(())
}
fn check_rest(case: CaseId, domain: &WorkforceDomainV1, people: PersonSelection) -> Result<()> {
    let WorkforceRule::MinimumRest(value) = source_rule(case, domain, "rule-rest-call-to-clinic")?
    else {
        bail!("missing elapsed rest obligation")
    };
    ensure!(
        value.strength == RequiredStrength::Required
            && value.minimum_minutes == 600
            && value.scope == Scope { people, ..all() },
        "600-minute rest strength/population changed"
    );
    ensure!(
        value.after_scope == type_scope(case, "type-call-overnight")?
            && value.before_scope == type_scope(case, "type-clinic-am")?,
        "directional call-to-clinic rest changed"
    );
    Ok(())
}
fn check_hours(
    case: CaseId,
    domain: &WorkforceDomainV1,
    role: &str,
    duration: u32,
    maximum: u32,
) -> Result<()> {
    let WorkforceRule::MaximumHours {
        strength,
        scope,
        bucket_id,
        window,
        maximum_minutes,
        ..
    } = source_rule(case, domain, role)?
    else {
        bail!("missing rolling-hours obligation")
    };
    ensure!(
        *strength == RequiredStrength::Required
            && *scope == all()
            && *bucket_id == id(case, "all-hours")?
            && *maximum_minutes == maximum
            && *window
                == WorkWindow::Rolling {
                    duration_minutes: duration,
                    membership: WindowMembership::Intersection
                },
        "rolling clipped elapsed hours changed"
    );
    Ok(())
}
fn check_score(
    case: CaseId,
    domain: &WorkforceDomainV1,
    balance: Option<(&str, PersonSelection, &str, &str)>,
) -> Result<()> {
    let WorkforceEntity::ScorePolicy(policy) = entity(case, domain, "score-policy")? else {
        bail!("missing score policy")
    };
    ensure!(
        policy.profile_key == "balanced"
            && policy.tie_break == TieBreakPolicy::StableAssignmentRank,
        "profile or separate final rank changed"
    );
    let levels: Vec<_> = policy
        .levels
        .iter()
        .map(|level| (level.level_key.as_str(), level.label.as_str()))
        .collect();
    ensure!(
        levels
            == [
                ("stability", "Stability"),
                ("high-preferences", "High preferences"),
                ("fairness", "Fairness"),
                ("normal-preferences", "Normal preferences")
            ],
        "logical five-tier score order changed or rank duplicated"
    );
    ensure!(
        policy.priority_mapping.len() == 4,
        "priority mapping inventory changed"
    );
    for (priority, key, scale) in [
        (PreferencePriority::VeryHigh, "high-preferences", 1),
        (PreferencePriority::High, "fairness", 1),
        (PreferencePriority::Normal, "normal-preferences", 2),
        (PreferencePriority::Low, "normal-preferences", 1),
    ] {
        ensure!(
            policy
                .priority_mapping
                .iter()
                .any(|entry| entry.priority == priority
                    && entry.level_key == key
                    && entry.scale == scale),
            "fixture priority mapping changed"
        );
    }
    check_balance(case, domain, policy, balance)
}

fn check_balance(
    case: CaseId,
    domain: &WorkforceDomainV1,
    policy: &WorkforceScorePolicy,
    balance: Option<(&str, PersonSelection, &str, &str)>,
) -> Result<()> {
    if let Some((bucket_role, people, start, end)) = balance {
        ensure!(
            policy.workload_policies.len() == 1,
            "balance policy inventory changed"
        );
        let value = policy
            .workload_policies
            .get(&id::<WorkloadPolicyId>(case, "policy-balance")?)
            .context("missing balance policy")?;
        ensure!(
            value.bucket_id == id(case, bucket_role)?
                && value.calendar_id == id(case, "calendar-plan")?
                && value.membership == WindowMembership::ReportingDate,
            "whole-plan balance bucket/window changed"
        );
        ensure!(
            value.peer_group
                == PeerGroup {
                    people,
                    team_ids: None
                }
                && value.target_mode == WorkloadTargetMode::WeightedEqualShare {}
                && value.penalty == DeviationPenalty::Absolute {},
            "weighted balance selector/target/penalty changed"
        );
        let WorkforceEntity::Calendar(calendar) = entity(case, domain, "calendar-plan")? else {
            bail!("missing whole-plan calendar")
        };
        ensure!(
            calendar.period
                == CalendarPeriod::Custom {
                    intervals: vec![LocalInterval {
                        starts_at: format!("{start}T00:00:00").parse()?,
                        ends_at: format!("{end}T00:00:00").parse()?
                    }]
                },
            "whole-plan custom reporting window changed"
        );
        let WorkforcePreference::WorkloadBalance {
            scope,
            priority,
            weight,
            workload_policy_id,
            ..
        } = source_preference(case, domain, "pref-balance")?
        else {
            bail!("missing active balance preference")
        };
        ensure!(
            *scope == all()
                && *priority == PreferencePriority::High
                && *weight == 1
                && *workload_policy_id == value.id,
            "balance preference weight/tier changed"
        );
    } else {
        ensure!(
            policy.workload_policies.is_empty(),
            "supported slice gained a later workload policy"
        );
    }
    Ok(())
}
fn check_friday(case: CaseId, domain: &WorkforceDomainV1, person_role: &str) -> Result<()> {
    let WorkforcePreference::Time {
        scope,
        priority,
        weight,
        direction,
        time_window,
        ..
    } = source_preference(case, domain, "pref-no-friday")?
    else {
        bail!("missing whole-Friday preference")
    };
    ensure!(
        *scope
            == Scope {
                people: selected(case, &[person_role.to_owned()])?,
                ..all()
            }
            && *priority == PreferencePriority::Normal
            && *weight == 1
            && *direction == PreferenceDirection::Avoid,
        "Friday person/scope/tier changed"
    );
    ensure!(
        *time_window
            == TimeWindow::Weekly {
                windows: vec![WeeklyWindow {
                    weekdays: vec![Weekday::Friday],
                    start_time: "00:00:00".parse()?,
                    end_time: "00:00:00".parse()?,
                    end_day_offset: 1
                }]
            },
        "whole-Friday interval changed to a start-day selector"
    );
    Ok(())
}

pub(crate) fn validate_semantics(
    case: CaseId,
    snapshot: &ScenarioSnapshotV1,
    expectation: &ExpectedCase,
) -> Result<()> {
    // This expectation is a source constant, independently of the loaded/generated document.
    // No assignment solution is rebuilt, prescribed, or inferred from solver output here.
    ensure!(
        *expectation == expected(case)?,
        "reviewed expectation differs from fixed source contract"
    );
    let doc = &snapshot.document;
    let domain = validate_document(doc)?;
    check_snapshot(case, snapshot, &domain)?;
    let (people, days, shifts, entities, rules, preferences, start, end) = match case {
        CaseId::AppendixF => (3, 30, 52, 17, 7, 2, "2026-09-01", "2026-10-01"),
        CaseId::ClinicTiny => (3, 7, 5, 10, 4, 0, "2026-09-01", "2026-09-08"),
        CaseId::ClinicInitial => (8, 28, 20, 15, 4, 0, "2026-09-01", "2026-09-29"),
        CaseId::ClinicFull => (8, 28, 20, 16, 4, 2, "2026-09-01", "2026-09-29"),
        CaseId::ClinicOvernight => (12, 28, 48, 22, 5, 0, "2026-09-01", "2026-09-29"),
        CaseId::RollingHours => (8, 28, 20, 15, 6, 0, "2026-09-01", "2026-09-29"),
        CaseId::SpecialistCoverage => (6, 7, 5, 14, 4, 0, "2026-09-01", "2026-09-08"),
        CaseId::RepairBefore => (4, 7, 5, 10, 4, 0, "2026-09-01", "2026-09-08"),
        CaseId::InfeasibleCoverage => (3, 1, 1, 9, 4, 0, "2026-09-01", "2026-09-02"),
        CaseId::DstSpring => (2, 1, 1, 5, 4, 0, "2026-03-08", "2026-03-09"),
        CaseId::DstFall => (2, 1, 1, 5, 4, 0, "2026-11-01", "2026-11-02"),
        CaseId::LargeSupported => (100, 12, 12, 103, 4, 0, "2026-09-01", "2026-09-13"),
        CaseId::LargePressure => (100, 20, 20, 103, 4, 0, "2026-09-01", "2026-09-21"),
    };
    ensure!(
        domain.entities.len() == entities
            && domain.rules.len() == rules
            && domain.preferences.len() == preferences,
        "source entity/rule/preference inventory changed"
    );
    ensure!(
        domain
            .entities
            .values()
            .filter(|value| matches!(value, WorkforceEntity::Person(_)))
            .count()
            == people,
        "source population changed"
    );
    let dates = planning_dates(&doc.settings)?;
    ensure!(
        dates.first_date == start.parse::<Date>()?
            && dates.last_date == end.parse::<Date>()?.checked_sub(Span::new().days(1))?,
        "source civil horizon changed"
    );
    ensure!(
        start.parse::<Date>()?.checked_add(Span::new().days(days))? == end.parse::<Date>()?,
        "source horizon assertion is inconsistent"
    );
    let resolved = temporal::resolve_shifts(doc, &CancellationToken::new())?;
    ensure!(
        resolved.len() == shifts,
        "source resolved shift count changed"
    );
    ensure!(
        temporal::preview_generation(doc, doc, &CancellationToken::new())?
            .reconciliation
            .is_none(),
        "source occurrence identities are unreconciled"
    );
    ensure!(
        resolved.iter().all(|shift| matches!(
            shift.origin,
            temporal::ResolvedShiftOrigin::Generated { .. }
        )),
        "source recurrence replaced by manual intervals"
    );
    check_basic(case, &domain)?;
    if case != CaseId::RollingHours {
        ensure!(
            domain.locked_assignments.is_empty(),
            "source-absent lock invented"
        );
    }
    if matches!(case, CaseId::DstSpring | CaseId::DstFall) {
        check_dst(case, doc, &domain, &resolved)?;
    } else {
        ensure!(
            doc.settings.gap_policy == GapPolicy::Reject
                && doc.settings.overlap_policy == OverlapPolicy::Reject,
            "review-on-ambiguity policy changed"
        );
        if matches!(case, CaseId::LargeSupported | CaseId::LargePressure) {
            check_large(case, &domain, &resolved, end)?;
        } else if case == CaseId::AppendixF {
            check_appendix(doc, &domain, &resolved)?;
        } else {
            check_clinic(case, &domain, &resolved, people, end)?;
        }
    }
    check_deferred_obligations(&domain, expectation)?;
    Ok(())
}

fn check_snapshot(
    case: CaseId,
    snapshot: &ScenarioSnapshotV1,
    domain: &WorkforceDomainV1,
) -> Result<()> {
    let doc = &snapshot.document;
    ensure!(
        snapshot.revision == Revision::INITIAL && snapshot.required_capabilities.is_empty(),
        "source snapshot revision/capabilities changed"
    );
    ensure!(
        snapshot.project.is_none()
            && snapshot.semantic_extensions.is_empty()
            && snapshot.extensions.is_empty(),
        "source-absent snapshot metadata/extensions invented"
    );
    ensure!(
        doc.scenario_id.to_string() == id::<EntityId>(case, "scenario")?.to_string(),
        "source scenario identity changed"
    );
    ensure!(
        doc.domain_pack.id.as_str() == "official.workforce" && doc.domain_pack.schema_version == 1,
        "source pack changed"
    );
    ensure!(
        doc.settings.time_zone.as_str() == "America/Chicago"
            && doc.settings.locale.as_str() == "en-US"
            && doc.settings.units == UnitSystem::UsCustomary,
        "source time zone/locale/units changed"
    );
    ensure!(
        doc.metadata.created_at == FIXTURE_CLOCK.parse()?
            && doc.metadata.updated_at == FIXTURE_CLOCK.parse()?
            && doc.extensions.is_empty(),
        "fixed clock/extensions changed"
    );
    ensure!(
        !domain.entities.values().any(|value| matches!(
            value,
            WorkforceEntity::BaseSchedule(_)
                | WorkforceEntity::ShiftInstance(_)
                | WorkforceEntity::CoverageRequirement(_)
                | WorkforceEntity::Team(_)
        )),
        "unexpected baseline, cached shift, coverage override or team"
    );
    Ok(())
}

fn check_deferred_obligations(
    domain: &WorkforceDomainV1,
    expectation: &ExpectedCase,
) -> Result<()> {
    // Every deferred ID has already had its precise source values checked above. Also prove
    // the preservation inventory is exhaustive rather than merely descriptive prose.
    for obligation in &expectation.deferred_obligations {
        match obligation {
            PreservedObligation::Rule { rule_id, rule_kind } => {
                let value = domain.rules.get(rule_id).context("missing deferred rule")?;
                ensure!(
                    value.header().1 && serde_json::to_value(value)?["kind"] == *rule_kind,
                    "deferred rule kind/activation changed"
                );
            }
            PreservedObligation::Preference {
                rule_id,
                preference_kind,
            } => {
                let value = domain
                    .preferences
                    .get(rule_id)
                    .context("missing deferred preference")?;
                ensure!(
                    value.header().1 && serde_json::to_value(value)?["kind"] == *preference_kind,
                    "deferred preference kind/activation changed"
                );
            }
            PreservedObligation::Lock { assignment_id } => {
                ensure!(
                    matches!(
                        domain.locked_assignments.get(assignment_id),
                        Some(AssignmentLock {
                            state: LockState::Hard {},
                            ..
                        })
                    ),
                    "deferred hard lock changed"
                );
            }
        }
    }
    Ok(())
}

fn check_appendix(
    doc: &ScenarioDocument,
    domain: &WorkforceDomainV1,
    resolved: &[temporal::ResolvedShift],
) -> Result<()> {
    let case = CaseId::AppendixF;
    ensure!(
        doc.metadata.title == "September clinic and overnight call"
            && doc.metadata.description == "Example medical-practice schedule",
        "Appendix metadata changed"
    );
    check_appendix_population(domain)?;
    check_template(
        case,
        domain,
        &TemplateExpectation {
            role: "template-weekday-clinic",
            type_role: "type-clinic-am",
            start: "2026-09-01",
            end: "2026-10-01",
            weekdays: &WEEKDAYS,
            timing: wall("08:00:00", "12:00:00", 0)?,
            count: 2,
            minima: &[("q-physician", 2)],
            location: true,
            occurrences: 22,
        },
    )?;
    check_template(
        case,
        domain,
        &TemplateExpectation {
            role: "template-nightly-call",
            type_role: "type-call-overnight",
            start: "2026-09-01",
            end: "2026-10-01",
            weekdays: &EVERY_DAY,
            timing: wall("20:00:00", "06:00:00", 1)?,
            count: 1,
            minima: &[("q-call", 1)],
            location: false,
            occurrences: 30,
        },
    )?;
    let roles: Vec<_> = ["person-smith", "person-jones", "person-patel"]
        .into_iter()
        .map(str::to_owned)
        .collect();
    check_rest(case, domain, selected(case, &roles)?)?;
    check_hours(case, domain, "rule-max-hours-rolling", 20160, 4800)?;
    ensure!(
        matches!(source_rule(case, domain, "rule-max-consecutive-nights")?, WorkforceRule::MaximumConsecutive { strength: RequiredStrength::Required, scope, mode: ConsecutiveMode::Assignments { break_minutes: 840 }, maximum: 2, .. } if *scope == type_scope(case, "type-call-overnight")?),
        "Appendix consecutive-call assignment runs changed"
    );
    check_day_off(case, domain, true, "2026-10-01")?;
    check_score(
        case,
        domain,
        Some((
            "overnight-count",
            selected(case, &roles)?,
            "2026-09-01",
            "2026-10-01",
        )),
    )?;
    check_friday(case, domain, "person-jones")?;
    check_clinic_intervals(case, resolved, true, "2026-09-30")?;
    ensure!(
        doc.settings.horizon.start.to_string() == "2026-09-01T05:00:00Z"
            && doc.settings.horizon.end.to_string() == "2026-10-01T05:00:00Z",
        "Appendix exact horizon instants changed"
    );
    Ok(())
}

fn check_appendix_population(domain: &WorkforceDomainV1) -> Result<()> {
    let case = CaseId::AppendixF;
    for (role, name, weight) in [
        ("person-smith", "Dr. Smith", (1, 1)),
        ("person-jones", "Dr. Jones", (1, 1)),
        ("person-patel", "Dr. Patel", (4, 5)),
    ] {
        check_person(
            case,
            domain,
            role,
            name,
            &["type-clinic-am", "type-call-overnight"],
            &["q-physician", "q-call"],
            weight,
        )?;
    }
    for (role, name) in [
        ("q-physician", "Physician"),
        ("q-call", "Overnight call eligible"),
    ] {
        ensure!(
            matches!(entity(case, domain, role)?, WorkforceEntity::Qualification(value) if value.name == name && value.description.is_empty()),
            "Appendix qualification changed"
        );
    }
    ensure!(
        matches!(entity(case, domain, "loc-main")?, WorkforceEntity::Location(value) if value.name == "Main clinic" && value.transitions.is_empty()),
        "Appendix location/travel meaning changed"
    );
    for role in ["all-hours", "clinic-hours", "call-hours", "overnight-count"] {
        let measurement = if role == "overnight-count" {
            WorkloadMeasurement::AssignmentCount
        } else {
            WorkloadMeasurement::ElapsedMinutes
        };
        ensure!(
            matches!(entity(case, domain, role)?, WorkforceEntity::WorkloadBucket(value) if value.measurement == measurement && value.overlapping_contribution == OverlappingContribution::Sum),
            "Appendix countsToward measurement changed"
        );
    }
    check_type(
        case,
        domain,
        "type-clinic-am",
        "Morning clinic",
        "clinic",
        240,
        &["all-hours", "clinic-hours"],
    )?;
    check_type(
        case,
        domain,
        "type-call-overnight",
        "Overnight call",
        "call",
        600,
        &["all-hours", "call-hours", "overnight-count"],
    )?;
    Ok(())
}

fn check_clinic(
    case: CaseId,
    domain: &WorkforceDomainV1,
    resolved: &[temporal::ResolvedShift],
    people: usize,
    end: &str,
) -> Result<()> {
    let overnight = case == CaseId::ClinicOvernight;
    check_clinic_source(case, domain, people, end)?;
    if !matches!(case, CaseId::RepairBefore | CaseId::InfeasibleCoverage) {
        check_day_off(case, domain, false, end)?;
    }
    if overnight {
        check_type(
            case,
            domain,
            "type-call-overnight",
            "Overnight call",
            "call",
            600,
            &["all-hours"],
        )?;
        check_template(
            case,
            domain,
            &TemplateExpectation {
                role: "template-nightly-call",
                type_role: "type-call-overnight",
                start: "2026-09-01",
                end: "2026-09-29",
                weekdays: &EVERY_DAY,
                timing: wall("20:00:00", "06:00:00", 1)?,
                count: 1,
                minima: &[("q-call", 1)],
                location: false,
                occurrences: 28,
            },
        )?;
        check_rest(case, domain, PersonSelection::All {})?;
    }
    check_clinic_intervals(case, resolved, overnight, "2026-09-28")?;
    if case == CaseId::ClinicFull {
        check_score(
            case,
            domain,
            Some((
                "clinic-count",
                PersonSelection::All {},
                "2026-09-01",
                "2026-09-29",
            )),
        )?;
        check_friday(case, domain, "person-1")?;
    } else {
        check_score(case, domain, None)?;
    }
    if case == CaseId::RollingHours {
        check_hours(case, domain, "rule-hours-seven-days", 10080, 1200)?;
        check_hours(case, domain, "rule-hours-fourteen-days", 20160, 2400)?;
        ensure!(
            domain.locked_assignments.len() == 1,
            "rolling fixture hard-lock inventory changed"
        );
        let lock = domain
            .locked_assignments
            .get(&id(case, "lock-first-clinic")?)
            .context("missing first-clinic lock")?;
        let first = resolved
            .iter()
            .find(|shift| shift.reporting_date.to_string() == "2026-09-01")
            .context("missing first-clinic occurrence")?;
        ensure!(
            lock.person_id == id::<PersonId>(case, "person-0")?
                && lock.shift_id == first.id
                && lock.state == LockState::Hard {},
            "concrete rolling first-clinic hard lock changed"
        );
    }
    Ok(())
}

fn check_clinic_source(
    case: CaseId,
    domain: &WorkforceDomainV1,
    people: usize,
    end: &str,
) -> Result<()> {
    let overnight = case == CaseId::ClinicOvernight;
    for index in 0..people {
        let mut types = vec!["type-clinic-am"];
        let mut grants = vec!["q-physician"];
        if overnight {
            types.push("type-call-overnight");
            grants.push("q-call");
        }
        if case == CaseId::SpecialistCoverage && index < 2 {
            grants.push("q-specialist");
        }
        if case == CaseId::InfeasibleCoverage && index > 0 {
            types.clear();
        }
        check_person(
            case,
            domain,
            &format!("person-{index}"),
            &format!("Synthetic clinician {}", index + 1),
            &types,
            &grants,
            (1, 1),
        )?;
    }
    check_type(
        case,
        domain,
        "type-clinic-am",
        "Morning clinic",
        "clinic",
        240,
        &["all-hours", "clinic-count"],
    )?;
    for (role, measurement) in [
        ("all-hours", WorkloadMeasurement::ElapsedMinutes),
        ("clinic-count", WorkloadMeasurement::AssignmentCount),
    ] {
        ensure!(
            matches!(entity(case, domain, role)?, WorkforceEntity::WorkloadBucket(value) if value.measurement == measurement && value.overlapping_contribution == OverlappingContribution::Sum),
            "clinic workload measurement changed"
        );
    }
    let mut minima = vec![("q-physician", 2)];
    if case == CaseId::SpecialistCoverage {
        minima.push(("q-specialist", 1));
    }
    let count = match case {
        CaseId::InfeasibleCoverage => 1,
        CaseId::ClinicTiny | CaseId::SpecialistCoverage | CaseId::RepairBefore => 5,
        _ => 20,
    };
    check_template(
        case,
        domain,
        &TemplateExpectation {
            role: "template-weekday-clinic",
            type_role: "type-clinic-am",
            start: "2026-09-01",
            end,
            weekdays: &WEEKDAYS,
            timing: wall("08:00:00", "12:00:00", 0)?,
            count: 2,
            minima: &minima,
            location: false,
            occurrences: count,
        },
    )?;
    Ok(())
}

fn check_clinic_intervals(
    case: CaseId,
    shifts: &[temporal::ResolvedShift],
    overnight: bool,
    last_call: &str,
) -> Result<()> {
    let clinic_id = id::<ShiftTemplateId>(case, "template-weekday-clinic")?;
    let call_id = id::<ShiftTemplateId>(case, "template-nightly-call")?;
    for shift in shifts {
        let temporal::ResolvedShiftOrigin::Generated {
            template_id,
            occurrence_date,
        } = shift.origin
        else {
            bail!("not a generated clinic occurrence")
        };
        ensure!(
            shift.reporting_date == occurrence_date,
            "start-date reporting changed"
        );
        let (minutes, start, end) = if template_id == clinic_id {
            (240, "08:00:00", "12:00:00")
        } else {
            ensure!(
                overnight && template_id == call_id,
                "unexpected clinical occurrence owner"
            );
            (600, "20:00:00", "06:00:00")
        };
        ensure!(
            shift.interval.elapsed_duration() == jiff::SignedDuration::from_mins(minutes)
                && shift.interval.scheduled_duration() == jiff::SignedDuration::from_mins(minutes),
            "clinical elapsed/scheduled duration changed"
        );
        ensure!(
            shift
                .interval
                .starts_at
                .local
                .as_datetime()
                .time()
                .to_string()
                == start
                && shift
                    .interval
                    .ends_at
                    .local
                    .as_datetime()
                    .time()
                    .to_string()
                    == end,
            "clinical endpoint wall time changed"
        );
    }
    if overnight {
        let last = shifts.iter().find(|shift| matches!(shift.origin, temporal::ResolvedShiftOrigin::Generated { template_id, .. } if template_id == call_id) && shift.reporting_date.to_string() == last_call).context("missing final civil-date call")?;
        ensure!(
            last.interval.ends_at.local.as_datetime().date()
                == last_call
                    .parse::<Date>()?
                    .checked_add(Span::new().days(1))?,
            "last call tail clipped at horizon"
        );
        // 06:00 call finish to 08:00 next clinic is120 elapsed minutes, so the
        //600-minute required rest excludes that same-person pair, not the shifts themselves.
        let call = shifts.iter().find(|shift| matches!(shift.origin, temporal::ResolvedShiftOrigin::Generated { template_id, .. } if template_id == call_id) && shift.reporting_date.to_string() == "2026-09-01").context("missing directional rest call")?;
        let clinic = shifts.iter().find(|shift| matches!(shift.origin, temporal::ResolvedShiftOrigin::Generated { template_id, .. } if template_id == clinic_id) && shift.reporting_date.to_string() == "2026-09-02").context("missing directional rest clinic")?;
        ensure!(
            call.interval
                .ends_at
                .instant
                .as_timestamp()
                .duration_until(clinic.interval.starts_at.instant.as_timestamp())
                == jiff::SignedDuration::from_mins(120),
            "rest witness geometry changed"
        );
    }
    Ok(())
}
fn check_dst(
    case: CaseId,
    doc: &ScenarioDocument,
    domain: &WorkforceDomainV1,
    shifts: &[temporal::ResolvedShift],
) -> Result<()> {
    let spring = case == CaseId::DstSpring;
    for index in 0..2 {
        check_person(
            case,
            domain,
            &format!("person-{index}"),
            &format!("Transition clinician {}", index + 1),
            &["type-transition"],
            &[],
            (1, 1),
        )?;
    }
    let (start, end, begins, ends, elapsed, scheduled, instant_start, instant_end) = if spring {
        (
            "2026-03-08",
            "2026-03-09",
            "02:30:00",
            "04:30:00",
            60,
            120,
            "2026-03-08T08:30:00Z",
            "2026-03-08T09:30:00Z",
        )
    } else {
        (
            "2026-11-01",
            "2026-11-02",
            "01:30:00",
            "02:30:00",
            120,
            60,
            "2026-11-01T06:30:00Z",
            "2026-11-01T08:30:00Z",
        )
    };
    check_type(
        case,
        domain,
        "type-transition",
        "Transition duty",
        "transition",
        scheduled,
        &[],
    )?;
    check_template(
        case,
        domain,
        &TemplateExpectation {
            role: "template-transition",
            type_role: "type-transition",
            start,
            end,
            weekdays: &[Weekday::Sunday],
            timing: wall(begins, ends, 0)?,
            count: 1,
            minima: &[],
            location: false,
            occurrences: 1,
        },
    )?;
    let shift = shifts.first().context("missing transition shift")?;
    ensure!(
        shift.interval.elapsed_duration() == jiff::SignedDuration::from_mins(elapsed)
            && shift.interval.scheduled_duration()
                == jiff::SignedDuration::from_mins(i64::from(scheduled)),
        "transition elapsed/scheduled meaning changed"
    );
    ensure!(
        shift.interval.starts_at.instant.to_string() == instant_start
            && shift.interval.ends_at.instant.to_string() == instant_end,
        "transition fold/gap instant changed"
    );
    ensure!(
        shift
            .interval
            .starts_at
            .local
            .as_datetime()
            .time()
            .to_string()
            == begins,
        "original local intent was discarded"
    );
    check_dst_policies(doc, spring, shift)?;
    check_score(case, domain, None)
}

fn check_dst_policies(
    doc: &ScenarioDocument,
    spring: bool,
    shift: &temporal::ResolvedShift,
) -> Result<()> {
    ensure!(
        doc.settings.gap_policy
            == if spring {
                GapPolicy::MoveForward
            } else {
                GapPolicy::Reject
            }
            && doc.settings.overlap_policy
                == if spring {
                    OverlapPolicy::Reject
                } else {
                    OverlapPolicy::Earlier
                },
        "explicit gap/fold policy changed"
    );
    let mut rejected = doc.clone();
    rejected.settings.gap_policy = GapPolicy::Reject;
    rejected.settings.overlap_policy = OverlapPolicy::Reject;
    let failure = temporal::resolve_shifts(&rejected, &CancellationToken::new())
        .err()
        .context("ambiguous transition did not reject")?;
    let expected_kind = if spring {
        TimeResolutionFailureKind::Gap
    } else {
        TimeResolutionFailureKind::Overlap
    };
    ensure!(
        matches!(failure, temporal::TemporalError::Issue(temporal::TemporalIssue { kind: temporal::TemporalIssueKind::Resolution(kind), .. }) if kind == expected_kind),
        "wrong temporal rejection boundary"
    );
    if !spring {
        let mut later = doc.clone();
        later.settings.overlap_policy = OverlapPolicy::Later;
        let alternate = temporal::resolve_shifts(&later, &CancellationToken::new())?;
        let second = alternate.first().context("missing later fold")?;
        ensure!(
            second.id == shift.id
                && second.interval.elapsed_duration() == jiff::SignedDuration::from_mins(60)
                && second.interval.starts_at.instant.to_string() == "2026-11-01T07:30:00Z",
            "later fold identity/elapsed meaning changed"
        );
    }
    Ok(())
}

fn check_large(
    case: CaseId,
    domain: &WorkforceDomainV1,
    shifts: &[temporal::ResolvedShift],
    end: &str,
) -> Result<()> {
    for index in 0..100 {
        check_person(
            case,
            domain,
            &format!("person-{index}"),
            &format!("Synthetic person {}", index + 1),
            &["type-synthetic"],
            &[],
            (1, 1),
        )?;
    }
    check_type(
        case,
        domain,
        "type-synthetic",
        "Unpruned synthetic duty",
        "synthetic",
        60,
        &[],
    )?;
    let count = if case == CaseId::LargeSupported {
        12
    } else {
        20
    };
    check_template(
        case,
        domain,
        &TemplateExpectation {
            role: "template-synthetic",
            type_role: "type-synthetic",
            start: "2026-09-01",
            end,
            weekdays: &EVERY_DAY,
            timing: wall("08:00:00", "09:00:00", 0)?,
            count: 1,
            minima: &[],
            location: false,
            occurrences: count,
        },
    )?;
    for shift in shifts {
        ensure!(
            shift.interval.elapsed_duration() == jiff::SignedDuration::from_mins(60),
            "large synthetic duration changed"
        );
    }
    // Exact entity inventory plus the checked universally active/type-eligible roster and
    // unconstrained type prove all1200/2000 pairs survive input filtering. No later rules.
    check_score(case, domain, None)
}

pub(crate) fn validate_repair(recipe: &RepairFixture, before: &ScenarioSnapshotV1) -> Result<()> {
    validate_semantics(
        CaseId::RepairBefore,
        before,
        &expected(CaseId::RepairBefore)?,
    )?;
    ensure!(
        recipe.format == REPAIR_FORMAT
            && recipe.schema_version == FORMAT_VERSION
            && recipe.corpus_version == CORPUS_VERSION
            && recipe.before_case == CaseId::RepairBefore,
        "repair recipe identity/version changed"
    );
    ensure!(
        recipe.execution_phase == 7
            && recipe.baseline_requirement == BASELINE_REQUIREMENT
            && recipe.repair_requirement == REPAIR_REQUIREMENT,
        "repair Phase07/accepted-selected baseline prerequisite changed"
    );
    ensure!(
        recipe.command_id == commands::ADD_ENTITY,
        "repair command is not the closed ADD_ENTITY operation"
    );
    let WorkforceEntity::Availability(change) = &recipe.payload.entity else {
        bail!("repair recipe must contain exactly one availability record, never result authority")
    };
    ensure!(
        change.id == id(CaseId::RepairBefore, "availability-callout")?
            && change.person_id == id(CaseId::RepairBefore, "person-0")?
            && change.availability_kind == AvailabilityKind::Unavailable,
        "repair must change the concrete call-out person only"
    );
    ensure!(
        change.time_window
            == TimeWindow::Instant {
                starts_at: "2026-09-02T05:00:00Z".parse()?,
                ends_at: "2026-09-03T05:00:00Z".parse()?
            }
            && change.effective_range == date_range("2026-09-01", "2026-09-08")?,
        "call-out day changed"
    );
    ensure!(
        change.assignment_type_ids.is_none()
            && change.location_ids.is_none()
            && change.source == "synthetic-callout"
            && change.note == "Synthetic unexpected call-out",
        "call-out restrictions/source changed"
    );
    ensure!(
        !before
            .document
            .domain
            .entities
            .contains_key(&change.id.as_entity_id()),
        "call-out already present in before-input"
    );
    let mutation = commands::apply_batch(
        &before.document,
        &DomainBatchCommand {
            schema_version: DOMAIN_BATCH_SCHEMA_VERSION,
            pack_id: "official.workforce".parse()?,
            scenario_schema_version: 1,
            label: None,
            commands: vec![DomainCommandEnvelope {
                command_type: recipe.command_id.clone(),
                payload: serde_json::to_value(&recipe.payload)?,
            }],
        },
    )?;
    let after = validate_document(&mutation.document)?;
    ensure!(
        after.entities.len() == 11
            && after.rules.len() == 4
            && after.preferences.is_empty()
            && after.locked_assignments.is_empty(),
        "call-out introduced non-mutation authority or changed obligations"
    );
    ensure!(
        after.entities.get(&change.id.as_entity_id()) == Some(&recipe.payload.entity),
        "call-out mutation did not apply"
    );
    let mut restored = mutation.document;
    restored.domain.entities.remove(&change.id.as_entity_id());
    ensure!(
        restored == before.document,
        "recipe changed anything beyond the one unavailable-person record"
    );
    let shifts = temporal::resolve_shifts(&restored, &CancellationToken::new())?;
    ensure!(
        shifts
            .iter()
            .any(|shift| shift.reporting_date.to_string() == "2026-09-02"),
        "call-out has no concrete affected shift"
    );
    // This proves a valid mutation only. There is intentionally no BaseSchedule,
    // selected solution, solver invocation or accepted result created by this function.
    Ok(())
}
