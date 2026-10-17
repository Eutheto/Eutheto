//! Fixed synthetic source, not accepted-result authority. Role IDs are independent of order.
//! Configurable generation is capped at 256 people, 64 daily shifts and 16,384 pairs;
//! these are corpus allocation bounds, not changes to any production resource allowance.
mod semantics;
pub(crate) use semantics::{validate_repair, validate_semantics};

use crate::contract::{
    CORPUS_VERSION, CaseId, CorpusDefinitions, CorpusFamily, ExpectedCase, ExpectedDisposition,
    FIXTURE_CLOCK, FixtureDefinition, LargeParameters, ModelEnvelope, REPAIR_FORMAT,
    REPAIR_SCHEMA_VERSION, RepairFixture, ScoreExpectation, WorkloadClass,
};
use anyhow::{Context, Result, ensure};
use eutheto_types::{
    AssignmentId, CancellationToken, Clock, EntityId, FixedClock, PersonId, Revision,
    ScenarioDocument, ScenarioSnapshotV1, TypedUuidError,
};
use eutheto_workforce::{
    commands,
    model::{
        ActiveRange, AssignmentLock, AssignmentType, Availability, AvailabilityKind,
        CalendarPeriod, ConsecutiveMode, Coverage, DateRange, DeviationPenalty, LocalInterval,
        Location, LocationBehavior, LockState, MinimumRestRule, OverlappingContribution, PeerGroup,
        Person, PersonSelection, PreferenceDirection, PreferencePriority, PriorityMapping,
        Qualification, QualificationExpression, QualificationGrant, QualificationMatch,
        QualificationMinimum, Recurrence, ReportingAttribution, RequiredStrength, Scope,
        ScoreLevel, ShiftTemplate, ShiftTiming, TieBreakPolicy, TimeBehavior, TimeWindow, Weekday,
        WeeklyWindow, WindowMembership, WorkCalendar, WorkWindow, WorkforceDomainV1,
        WorkforceEntity, WorkforcePreference, WorkforceRule, WorkforceScorePolicy, WorkloadBucket,
        WorkloadMeasurement, WorkloadPolicy, WorkloadQuotaMeasure, WorkloadQuotaRule,
        WorkloadTargetMode, WorkloadWeight,
    },
    temporal,
};
use jiff::{Span, civil::Date};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    str::FromStr,
};
use uuid::Uuid;

pub(crate) const REPAIR_REQUIREMENT: &str = "Phase07: repair from that accepted and selected baseline after this single unavailable-person mutation; pure mutation validation is not repair execution";
pub(crate) const BASELINE_REQUIREMENT: &str = "Phase07: obtain a genuine independently accepted solution of practitioner-workflow and select it as the baseline; bind this call-out to the canonical first accepted pair on the exact 2027-02-10 weekday clinic that is not Hard-locked";
const WEEKDAYS: [Weekday; 5] = [
    Weekday::Monday,
    Weekday::Tuesday,
    Weekday::Wednesday,
    Weekday::Thursday,
    Weekday::Friday,
];
const EVERY_DAY: [Weekday; 7] = [
    Weekday::Monday,
    Weekday::Tuesday,
    Weekday::Wednesday,
    Weekday::Thursday,
    Weekday::Friday,
    Weekday::Saturday,
    Weekday::Sunday,
];

// The fixed timestamp prefix and framed role digest form UUIDv7 source identities.
// Occurrences are deliberately NOT derived here: production temporal reconciliation owns them.
fn id<T: FromStr<Err = TypedUuidError>>(case: CaseId, role: &str) -> Result<T> {
    let mut hash = Sha256::new();
    hash.update(b"eutheto/workforce-corpus/source-id/v1\0");
    for part in [case.slug().as_bytes(), role.as_bytes()] {
        hash.update((part.len() as u64).to_be_bytes());
        hash.update(part);
    }
    let digest = hash.finalize();
    let mut bytes = [0_u8; 16];
    let millis = FIXTURE_CLOCK.parse::<jiff::Timestamp>()?.as_millisecond();
    bytes[..6].copy_from_slice(&millis.to_be_bytes()[2..]);
    bytes[6..].copy_from_slice(&digest[..10]);
    bytes[6] = (bytes[6] & 0x0f) | 0x70;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Ok(Uuid::from_bytes(bytes).to_string().parse()?)
}

fn all() -> Scope {
    Scope {
        people: PersonSelection::All {},
        team_ids: None,
        assignment_type_ids: None,
        categories: None,
        weekdays: None,
        location_ids: None,
    }
}
fn type_scope(case: CaseId, role: &str) -> Result<Scope> {
    Ok(Scope {
        assignment_type_ids: Some(vec![id(case, role)?]),
        ..all()
    })
}
fn selected(case: CaseId, roles: &[String]) -> Result<PersonSelection> {
    Ok(PersonSelection::Selected {
        person_ids: roles
            .iter()
            .map(|role| id(case, role))
            .collect::<Result<_>>()?,
    })
}
fn insert(domain: &mut WorkforceDomainV1, entity: WorkforceEntity) {
    domain.entities.insert(entity.id(), entity);
}
fn rule(domain: &mut WorkforceDomainV1, value: WorkforceRule) {
    domain.rules.insert(value.header().0, value);
}
fn preference(domain: &mut WorkforceDomainV1, value: WorkforcePreference) {
    domain.preferences.insert(value.header().0, value);
}
fn basic_rules(case: CaseId, domain: &mut WorkforceDomainV1) -> Result<()> {
    rule(
        domain,
        WorkforceRule::Eligibility {
            id: id(case, "rule-eligibility")?,
            active: true,
            strength: RequiredStrength::Required,
            scope: all(),
        },
    );
    rule(
        domain,
        WorkforceRule::Availability {
            id: id(case, "rule-availability")?,
            active: true,
            strength: RequiredStrength::Required,
            scope: all(),
        },
    );
    rule(
        domain,
        WorkforceRule::Coverage {
            id: id(case, "rule-coverage")?,
            active: true,
            strength: RequiredStrength::Required,
            scope: all(),
        },
    );
    rule(
        domain,
        WorkforceRule::NoOverlap {
            id: id(case, "rule-no-overlap")?,
            active: true,
            strength: RequiredStrength::Required,
            scope: all(),
            compatible_category_pairs: vec![],
        },
    );
    Ok(())
}
fn bucket(
    case: CaseId,
    domain: &mut WorkforceDomainV1,
    role: &str,
    measurement: WorkloadMeasurement,
) -> Result<()> {
    insert(
        domain,
        WorkforceEntity::WorkloadBucket(WorkloadBucket {
            id: id(case, role)?,
            name: role.to_owned(),
            measurement,
            overlapping_contribution: OverlappingContribution::Sum,
        }),
    );
    Ok(())
}
fn qualification(
    case: CaseId,
    domain: &mut WorkforceDomainV1,
    role: &str,
    name: &str,
) -> Result<()> {
    insert(
        domain,
        WorkforceEntity::Qualification(Qualification {
            id: id(case, role)?,
            name: name.to_owned(),
            description: String::new(),
        }),
    );
    Ok(())
}
fn person(
    case: CaseId,
    domain: &mut WorkforceDomainV1,
    role: &str,
    name: &str,
    types: &[&str],
    grants: &[&str],
    weight: WorkloadWeight,
) -> Result<()> {
    insert(
        domain,
        WorkforceEntity::Person(Person {
            id: id(case, role)?,
            name: name.to_owned(),
            external_id: None,
            active_range: ActiveRange::Always {},
            qualification_grants: grants
                .iter()
                .map(|role| {
                    Ok(QualificationGrant {
                        qualification_id: id(case, role)?,
                        effective_from: None,
                        expires_at: None,
                    })
                })
                .collect::<Result<_>>()?,
            eligible_assignment_type_ids: types
                .iter()
                .map(|role| id(case, role))
                .collect::<Result<_>>()?,
            home_location_id: None,
            workload_weight: weight,
            workload_target: None,
            tags: vec![],
            team_ids: vec![],
            display: None,
        }),
    );
    Ok(())
}
fn dated_person(
    case: CaseId,
    role: &str,
    name: &str,
    types: &[&str],
    grants: &[&str],
    range: DateRange,
    weight: WorkloadWeight,
) -> Result<WorkforceEntity> {
    let starts_at = "2027-01-31T06:00:00Z".parse()?;
    let expires_at = "2027-03-02T06:00:00Z".parse()?;
    Ok(WorkforceEntity::Person(Person {
        id: id(case, role)?,
        name: name.to_owned(),
        external_id: None,
        active_range: ActiveRange::DateRange(range),
        qualification_grants: grants
            .iter()
            .map(|role| {
                Ok(QualificationGrant {
                    qualification_id: id(case, role)?,
                    effective_from: Some(starts_at),
                    expires_at: Some(expires_at),
                })
            })
            .collect::<Result<_>>()?,
        eligible_assignment_type_ids: types
            .iter()
            .map(|role| id(case, role))
            .collect::<Result<_>>()?,
        home_location_id: None,
        workload_weight: weight,
        workload_target: None,
        tags: vec![],
        team_ids: vec![],
        display: None,
    }))
}
fn assignment_type(
    case: CaseId,
    domain: &mut WorkforceDomainV1,
    role: &str,
    name: &str,
    category: &str,
    duration: u32,
    buckets: &[&str],
) -> Result<()> {
    insert(
        domain,
        WorkforceEntity::AssignmentType(AssignmentType {
            id: id(case, role)?,
            name: name.to_owned(),
            category: category.to_owned(),
            time_behavior: TimeBehavior::LocalWallClock,
            default_duration_minutes: duration,
            qualifications: QualificationExpression::Unconstrained {},
            location_behavior: LocationBehavior::Optional {},
            workload_bucket_ids: buckets
                .iter()
                .map(|role| id(case, role))
                .collect::<Result<_>>()?,
        }),
    );
    Ok(())
}
fn exact(
    case: CaseId,
    count: u16,
    qualification_role: Option<&str>,
    minimum: u16,
) -> Result<Coverage> {
    Ok(Coverage::Exact {
        count,
        qualification_minimums: qualification_role
            .map(|role| {
                Ok::<_, anyhow::Error>(QualificationMinimum {
                    qualifications: QualificationMatch {
                        all_qualification_ids: vec![id(case, role)?],
                        any_qualification_ids: vec![],
                    },
                    minimum,
                })
            })
            .transpose()?
            .into_iter()
            .collect(),
    })
}
fn date_range(start: &str, end: &str) -> Result<DateRange> {
    Ok(DateRange {
        start_date: start.parse()?,
        end_date_exclusive: end.parse()?,
    })
}
struct TemplateSpec<'a> {
    role: &'a str,
    name: &'a str,
    type_role: &'a str,
    range: DateRange,
    weekdays: &'a [Weekday],
    timing: ShiftTiming,
    coverage: Coverage,
    location: bool,
}

fn template(case: CaseId, domain: &mut WorkforceDomainV1, spec: TemplateSpec<'_>) -> Result<()> {
    let TemplateSpec {
        role,
        name,
        type_role,
        range,
        weekdays,
        timing,
        coverage,
        location,
    } = spec;
    insert(
        domain,
        WorkforceEntity::ShiftTemplate(ShiftTemplate {
            id: id(case, role)?,
            name: name.to_owned(),
            assignment_type_id: id(case, type_role)?,
            location_id: if location {
                Some(id(case, "loc-main")?)
            } else {
                None
            },
            recurrence: Recurrence {
                weekdays: weekdays.to_vec(),
                effective_range: range,
                excluded_dates: vec![],
            },
            timing,
            coverage,
            tags: vec![],
            reporting_attribution: ReportingAttribution::StartLocalDate,
            occurrence_identities: BTreeMap::new(),
        }),
    );
    Ok(())
}
fn wall(start: &str, end: &str, end_day_offset: u8) -> Result<ShiftTiming> {
    Ok(ShiftTiming::LocalWindow {
        start_time: start.parse()?,
        end_time: end.parse()?,
        end_day_offset,
    })
}
fn unavailable(
    case: CaseId,
    role: &str,
    person_role: &str,
    instants: (&str, &str),
    range: DateRange,
    source: &str,
    note: &str,
) -> Result<Availability> {
    Ok(Availability {
        id: id(case, role)?,
        person_id: id(case, person_role)?,
        availability_kind: AvailabilityKind::Unavailable,
        time_window: TimeWindow::Instant {
            starts_at: instants.0.parse()?,
            ends_at: instants.1.parse()?,
        },
        effective_range: range,
        assignment_type_ids: None,
        location_ids: None,
        source: source.to_owned(),
        note: note.to_owned(),
    })
}
fn approved_leave(
    case: CaseId,
    role: &str,
    person_role: &str,
    instants: (&str, &str),
    range: DateRange,
    source: &str,
    note: &str,
) -> Result<Availability> {
    let mut value = unavailable(case, role, person_role, instants, range, source, note)?;
    value.availability_kind = AvailabilityKind::ApprovedTimeOff;
    Ok(value)
}
fn rest(case: CaseId, domain: &mut WorkforceDomainV1, people: PersonSelection) -> Result<()> {
    rule(
        domain,
        WorkforceRule::MinimumRest(Box::new(MinimumRestRule {
            id: id(case, "rule-rest-call-to-clinic")?,
            active: true,
            strength: RequiredStrength::Required,
            scope: Scope { people, ..all() },
            after_scope: type_scope(case, "type-call-overnight")?,
            before_scope: type_scope(case, "type-clinic-am")?,
            minimum_minutes: 600,
        })),
    );
    Ok(())
}
fn directional_rest(
    case: CaseId,
    domain: &mut WorkforceDomainV1,
    role: &str,
    people: PersonSelection,
    after_type: &str,
    before_type: &str,
    minimum_minutes: u32,
) -> Result<()> {
    rule(
        domain,
        WorkforceRule::MinimumRest(Box::new(MinimumRestRule {
            id: id(case, role)?,
            active: true,
            strength: RequiredStrength::Required,
            scope: Scope { people, ..all() },
            after_scope: type_scope(case, after_type)?,
            before_scope: type_scope(case, before_type)?,
            minimum_minutes,
        })),
    );
    Ok(())
}
fn score_policy(
    case: CaseId,
    domain: &mut WorkforceDomainV1,
    balance: Option<(&str, PersonSelection, DateRange)>,
) -> Result<()> {
    let levels = [
        ("stability", "Stability"),
        ("high-preferences", "High preferences"),
        ("fairness", "Fairness"),
        ("normal-preferences", "Normal preferences"),
    ];
    let mut policies = BTreeMap::new();
    if let Some((bucket_role, people, range)) = balance {
        insert(
            domain,
            WorkforceEntity::Calendar(WorkCalendar {
                id: id(case, "calendar-plan")?,
                name: "Whole plan".to_owned(),
                period: CalendarPeriod::Custom {
                    intervals: vec![LocalInterval {
                        starts_at: format!("{}T00:00:00", range.start_date).parse()?,
                        ends_at: format!("{}T00:00:00", range.end_date_exclusive).parse()?,
                    }],
                },
            }),
        );
        let policy_id = id(case, "policy-balance")?;
        policies.insert(
            policy_id,
            WorkloadPolicy {
                id: policy_id,
                bucket_id: id(case, bucket_role)?,
                calendar_id: id(case, "calendar-plan")?,
                membership: WindowMembership::ReportingDate,
                peer_group: PeerGroup {
                    people,
                    team_ids: None,
                },
                target_mode: WorkloadTargetMode::WeightedEqualShare {},
                penalty: DeviationPenalty::Absolute {},
            },
        );
        preference(
            domain,
            WorkforcePreference::WorkloadBalance {
                id: id(case, "pref-balance")?,
                active: true,
                scope: all(),
                priority: PreferencePriority::High,
                weight: 1,
                workload_policy_id: policy_id,
            },
        );
    }
    insert(
        domain,
        WorkforceEntity::ScorePolicy(WorkforceScorePolicy {
            id: id(case, "score-policy")?,
            profile_key: "balanced".to_owned(),
            levels: levels
                .into_iter()
                .map(|(key, label)| ScoreLevel {
                    level_key: key.to_owned(),
                    label: label.to_owned(),
                })
                .collect(),
            priority_mapping: [
                (PreferencePriority::VeryHigh, "high-preferences", 1),
                (PreferencePriority::High, "fairness", 1),
                (PreferencePriority::Normal, "normal-preferences", 2),
                (PreferencePriority::Low, "normal-preferences", 1),
            ]
            .into_iter()
            .map(|(priority, level, scale)| PriorityMapping {
                priority,
                level_key: level.to_owned(),
                scale,
            })
            .collect(),
            workload_policies: policies,
            tie_break: TieBreakPolicy::StableAssignmentRank,
        }),
    );
    Ok(())
}
fn friday_preference(
    case: CaseId,
    domain: &mut WorkforceDomainV1,
    person_role: &str,
) -> Result<()> {
    preference(
        domain,
        WorkforcePreference::Time {
            id: id(case, "pref-no-friday")?,
            active: true,
            scope: Scope {
                people: selected(case, &[person_role.to_owned()])?,
                ..all()
            },
            priority: PreferencePriority::Normal,
            weight: 1,
            direction: PreferenceDirection::Avoid,
            time_window: TimeWindow::Weekly {
                windows: vec![WeeklyWindow {
                    weekdays: vec![Weekday::Friday],
                    start_time: "00:00:00".parse()?,
                    end_time: "00:00:00".parse()?,
                    end_day_offset: 1,
                }],
            },
        },
    );
    Ok(())
}
fn snapshot(
    case: CaseId,
    domain: &WorkforceDomainV1,
    range: DateRange,
    title: &str,
    description: &str,
    gap: &str,
    overlap: &str,
) -> Result<ScenarioSnapshotV1> {
    let clock = FixedClock::new(FIXTURE_CLOCK.parse()?);
    let zone = jiff::tz::TimeZone::get("America/Chicago")?;
    let midnight = |date: Date| -> Result<_> {
        Ok(date
            .at(0, 0, 0, 0)
            .to_zoned(zone.clone())?
            .timestamp()
            .to_string())
    };
    let document: ScenarioDocument = serde_json::from_value(json!({
        "format":"eutheto/scenario", "formatVersion":1, "scenarioId":id::<EntityId>(case, "scenario")?,
        "domainPack":{"id":"official.workforce","schemaVersion":1},
        "metadata":{"title":title,"description":description,"createdAt":clock.now(),"updatedAt":clock.now()},
        "settings":{"timeZone":"America/Chicago","locale":"en-US","units":"us-customary","horizon":{"start":midnight(range.start_date)?,"end":midnight(range.end_date_exclusive)?},"gapPolicy":gap,"overlapPolicy":overlap},
        "domain":domain,"extensions":{}
    }))?;
    let preview = temporal::preview_generation(&document, &document, &CancellationToken::new())?;
    let document = if let Some(batch) = preview.reconciliation {
        commands::apply_batch(&document, &batch)?.document
    } else {
        document
    };
    Ok(ScenarioSnapshotV1::current(
        Revision::INITIAL,
        document,
        BTreeSet::new(),
    ))
}

fn appendix() -> Result<ScenarioSnapshotV1> {
    let case = CaseId::AppendixF;
    let mut domain = appendix_population()?;
    let range = date_range("2026-09-01", "2026-10-01")?;
    let roles: Vec<_> = ["person-smith", "person-jones", "person-patel"]
        .into_iter()
        .map(str::to_owned)
        .collect();
    template(
        case,
        &mut domain,
        TemplateSpec {
            role: "template-weekday-clinic",
            name: "Weekday morning clinic",
            type_role: "type-clinic-am",
            range,
            weekdays: &WEEKDAYS,
            timing: wall("08:00:00", "12:00:00", 0)?,
            coverage: exact(case, 2, Some("q-physician"), 2)?,
            location: true,
        },
    )?;
    template(
        case,
        &mut domain,
        TemplateSpec {
            role: "template-nightly-call",
            name: "Nightly overnight call",
            type_role: "type-call-overnight",
            range,
            weekdays: &EVERY_DAY,
            timing: wall("20:00:00", "06:00:00", 1)?,
            coverage: exact(case, 1, Some("q-call"), 1)?,
            location: false,
        },
    )?;
    insert(
        &mut domain,
        WorkforceEntity::Availability(unavailable(
            case,
            "availability-smith-thursday",
            "person-smith",
            ("2026-09-10T05:00:00Z", "2026-09-11T05:00:00Z"),
            range,
            "synthetic-appendix-f",
            "Requested day off",
        )?),
    );
    basic_rules(case, &mut domain)?;
    rest(case, &mut domain, selected(case, &roles)?)?;
    rule(
        &mut domain,
        WorkforceRule::MaximumHours {
            id: id(case, "rule-max-hours-rolling")?,
            active: true,
            strength: RequiredStrength::Required,
            scope: all(),
            bucket_id: id(case, "all-hours")?,
            window: WorkWindow::Rolling {
                duration_minutes: 20160,
                membership: WindowMembership::Intersection,
            },
            maximum_minutes: 4800,
        },
    );
    rule(
        &mut domain,
        WorkforceRule::MaximumConsecutive {
            id: id(case, "rule-max-consecutive-nights")?,
            active: true,
            strength: RequiredStrength::Required,
            scope: type_scope(case, "type-call-overnight")?,
            mode: ConsecutiveMode::Assignments { break_minutes: 840 },
            maximum: 2,
        },
    );
    score_policy(
        case,
        &mut domain,
        Some(("overnight-count", selected(case, &roles)?, range)),
    )?;
    friday_preference(case, &mut domain, "person-jones")?;
    snapshot(
        case,
        &domain,
        date_range("2026-09-01", "2026-10-01")?,
        "September clinic and overnight call",
        "Example medical-practice schedule",
        "reject",
        "reject",
    )
}

fn appendix_population() -> Result<WorkforceDomainV1> {
    let case = CaseId::AppendixF;
    let mut domain = WorkforceDomainV1::default();
    qualification(case, &mut domain, "q-physician", "Physician")?;
    qualification(case, &mut domain, "q-call", "Overnight call eligible")?;
    insert(
        &mut domain,
        WorkforceEntity::Location(Location {
            id: id(case, "loc-main")?,
            name: "Main clinic".to_owned(),
            transitions: vec![],
        }),
    );
    for role in ["all-hours", "clinic-hours", "call-hours"] {
        bucket(case, &mut domain, role, WorkloadMeasurement::ElapsedMinutes)?;
    }
    bucket(
        case,
        &mut domain,
        "overnight-count",
        WorkloadMeasurement::AssignmentCount,
    )?;
    assignment_type(
        case,
        &mut domain,
        "type-clinic-am",
        "Morning clinic",
        "clinic",
        240,
        &["all-hours", "clinic-hours"],
    )?;
    assignment_type(
        case,
        &mut domain,
        "type-call-overnight",
        "Overnight call",
        "call",
        600,
        &["all-hours", "call-hours", "overnight-count"],
    )?;
    for (role, name, numerator, denominator) in [
        ("person-smith", "Dr. Smith", 1, 1),
        ("person-jones", "Dr. Jones", 1, 1),
        ("person-patel", "Dr. Patel", 4, 5),
    ] {
        person(
            case,
            &mut domain,
            role,
            name,
            &["type-clinic-am", "type-call-overnight"],
            &["q-physician", "q-call"],
            WorkloadWeight {
                numerator,
                denominator,
            },
        )?;
    }
    Ok(domain)
}

fn clinic(case: CaseId) -> Result<ScenarioSnapshotV1> {
    let (people, end, overnight, full) = match case {
        CaseId::ClinicTiny => (3, "2026-09-08", false, false),
        CaseId::ClinicInitial | CaseId::RollingHours => (8, "2026-09-29", false, false),
        CaseId::ClinicFull => (8, "2026-09-29", false, true),
        CaseId::ClinicOvernight => (12, "2026-09-29", true, false),
        CaseId::SpecialistCoverage => (6, "2026-09-08", false, false),
        CaseId::RepairBefore => (4, "2026-09-08", false, false),
        CaseId::InfeasibleCoverage => (3, "2026-09-02", false, false),
        _ => anyhow::bail!("not a clinic case"),
    };
    let mut domain = clinic_population(case, people, overnight)?;
    let range = date_range("2026-09-01", end)?;
    clinic_templates(case, &mut domain, range, overnight)?;
    if case != CaseId::InfeasibleCoverage && case != CaseId::RepairBefore {
        insert(
            &mut domain,
            WorkforceEntity::Availability(unavailable(
                case,
                "availability-day-off",
                "person-0",
                ("2026-09-03T05:00:00Z", "2026-09-04T05:00:00Z"),
                range,
                "synthetic-corpus",
                "Synthetic requested day off",
            )?),
        );
    }
    clinic_rules(case, &mut domain, range, overnight, full)?;
    let description = if case == CaseId::ClinicInitial {
        "Phase05 initial slice: full-fairness clinic is separately preserved as clinic-full; workload balance and Friday preference are intentionally absent here"
    } else if full {
        "Full four-week clinic: active weighted equal-share fairness and Friday avoidance require Phase07"
    } else {
        "Fixed synthetic Workforce corpus input; no baseline or accepted result authority"
    };
    let mut result = snapshot(
        case,
        &domain,
        date_range("2026-09-01", end)?,
        case.slug(),
        description,
        "reject",
        "reject",
    )?;
    if case == CaseId::RollingHours {
        let shifts = temporal::resolve_shifts(&result.document, &CancellationToken::new())?;
        let first = shifts
            .iter()
            .find(|shift| shift.reporting_date.to_string() == "2026-09-01")
            .ok_or_else(|| anyhow::anyhow!("missing first clinic shift"))?;
        let lock = AssignmentLock {
            id: id(case, "lock-first-clinic")?,
            person_id: id(case, "person-0")?,
            shift_id: first.id,
            state: LockState::Hard {},
        };
        result
            .document
            .domain
            .locked_assignments
            .insert(lock.id, serde_json::to_value(lock)?);
    }
    Ok(result)
}

fn clinic_population(case: CaseId, people: usize, overnight: bool) -> Result<WorkforceDomainV1> {
    let mut domain = WorkforceDomainV1::default();
    qualification(case, &mut domain, "q-physician", "Physician")?;
    if overnight {
        qualification(case, &mut domain, "q-call", "Overnight call eligible")?;
    }
    if case == CaseId::SpecialistCoverage {
        qualification(case, &mut domain, "q-specialist", "Synthetic specialist")?;
    }
    bucket(
        case,
        &mut domain,
        "all-hours",
        WorkloadMeasurement::ElapsedMinutes,
    )?;
    bucket(
        case,
        &mut domain,
        "clinic-count",
        WorkloadMeasurement::AssignmentCount,
    )?;
    assignment_type(
        case,
        &mut domain,
        "type-clinic-am",
        "Morning clinic",
        "clinic",
        240,
        &["all-hours", "clinic-count"],
    )?;
    if overnight {
        assignment_type(
            case,
            &mut domain,
            "type-call-overnight",
            "Overnight call",
            "call",
            600,
            &["all-hours"],
        )?;
    }
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
        // Exactly one eligible person remains for a two-person requirement. Other people
        // remain active and qualified: insufficient eligible coverage, not malformed data.
        if case == CaseId::InfeasibleCoverage && index > 0 {
            types.clear();
        }
        person(
            case,
            &mut domain,
            &format!("person-{index}"),
            &format!("Synthetic clinician {}", index + 1),
            &types,
            &grants,
            WorkloadWeight {
                numerator: 1,
                denominator: 1,
            },
        )?;
    }
    Ok(domain)
}

fn clinic_templates(
    case: CaseId,
    domain: &mut WorkforceDomainV1,
    range: DateRange,
    overnight: bool,
) -> Result<()> {
    let mut coverage = exact(case, 2, Some("q-physician"), 2)?;
    if case == CaseId::SpecialistCoverage
        && let Coverage::Exact {
            qualification_minimums,
            ..
        } = &mut coverage
    {
        qualification_minimums.push(QualificationMinimum {
            qualifications: QualificationMatch {
                all_qualification_ids: vec![id(case, "q-specialist")?],
                any_qualification_ids: vec![],
            },
            minimum: 1,
        });
    }
    template(
        case,
        domain,
        TemplateSpec {
            role: "template-weekday-clinic",
            name: "Weekday morning clinic",
            type_role: "type-clinic-am",
            range,
            weekdays: &WEEKDAYS,
            timing: wall("08:00:00", "12:00:00", 0)?,
            coverage,
            location: false,
        },
    )?;
    if overnight {
        template(
            case,
            domain,
            TemplateSpec {
                role: "template-nightly-call",
                name: "Nightly overnight call",
                type_role: "type-call-overnight",
                range,
                weekdays: &EVERY_DAY,
                timing: wall("20:00:00", "06:00:00", 1)?,
                coverage: exact(case, 1, Some("q-call"), 1)?,
                location: false,
            },
        )?;
    }
    Ok(())
}

fn clinic_rules(
    case: CaseId,
    domain: &mut WorkforceDomainV1,
    range: DateRange,
    overnight: bool,
    full: bool,
) -> Result<()> {
    basic_rules(case, domain)?;
    if overnight {
        rest(case, domain, PersonSelection::All {})?;
    }
    if full {
        score_policy(
            case,
            domain,
            Some(("clinic-count", PersonSelection::All {}, range)),
        )?;
        friday_preference(case, domain, "person-1")?;
    } else {
        score_policy(case, domain, None)?;
    }
    if case == CaseId::RollingHours {
        for (role, duration_minutes, maximum_minutes) in [
            ("rule-hours-seven-days", 10080, 1200),
            ("rule-hours-fourteen-days", 20160, 2400),
        ] {
            rule(
                domain,
                WorkforceRule::MaximumHours {
                    id: id(case, role)?,
                    active: true,
                    strength: RequiredStrength::Required,
                    scope: all(),
                    bucket_id: id(case, "all-hours")?,
                    window: WorkWindow::Rolling {
                        duration_minutes,
                        membership: WindowMembership::Intersection,
                    },
                    maximum_minutes,
                },
            );
        }
    }
    Ok(())
}
fn practitioner_population(
    case: CaseId,
    activity_range: DateRange,
    infeasible: bool,
) -> Result<WorkforceDomainV1> {
    let mut domain = WorkforceDomainV1::default();
    qualification(case, &mut domain, "q-physician", "Physician")?;
    qualification(case, &mut domain, "q-call", "Overnight call eligible")?;
    qualification(case, &mut domain, "q-specialist", "Synthetic specialist")?;
    for (role, measurement) in [
        ("all-hours", WorkloadMeasurement::ElapsedMinutes),
        ("combined-count", WorkloadMeasurement::AssignmentCount),
        ("clinic-hours", WorkloadMeasurement::ElapsedMinutes),
        ("call-hours", WorkloadMeasurement::ElapsedMinutes),
    ] {
        bucket(case, &mut domain, role, measurement)?;
    }
    assignment_type(
        case,
        &mut domain,
        "type-clinic-am",
        "Morning clinic",
        "clinic",
        240,
        &["all-hours", "clinic-hours", "combined-count"],
    )?;
    assignment_type(
        case,
        &mut domain,
        "type-call-overnight",
        "Overnight call",
        "call",
        600,
        &["all-hours", "call-hours", "combined-count"],
    )?;
    let names = [
        "Aster Vale",
        "Brin Rowan",
        "Cleo Maren",
        "Dax Solis",
        "Elian Moss",
        "Faye Quill",
        "Gio Lark",
        "Hana Voss",
        "Ira Nolin",
        "Juno Pike",
        "Kian Wren",
        "Lumi Sable",
    ];
    for (index, name) in names.into_iter().enumerate() {
        let mut grants = vec!["q-physician", "q-call"];
        let specialist = if infeasible { index == 0 } else { index < 4 };
        if specialist {
            grants.push("q-specialist");
        }
        let weight = match index {
            0..=3 => WorkloadWeight {
                numerator: 1,
                denominator: 1,
            },
            4..=7 => WorkloadWeight {
                numerator: 3,
                denominator: 4,
            },
            _ => WorkloadWeight {
                numerator: 1,
                denominator: 2,
            },
        };
        insert(
            &mut domain,
            dated_person(
                case,
                &format!("person-{index:02}"),
                name,
                &["type-clinic-am", "type-call-overnight"],
                &grants,
                activity_range,
                weight,
            )?,
        );
    }
    Ok(domain)
}

fn practitioner_templates(
    case: CaseId,
    domain: &mut WorkforceDomainV1,
    range: DateRange,
    infeasible: bool,
) -> Result<()> {
    let mut clinic_coverage = exact(case, 2, Some("q-physician"), 2)?;
    if let Coverage::Exact {
        qualification_minimums,
        ..
    } = &mut clinic_coverage
    {
        qualification_minimums.push(QualificationMinimum {
            qualifications: QualificationMatch {
                all_qualification_ids: vec![id(case, "q-specialist")?],
                any_qualification_ids: vec![],
            },
            minimum: if infeasible { 2 } else { 1 },
        });
    }
    template(
        case,
        domain,
        TemplateSpec {
            role: "template-weekday-clinic",
            name: "Weekday morning clinic",
            type_role: "type-clinic-am",
            range,
            weekdays: &WEEKDAYS,
            timing: wall("08:00:00", "12:00:00", 0)?,
            coverage: clinic_coverage,
            location: false,
        },
    )?;
    template(
        case,
        domain,
        TemplateSpec {
            role: "template-nightly-call",
            name: "Nightly overnight call",
            type_role: "type-call-overnight",
            range,
            weekdays: &EVERY_DAY,
            timing: wall("20:00:00", "06:00:00", 1)?,
            coverage: exact(case, 1, Some("q-call"), 1)?,
            location: false,
        },
    )?;
    Ok(())
}

fn practitioner_rules(
    case: CaseId,
    domain: &mut WorkforceDomainV1,
    range: DateRange,
) -> Result<()> {
    basic_rules(case, domain)?;
    directional_rest(
        case,
        domain,
        "rule-rest-call-to-clinic",
        PersonSelection::All {},
        "type-call-overnight",
        "type-clinic-am",
        600,
    )?;
    directional_rest(
        case,
        domain,
        "rule-rest-clinic-to-call",
        PersonSelection::All {},
        "type-clinic-am",
        "type-call-overnight",
        600,
    )?;
    insert(
        domain,
        WorkforceEntity::Calendar(WorkCalendar {
            id: id(case, "calendar-week")?,
            name: "Authored weekly period".to_owned(),
            period: CalendarPeriod::Custom {
                intervals: vec![
                    LocalInterval {
                        starts_at: "2027-02-01T00:00:00".parse()?,
                        ends_at: "2027-02-08T00:00:00".parse()?,
                    },
                    LocalInterval {
                        starts_at: "2027-02-08T00:00:00".parse()?,
                        ends_at: "2027-02-15T00:00:00".parse()?,
                    },
                    LocalInterval {
                        starts_at: "2027-02-15T00:00:00".parse()?,
                        ends_at: "2027-02-22T00:00:00".parse()?,
                    },
                    LocalInterval {
                        starts_at: "2027-02-22T00:00:00".parse()?,
                        ends_at: "2027-03-01T00:00:00".parse()?,
                    },
                ],
            },
        }),
    );
    score_policy(
        case,
        domain,
        Some(("combined-count", PersonSelection::All {}, range)),
    )?;
    preference(
        domain,
        WorkforcePreference::WeekendTarget {
            id: id(case, "pref-weekend-target")?,
            active: true,
            scope: all(),
            priority: PreferencePriority::Normal,
            weight: 1,
            target_count: 1,
        },
    );
    practitioner_workload_rules(case, domain)?;
    practitioner_maximum_rules(case, domain)?;
    Ok(())
}

fn practitioner_workload_rules(case: CaseId, domain: &mut WorkforceDomainV1) -> Result<()> {
    rule(
        domain,
        WorkforceRule::WorkloadQuota(Box::new(WorkloadQuotaRule {
            id: id(case, "rule-quota-count")?,
            active: true,
            strength: RequiredStrength::Required,
            scope: all(),
            calendar_id: id(case, "calendar-week")?,
            membership: WindowMembership::ReportingDate,
            minimum: 1,
            maximum: Some(3),
            measure: WorkloadQuotaMeasure::AssignmentCount {},
        })),
    );
    rule(
        domain,
        WorkforceRule::WorkloadQuota(Box::new(WorkloadQuotaRule {
            id: id(case, "rule-quota-hours")?,
            active: true,
            strength: RequiredStrength::Required,
            scope: all(),
            calendar_id: id(case, "calendar-week")?,
            membership: WindowMembership::Intersection,
            minimum: 240,
            maximum: Some(2400),
            measure: WorkloadQuotaMeasure::BucketMinutes {
                bucket_id: id(case, "all-hours")?,
            },
        })),
    );
    rule(
        domain,
        WorkforceRule::MaximumHours {
            id: id(case, "rule-max-hours-fourteen-days")?,
            active: true,
            strength: RequiredStrength::Required,
            scope: all(),
            bucket_id: id(case, "all-hours")?,
            window: WorkWindow::Rolling {
                duration_minutes: 20160,
                membership: WindowMembership::Intersection,
            },
            maximum_minutes: 6000,
        },
    );
    rule(
        domain,
        WorkforceRule::MaximumAssignmentCount {
            id: id(case, "rule-max-count-week")?,
            active: true,
            strength: RequiredStrength::Required,
            scope: all(),
            calendar_id: id(case, "calendar-week")?,
            maximum: 3,
        },
    );
    Ok(())
}

fn practitioner_maximum_rules(case: CaseId, domain: &mut WorkforceDomainV1) -> Result<()> {
    rule(
        domain,
        WorkforceRule::MaximumConsecutive {
            id: id(case, "rule-max-consecutive-call")?,
            active: true,
            strength: RequiredStrength::Required,
            scope: type_scope(case, "type-call-overnight")?,
            mode: ConsecutiveMode::Assignments { break_minutes: 900 },
            maximum: 2,
        },
    );
    rule(
        domain,
        WorkforceRule::MaximumWorkedWeekends {
            id: id(case, "rule-max-worked-weekends")?,
            active: true,
            strength: RequiredStrength::Required,
            scope: all(),
            maximum: 2,
        },
    );
    Ok(())
}

fn practitioner(case: CaseId, infeasible: bool) -> Result<ScenarioSnapshotV1> {
    let range = date_range("2027-02-01", "2027-03-01")?;
    // Finite person activity contains the final overnight tail without extending generation.
    let activity_range = date_range("2027-02-01", "2027-03-02")?;
    let mut domain = practitioner_population(case, activity_range, infeasible)?;
    practitioner_templates(case, &mut domain, range, infeasible)?;
    insert(
        &mut domain,
        WorkforceEntity::Availability(approved_leave(
            case,
            "availability-approved-leave",
            "person-07",
            ("2027-02-17T13:00:00Z", "2027-02-18T01:00:00Z"),
            range,
            "synthetic-practitioner",
            "Approved study leave",
        )?),
    );
    practitioner_rules(case, &mut domain, range)?;
    let mut result = snapshot(
        case,
        &domain,
        range,
        if infeasible {
            "Synthetic practitioner workflow: specialist infeasible variation"
        } else {
            "Synthetic practitioner workflow: combined clinic and overnight call"
        },
        "Deliberately fictional twelve-clinician four-week source with authored coverage, dated eligibility, leave, quotas, recovery, burden metrics, weekend cap and preference target; no accepted result authority",
        "reject",
        "reject",
    )?;
    let shifts = temporal::resolve_shifts(&result.document, &CancellationToken::new())?;
    let clinic_id = id::<eutheto_workforce::ids::ShiftTemplateId>(case, "template-weekday-clinic")?;
    let anchor = shifts
        .iter()
        .find(|shift| {
            shift.reporting_date.to_string() == "2027-02-01"
                && matches!(
                    shift.origin,
                    temporal::ResolvedShiftOrigin::Generated { template_id, .. }
                        if template_id == clinic_id
                )
        })
        .context("missing practitioner hard-lock clinic")?;
    let lock = AssignmentLock {
        id: id::<AssignmentId>(case, "lock-clinic-anchor")?,
        person_id: id(case, "person-00")?,
        shift_id: anchor.id,
        state: LockState::Hard {},
    };
    result
        .document
        .domain
        .locked_assignments
        .insert(lock.id, serde_json::to_value(lock)?);
    Ok(result)
}

fn dst(case: CaseId) -> Result<ScenarioSnapshotV1> {
    let spring = case == CaseId::DstSpring;
    let (start, end, begins, ends, gap, overlap) = if spring {
        (
            "2026-03-08",
            "2026-03-09",
            "02:30:00",
            "04:30:00",
            "moveForward",
            "reject",
        )
    } else {
        (
            "2026-11-01",
            "2026-11-02",
            "01:30:00",
            "02:30:00",
            "reject",
            "earlier",
        )
    };
    let mut domain = WorkforceDomainV1::default();
    assignment_type(
        case,
        &mut domain,
        "type-transition",
        "Transition duty",
        "transition",
        if spring { 120 } else { 60 },
        &[],
    )?;
    for index in 0..2 {
        person(
            case,
            &mut domain,
            &format!("person-{index}"),
            &format!("Transition clinician {}", index + 1),
            &["type-transition"],
            &[],
            WorkloadWeight {
                numerator: 1,
                denominator: 1,
            },
        )?;
    }
    template(
        case,
        &mut domain,
        TemplateSpec {
            role: "template-transition",
            name: "Sunday transition duty",
            type_role: "type-transition",
            range: date_range(start, end)?,
            weekdays: &[Weekday::Sunday],
            timing: wall(begins, ends, 0)?,
            coverage: exact(case, 1, None, 0)?,
            location: false,
        },
    )?;
    basic_rules(case, &mut domain)?;
    score_policy(case, &mut domain, None)?;
    snapshot(
        case,
        &domain,
        date_range(start, end)?,
        case.slug(),
        "Bounded transition-day solve; changing the selected explicit gap/fold policy to Reject must reject the same local template",
        gap,
        overlap,
    )
}

fn large(case: CaseId, parameters: LargeParameters) -> Result<ScenarioSnapshotV1> {
    ensure!(
        (1..=256).contains(&parameters.people),
        "synthetic people must be in 1..=256"
    );
    ensure!(
        (1..=64).contains(&parameters.shifts),
        "synthetic daily shifts must be in 1..=64"
    );
    ensure!(
        u32::from(parameters.people) * u32::from(parameters.shifts) <= 16384,
        "synthetic pair bound exceeded"
    );
    let mut domain = WorkforceDomainV1::default();
    assignment_type(
        case,
        &mut domain,
        "type-synthetic",
        "Unpruned synthetic duty",
        "synthetic",
        60,
        &[],
    )?;
    for index in 0..parameters.people {
        person(
            case,
            &mut domain,
            &format!("person-{index}"),
            &format!("Synthetic person {}", index + 1),
            &["type-synthetic"],
            &[],
            WorkloadWeight {
                numerator: 1,
                denominator: 1,
            },
        )?;
    }
    let end = "2026-09-01"
        .parse::<Date>()?
        .checked_add(Span::new().days(i64::from(parameters.shifts)))?
        .to_string();
    template(
        case,
        &mut domain,
        TemplateSpec {
            role: "template-synthetic",
            name: "Daily unpruned duty",
            type_role: "type-synthetic",
            range: date_range("2026-09-01", &end)?,
            weekdays: &EVERY_DAY,
            timing: wall("08:00:00", "09:00:00", 0)?,
            coverage: exact(case, 1, None, 0)?,
            location: false,
        },
    )?;
    basic_rules(case, &mut domain)?;
    score_policy(case, &mut domain, None)?;
    snapshot(
        case,
        &domain,
        date_range("2026-09-01", &end)?,
        case.slug(),
        "Unpruned Cartesian population; no availability, type, qualification or later-rule shortcut; seed 0; generator bounds 256 people, 64 shifts and 16384 pairs",
        "reject",
        "reject",
    )
}
pub(crate) fn build_large(parameters: LargeParameters) -> Result<ScenarioSnapshotV1> {
    large(CaseId::LargeSupported, parameters)
}

fn expected(case: CaseId) -> ExpectedCase {
    use ExpectedDisposition::{Accepted, Infeasible, ResourceLimit};
    let (people, horizon_days, resolved_shifts, selected_assignments, disposition) = match case {
        CaseId::AppendixF => (3, 30, 52, None, Infeasible),
        CaseId::ClinicTiny => (3, 7, 5, Some(10), Accepted),
        CaseId::ClinicInitial | CaseId::RollingHours | CaseId::ClinicFull => {
            (8, 28, 20, Some(40), Accepted)
        }
        CaseId::ClinicOvernight | CaseId::PractitionerWorkflow => (12, 28, 48, Some(68), Accepted),
        CaseId::SpecialistCoverage => (6, 7, 5, Some(10), Accepted),
        CaseId::RepairBefore => (4, 7, 5, Some(10), Accepted),
        CaseId::InfeasibleCoverage => (3, 1, 1, None, Infeasible),
        CaseId::PractitionerInfeasible => (12, 28, 48, None, Infeasible),
        CaseId::DstSpring | CaseId::DstFall => (2, 1, 1, Some(1), Accepted),
        CaseId::LargeSupported => (101, 12, 12, Some(12), Accepted),
        CaseId::LargePressure => (100, 64, 64, None, ResourceLimit),
    };
    // Broad source-derived integer rank bounds do not assert a particular equal-score schedule.
    // Characterize exact committed IR shapes centrally before tightening these envelopes.
    let score = selected_assignments.map(|selected| ScoreExpectation {
        feasibility: 0,
        minimum_stable_rank: 0,
        maximum_stable_rank: i64::from(people * resolved_shifts * selected),
    });
    let model_envelope = if matches!(disposition, Accepted | Infeasible) {
        Some(ModelEnvelope {
            minimum_variables: if case == CaseId::LargeSupported {
                2400
            } else {
                1
            },
            maximum_variables: if matches!(
                case,
                CaseId::PractitionerWorkflow | CaseId::PractitionerInfeasible
            ) {
                12000
            } else if case == CaseId::LargeSupported {
                2600
            } else {
                3000
            },
            maximum_constraints: 12000,
        })
    } else {
        None
    };
    let detail = semantic_description(case);
    let semantics = if matches!(
        case,
        CaseId::PractitionerWorkflow | CaseId::PractitionerInfeasible
    ) {
        vec![
            detail.to_owned(),
            "Reviewed source totals: 20 weekday clinic occurrences at exact two physicians with one specialist minimum in the feasible variant (two in the infeasible variation), plus 28 daily overnight calls at exact one call-qualified clinician; the feasible source therefore has 40 clinic and 28 call assignments, while the variation has no accepted assignment authority.".to_owned(),
            "Reviewed Required boundaries: dated active eligibility is 2027-02-01..2027-03-02 (exclusive end, containing the final overnight tail), approved study leave is an explicit non-work availability record, both directional recovery rules require 600 minutes, weekly count quota is 1..=3, weekly elapsed bucket quota is 240..=2400 minutes, rolling maximum is 6000 minutes per 14 days, and weekly assignment maximum is 3.".to_owned(),
            "Reviewed burden/preference metrics: all-hours, clinic-hours, call-hours and combined-count source buckets are retained; clinician weights are 1/1, 3/4 or 1/2; Required distinct worked weekends cap at 2 and explicit weekendTarget prefers 1 across the horizon; operator totals report period, burden and preference deviations rather than a prescribed roster.".to_owned(),
            "Boundary and scope decisions are explicit: weekly quotas use four authored Monday-midnight periods from February 1 through March 1; the final overnight tail remains work but lies outside those quota periods. Other elapsed/weekend membership still includes overnight tails, no unpaid-break or administrative-leave credit is inferred, no new-work grouping or guaranteed free/consecutive weekend is claimed, and no linked split-session alternative is modeled.".to_owned(),
        ]
    } else {
        vec![detail.to_owned()]
    };
    ExpectedCase {
        id: case,
        disposition,
        people,
        horizon_days,
        resolved_shifts,
        selected_assignments,
        score,
        model_envelope,
        deferred_obligations: vec![],
        semantics,
    }
}

fn semantic_description(case: CaseId) -> &'static str {
    match case {
        CaseId::AppendixF => {
            "Source-complete Appendix F remains infeasible: Smith's full local September 10 absence excludes the preceding overnight call and morning clinic; the remaining call owner cannot cover that clinic after only 120 of the Required 600 rest minutes, leaving one clinician for exact-two coverage. Preserve all three people, full-day absence, coverage, rest and active preferences; no accepted assignment count or score is claimed."
        }
        CaseId::ClinicTiny => {
            "Three clinicians, five four-hour weekday shifts, exact two physicians, one full local day unavailable; basic Required rules only"
        }
        CaseId::ClinicInitial => {
            "Eight clinicians/four weeks; availability and exact two physicians; explicitly omits clinic-full workload balance and Friday preference"
        }
        CaseId::ClinicFull => {
            "Eight clinicians/four weeks; exact two physicians plus active whole-plan weighted equal-share balance and whole-Friday avoidance; current compiler supports the complete source"
        }
        CaseId::ClinicOvernight => {
            "Twelve clinicians;20 clinic shifts and28 overnight calls; exact2/1 qualified coverage; unavailable day intersects preceding overnight;600 elapsed minutes call-to-clinic rest"
        }
        CaseId::RollingHours => {
            "Eight clinicians/four weeks; overlapping7-day1200-minute and14-day2400-minute Intersection hour limits plus a concrete first-clinic hard lock; Phase07"
        }
        CaseId::SpecialistCoverage => {
            "Six physicians, exactly two also specialists; each shift requires exactly two physicians including at least one specialist through Coverage.qualification_minimums; no RequiredSkillMix"
        }
        CaseId::RepairBefore => {
            "Four clinicians ordinary supported before-input; no BaseSchedule or accepted/selected authority; separately typed call-out mutation remains an independent recipe requiring a genuine accepted and selected baseline"
        }
        CaseId::InfeasibleCoverage => {
            "Three active qualified people but only one eligible for a clinic needing exactly two; structurally valid model must reach backend infeasible termination"
        }
        CaseId::PractitionerWorkflow => {
            "Twelve fictional clinicians across 2027-02-01..2027-03-01: 20 weekday four-hour clinics and 28 ten-hour overnight calls; exact qualified coverage, dated eligibility, approved leave, 600-minute directional rest, count/hour quotas, maxima, hard-protected anchor, distinct-weekend cap and explicit weekend target jointly optimize one source"
        }
        CaseId::PractitionerInfeasible => {
            "Same twelve-clinician combined source and boundaries, but every clinic requires two distinct specialist-qualified people while only one dated specialist grant is authored; this is a genuine mapped specialist-coverage infeasible variation, not a time-limit result"
        }
        CaseId::DstSpring => {
            "Chicago2026-03-08 gap02:30 moved forward;03:30 actual start/04:30 end;60 elapsed and120 scheduled minutes; Reject fails with Gap"
        }
        CaseId::DstFall => {
            "Chicago2026-11-01 first01:30 to02:30;120 elapsed and60 scheduled minutes; Later gives60 elapsed; Reject fails with Overlap"
        }
        CaseId::LargeSupported => {
            "Exactly101 active universally eligible people by12 independent daily one-hour shifts:1212 potential/surviving pairs; no pruning; accepted profile pending exact first-run characterization"
        }
        CaseId::LargePressure => {
            "Exactly100 active universally eligible people by64 independent daily one-hour shifts:6400 unpruned pairs; expected typed resource limit, never infeasible or accepted; metrics unavailable unless actually produced"
        }
    }
}

fn repair_fixture(before: &ScenarioSnapshotV1) -> Result<RepairFixture> {
    let case = CaseId::PractitionerWorkflow;
    let shifts = temporal::resolve_shifts(&before.document, &CancellationToken::new())?;
    let clinic_template =
        id::<eutheto_workforce::ids::ShiftTemplateId>(case, "template-weekday-clinic")?;
    let mut targets = shifts.iter().filter(|shift| {
        shift.reporting_date.to_string() == "2027-02-10"
            && matches!(
                shift.origin,
                temporal::ResolvedShiftOrigin::Generated { template_id, .. }
                    if template_id == clinic_template
            )
    });
    let target = targets.next().context("missing repair target clinic")?;
    ensure!(targets.next().is_none(), "ambiguous repair target clinic");
    Ok(RepairFixture {
        format: REPAIR_FORMAT.to_owned(),
        schema_version: REPAIR_SCHEMA_VERSION,
        corpus_version: CORPUS_VERSION,
        before_case: CaseId::PractitionerWorkflow,
        execution_phase: 7,
        baseline_requirement: BASELINE_REQUIREMENT.to_owned(),
        command_id: commands::ADD_ENTITY.to_owned(),
        availability_id: id(case, "availability-callout")?,
        target_shift_id: target.id,
        time_window: TimeWindow::Instant {
            starts_at: "2027-02-10T13:00:00Z".parse()?,
            ends_at: "2027-02-11T01:00:00Z".parse()?,
        },
        effective_range: date_range("2027-02-01", "2027-03-01")?,
        repair_requirement: REPAIR_REQUIREMENT.to_owned(),
    })
}

pub(crate) fn repair_payload(
    recipe: &RepairFixture,
    person_id: PersonId,
) -> commands::EntityPayload {
    commands::EntityPayload {
        entity: WorkforceEntity::Availability(Availability {
            id: recipe.availability_id,
            person_id,
            availability_kind: AvailabilityKind::Unavailable,
            time_window: recipe.time_window.clone(),
            effective_range: recipe.effective_range,
            assignment_type_ids: None,
            location_ids: None,
            source: "synthetic-practitioner-callout".to_owned(),
            note: "Synthetic unexpected call-out".to_owned(),
        }),
    }
}

pub(crate) fn build() -> Result<CorpusDefinitions> {
    let mut fixtures = Vec::with_capacity(CaseId::ALL.len());
    for case in CaseId::ALL {
        let snapshot = match case {
            CaseId::AppendixF => appendix()?,
            CaseId::PractitionerWorkflow => practitioner(case, false)?,
            CaseId::PractitionerInfeasible => practitioner(case, true)?,
            CaseId::DstSpring | CaseId::DstFall => dst(case)?,
            CaseId::LargeSupported => large(
                case,
                LargeParameters {
                    people: 101,
                    shifts: 12,
                },
            )?,
            CaseId::LargePressure => large(
                case,
                LargeParameters {
                    people: 100,
                    shifts: 64,
                },
            )?,
            _ => clinic(case)?,
        };
        let family = match case {
            CaseId::AppendixF => None,
            CaseId::ClinicTiny | CaseId::ClinicInitial | CaseId::ClinicFull => {
                Some(CorpusFamily::ClinicSmall)
            }
            CaseId::ClinicOvernight => Some(CorpusFamily::ClinicOvernight),
            CaseId::RollingHours => Some(CorpusFamily::RollingHours),
            CaseId::SpecialistCoverage => Some(CorpusFamily::SpecialistCoverage),
            CaseId::RepairBefore => Some(CorpusFamily::RepairCallout),
            CaseId::PractitionerWorkflow | CaseId::PractitionerInfeasible => {
                Some(CorpusFamily::PractitionerWorkflow)
            }
            CaseId::InfeasibleCoverage => Some(CorpusFamily::InfeasibleCoverage),
            CaseId::DstSpring | CaseId::DstFall => Some(CorpusFamily::Dst),
            CaseId::LargeSupported | CaseId::LargePressure => Some(CorpusFamily::LargeSynthetic),
        };
        let workload_class = match case {
            CaseId::ClinicTiny
            | CaseId::InfeasibleCoverage
            | CaseId::DstSpring
            | CaseId::DstFall => WorkloadClass::A,
            CaseId::LargeSupported => WorkloadClass::C,
            CaseId::LargePressure => WorkloadClass::D,
            _ => WorkloadClass::B,
        };
        fixtures.push(FixtureDefinition {
            id: case,
            family,
            workload_class,
            snapshot,
            expected: expected(case),
        });
    }
    let before = fixtures
        .iter()
        .find(|fixture| fixture.id == CaseId::PractitionerWorkflow)
        .context("repair before case absent")?;
    let repair = repair_fixture(&before.snapshot)?;
    Ok(CorpusDefinitions { fixtures, repair })
}
