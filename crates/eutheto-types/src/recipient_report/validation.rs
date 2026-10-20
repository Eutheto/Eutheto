// SPDX-License-Identifier: Apache-2.0
use super::{
    EntityId, MAX_RECIPIENT_REPORT_ROWS, PersonId, RECIPIENT_REPORT_SCHEMA_VERSION,
    RecipientChangeBasisV1, RecipientChangesV1, RecipientContentV1, RecipientEndpointV1,
    RecipientFractionV1, RecipientHistoricalEndpointV1, RecipientIntervalV1, RecipientLabelV1,
    RecipientMembershipV1, RecipientOriginV1, RecipientOverlapV1, RecipientPairChangeV1,
    RecipientReportOptionsV1, RecipientReportV1, RecipientWorkloadMeasureV1, WorkforceRecipientV1,
};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

/// A structural report failure. This never establishes native acceptance or publication custody.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RecipientReportError(&'static str);

impl fmt::Display for RecipientReportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}

impl std::error::Error for RecipientReportError {}

type Check = Result<(), RecipientReportError>;

fn require(condition: bool, message: &'static str) -> Check {
    if condition {
        Ok(())
    } else {
        Err(RecipientReportError(message))
    }
}

fn label(value: &RecipientLabelV1, allowed: bool) -> Check {
    require(
        allowed || matches!(value, RecipientLabelV1::Generated(_)),
        "unapproved authored label",
    )
}

fn digest(value: &str) -> Check {
    require(
        value.len() == 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "invalid report digest",
    )
}

fn interval(value: &RecipientIntervalV1) -> Check {
    require(
        value.starts_at.resolved.instant < value.ends_at.resolved.instant,
        "empty or reversed report interval",
    )?;
    for endpoint in [value.starts_at, value.ends_at] {
        require(
            RecipientEndpointV1::from_resolved(endpoint.resolved).ok() == Some(endpoint),
            "inconsistent actual civil time",
        )?;
    }
    Ok(())
}

fn date(value: &str) -> Check {
    let parsed = value
        .parse::<jiff::civil::Date>()
        .map_err(|_| RecipientReportError("invalid reporting date"))?;
    require(parsed.to_string() == value, "noncanonical reporting date")
}

fn fraction(value: &RecipientFractionV1) -> Result<(i128, i128), RecipientReportError> {
    let numerator = value
        .numerator
        .parse::<i128>()
        .map_err(|_| RecipientReportError("invalid exact numerator"))?;
    let denominator = value
        .denominator
        .parse::<i128>()
        .map_err(|_| RecipientReportError("invalid exact denominator"))?;
    require(
        numerator.to_string() == value.numerator
            && denominator > 0
            && denominator.to_string() == value.denominator,
        "noncanonical exact fraction",
    )?;
    Ok((numerator, denominator))
}

fn charge(rows: &mut usize, count: usize) -> Check {
    *rows = rows
        .checked_add(count)
        .filter(|total| *total <= MAX_RECIPIENT_REPORT_ROWS)
        .ok_or(RecipientReportError("report row limit exceeded"))?;
    Ok(())
}

impl RecipientReportV1 {
    /// Checks intrinsic consistency and disclosure switches, not the truth of imported claims.
    /// Encoded input/output must additionally respect `MAX_RECIPIENT_REPORT_BYTES` before retention.
    ///
    /// # Errors
    /// Rejects unsupported versions, inconsistent references/times/counts, or unapproved sections.
    pub fn validate(&self) -> Check {
        require(
            self.schema_version == RECIPIENT_REPORT_SCHEMA_VERSION,
            "unsupported recipient report version",
        )?;
        require(
            self.renderer.version > 0 && self.source.result_schema_version > 0,
            "invalid report version",
        )?;
        require(
            self.horizon.start < self.horizon.end,
            "invalid report horizon",
        )?;
        for value in [
            &self.renderer.javascript_sha256,
            &self.renderer.css_sha256,
            &self.source.accepted_result_checksum,
            &self.source.verification_report_checksum,
            &self.source.source_document_hash,
        ] {
            digest(value)?;
        }
        require(
            matches!(self.source.origin, RecipientOriginV1::VerifiedFile)
                == self.source.observed_current_revision.is_none(),
            "inconsistent report source origin",
        )?;
        label(&self.title, self.privacy.include_authored_labels)?;
        let RecipientContentV1::Workforce(schedule) = &self.content;
        schedule.validate(self.privacy)
    }
}

impl WorkforceRecipientV1 {
    // The checks share one bounded census and its reference indexes.
    #[allow(clippy::too_many_lines)]
    fn validate(&self, privacy: RecipientReportOptionsV1) -> Check {
        require(
            self.staffing.is_some() == privacy.include_staffing
                && self.changes.is_some() == privacy.include_changes
                && self.fairness_actuals.is_some() == privacy.include_fairness_actuals,
            "report section does not match disclosure",
        )?;
        // Bound the complete census before allocating any reference indexes.
        let mut rows = 0;
        for count in [
            self.people.len(),
            self.assignment_types.len(),
            self.locations.len(),
            self.shifts.len(),
            self.assignments.len(),
        ] {
            charge(&mut rows, count)?;
        }
        if let Some(staffing) = &self.staffing {
            charge(&mut rows, staffing.len())?;
        }
        if let Some(changes) = &self.changes {
            charge(&mut rows, changes.len())?;
            for comparison in changes {
                charge(&mut rows, comparison.rows.len())?;
            }
        }
        if let Some(groups) = &self.fairness_actuals {
            charge(&mut rows, groups.len())?;
            for group in groups {
                for count in [
                    group.people.len(),
                    group.qualifying_shifts.len(),
                    group.periods.len(),
                ] {
                    charge(&mut rows, count)?;
                }
                for period in &group.periods {
                    charge(&mut rows, period.actuals.len())?;
                }
            }
        }
        let mut people = BTreeSet::new();
        for person in &self.people {
            require(people.insert(person.person_id), "duplicate report person")?;
            label(&person.label, privacy.include_person_names)?;
        }
        let mut types = BTreeMap::new();
        for entity in &self.assignment_types {
            require(
                types.insert(entity.entity_id, false).is_none(),
                "duplicate report assignment type",
            )?;
            label(&entity.label, privacy.include_authored_labels)?;
        }
        let mut locations = BTreeMap::new();
        for entity in &self.locations {
            require(
                locations.insert(entity.entity_id, false).is_none(),
                "duplicate report location",
            )?;
            label(&entity.label, privacy.include_authored_labels)?;
        }
        let mut counts = BTreeMap::new();
        for shift in &self.shifts {
            require(
                counts.insert(shift.shift_id, 0_u32).is_none(),
                "duplicate report shift",
            )?;
            *types
                .get_mut(&shift.assignment_type_id)
                .ok_or(RecipientReportError("unresolved report work reference"))? = true;
            if let Some(id) = shift.location_id {
                *locations
                    .get_mut(&id)
                    .ok_or(RecipientReportError("unresolved report work reference"))? = true;
            }
            interval(&shift.interval)?;
            date(&shift.reporting_date)?;
        }
        require(
            types.values().all(|used| *used) && locations.values().all(|used| *used),
            "unreferenced report work label",
        )?;
        let mut assignments = BTreeSet::new();
        let mut pairs = BTreeSet::new();
        for assignment in &self.assignments {
            require(
                !assignment.assignment_id.is_empty()
                    && assignments.insert(&assignment.assignment_id)
                    && pairs.insert((assignment.person_id, assignment.shift_id)),
                "duplicate or empty report assignment identity",
            )?;
            require(
                people.contains(&assignment.person_id),
                "unresolved report person",
            )?;
            let count = counts
                .get_mut(&assignment.shift_id)
                .ok_or(RecipientReportError("unresolved report shift"))?;
            *count = count
                .checked_add(1)
                .ok_or(RecipientReportError("staffing count overflow"))?;
        }
        if let Some(staffing) = &self.staffing {
            let mut seen = BTreeSet::new();
            for row in staffing {
                require(
                    seen.insert(row.shift_id)
                        && counts.get(&row.shift_id) == Some(&row.assigned_count),
                    "inconsistent staffing count",
                )?;
            }
            require(seen.len() == counts.len(), "incomplete staffing census")?;
        }
        if let Some(groups) = &self.fairness_actuals {
            for group in groups {
                require(
                    match group.measure {
                        RecipientWorkloadMeasureV1::AssignmentCount => {
                            group.overlap != RecipientOverlapV1::Union
                        }
                        RecipientWorkloadMeasureV1::ElapsedMinutes => true,
                        RecipientWorkloadMeasureV1::ScheduledMinutes => {
                            group.overlap != RecipientOverlapV1::Union
                                && group.membership != RecipientMembershipV1::Intersection
                        }
                    },
                    "unsupported report measurement semantics",
                )?;
                let mut peers = BTreeSet::new();
                for person in &group.people {
                    require(
                        people.contains(person) && peers.insert(*person),
                        "invalid fairness population",
                    )?;
                }
                let mut shifts = BTreeSet::new();
                for shift in &group.qualifying_shifts {
                    require(
                        counts.contains_key(shift) && shifts.insert(*shift),
                        "invalid fairness contribution reference",
                    )?;
                }
                for period in &group.periods {
                    interval(&period.interval)?;
                    require(
                        period.actuals.len() == group.people.len(),
                        "incomplete fairness population",
                    )?;
                    for (actual, person) in period.actuals.iter().zip(&group.people) {
                        require(
                            actual.person_id == *person,
                            "inconsistent fairness population order",
                        )?;
                        let (numerator, denominator) = fraction(&actual.actual)?;
                        require(
                            match group.measure {
                                RecipientWorkloadMeasureV1::AssignmentCount => {
                                    numerator >= 0 && numerator % denominator == 0
                                }
                                RecipientWorkloadMeasureV1::ElapsedMinutes => numerator >= 0,
                                RecipientWorkloadMeasureV1::ScheduledMinutes => true,
                            },
                            "actual contradicts its measurement",
                        )?;
                    }
                }
            }
        }
        if let Some(changes) = &self.changes {
            Self::validate_changes(changes, &pairs, privacy)?;
        }
        Ok(())
    }

    fn validate_changes(
        changes: &[RecipientChangesV1],
        current: &BTreeSet<(PersonId, EntityId)>,
        privacy: RecipientReportOptionsV1,
    ) -> Check {
        let mut original = false;
        let mut manual = false;
        let mut external = false;
        for comparison in changes {
            let seen = match comparison.basis {
                RecipientChangeBasisV1::RecordedOriginal => &mut original,
                RecipientChangeBasisV1::RecordedManual => &mut manual,
                RecipientChangeBasisV1::ExternalStartingPlan => &mut external,
            };
            require(!*seen, "duplicate change comparison")?;
            *seen = true;
            require(
                matches!(comparison.basis, RecipientChangeBasisV1::RecordedOriginal)
                    == comparison.original.is_some(),
                "inconsistent original identity",
            )?;
            if let Some(reference) = &comparison.original {
                digest(&reference.accepted_result_checksum)?;
            }
            let mut pairs = BTreeSet::new();
            let mut current_count = 0;
            for row in &comparison.rows {
                let pair = (row.person_id, row.shift_id);
                require(pairs.insert(pair), "duplicate change pair")?;
                let has_current = !matches!(row.membership, RecipientPairChangeV1::Removed);
                require(
                    current.contains(&pair) == has_current,
                    "inconsistent current change membership",
                )?;
                if has_current {
                    current_count += 1;
                }
                require(
                    row.previous.is_some()
                        != matches!(row.membership, RecipientPairChangeV1::Added),
                    "inconsistent historical change membership",
                )?;
                if let Some(RecipientHistoricalEndpointV1::Available(facts)) = &row.previous {
                    label(&facts.person_label, privacy.include_person_names)?;
                    label(
                        &facts.assignment_type.label,
                        privacy.include_authored_labels,
                    )?;
                    if let Some(location) = &facts.location {
                        label(&location.label, privacy.include_authored_labels)?;
                    }
                    interval(&facts.interval)?;
                    date(&facts.reporting_date)?;
                }
            }
            require(
                current_count == current.len(),
                "incomplete current change census",
            )?;
        }
        if manual && !original {
            return Err(RecipientReportError(
                "manual comparison requires its original",
            ));
        }
        require(
            !external || (!original && !manual),
            "incompatible change sources",
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recipient_report::*;

    #[test]
    #[allow(clippy::too_many_lines)] // One shared complete schedule for the disclosure boundary cases.
    fn recipient_admission_preserves_complete_counts_populations_and_disclosure()
    -> Result<(), Box<dyn std::error::Error>> {
        let person_id = "018f0f04-0000-7000-8000-000000000001".parse()?;
        let shift_id = "018f0f04-0000-7000-8000-000000000002".parse()?;
        let assignment_type_id = "018f0f04-0000-7000-8000-000000000003".parse()?;
        let endpoint = |instant, local| -> Result<_, Box<dyn std::error::Error>> {
            Ok(RecipientEndpointV1::from_resolved(ResolvedLocalTime {
                instant: Rfc3339Timestamp::parse(instant)?,
                local: LocalWallTime::parse(local)?,
                offset_seconds: 0,
            })?)
        };
        let work = RecipientIntervalV1 {
            starts_at: endpoint("2024-03-10T08:00:00Z", "2024-03-10T08:00:00")?,
            ends_at: endpoint("2024-03-10T09:00:00Z", "2024-03-10T09:00:00")?,
        };
        let mut schedule = WorkforceRecipientV1 {
            people: vec![RecipientPersonV1 {
                person_id,
                label: RecipientLabelV1::Generated("Person 1".into()),
            }],
            assignment_types: vec![RecipientEntityV1 {
                entity_id: assignment_type_id,
                label: RecipientLabelV1::Generated("Work 1".into()),
            }],
            locations: vec![],
            shifts: vec![RecipientShiftV1 {
                shift_id,
                assignment_type_id,
                location_id: None,
                interval: work,
                reporting_date: "2024-03-10".into(),
            }],
            assignments: vec![],
            staffing: Some(vec![RecipientStaffingV1 {
                shift_id,
                assigned_count: 0,
            }]),
            changes: None,
            fairness_actuals: Some(vec![RecipientActualGroupV1 {
                measure: RecipientWorkloadMeasureV1::ScheduledMinutes,
                membership: RecipientMembershipV1::StartInstant,
                overlap: RecipientOverlapV1::Sum,
                people: vec![person_id],
                qualifying_shifts: vec![shift_id],
                periods: vec![RecipientActualPeriodV1 {
                    interval: work,
                    actuals: vec![RecipientActualV1 {
                        person_id,
                        actual: RecipientFractionV1 {
                            numerator: "0".into(),
                            denominator: "1".into(),
                        },
                    }],
                }],
            }]),
        };
        let privacy = RecipientReportOptionsV1 {
            include_staffing: true,
            include_fairness_actuals: true,
            ..Default::default()
        };
        schedule.validate(privacy)?;
        for (measure, membership, overlap, numerator, expected) in [
            (
                RecipientWorkloadMeasureV1::AssignmentCount,
                RecipientMembershipV1::StartInstant,
                RecipientOverlapV1::Sum,
                "-1",
                Err(RecipientReportError("actual contradicts its measurement")),
            ),
            (
                RecipientWorkloadMeasureV1::AssignmentCount,
                RecipientMembershipV1::StartInstant,
                RecipientOverlapV1::Sum,
                "1",
                Err(RecipientReportError("actual contradicts its measurement")),
            ),
            (
                RecipientWorkloadMeasureV1::ElapsedMinutes,
                RecipientMembershipV1::Intersection,
                RecipientOverlapV1::Sum,
                "-1",
                Err(RecipientReportError("actual contradicts its measurement")),
            ),
            (
                RecipientWorkloadMeasureV1::ScheduledMinutes,
                RecipientMembershipV1::StartInstant,
                RecipientOverlapV1::Sum,
                "-1",
                Ok(()),
            ),
            (
                RecipientWorkloadMeasureV1::AssignmentCount,
                RecipientMembershipV1::StartInstant,
                RecipientOverlapV1::Union,
                "2",
                Err(RecipientReportError(
                    "unsupported report measurement semantics",
                )),
            ),
            (
                RecipientWorkloadMeasureV1::ScheduledMinutes,
                RecipientMembershipV1::Intersection,
                RecipientOverlapV1::Sum,
                "2",
                Err(RecipientReportError(
                    "unsupported report measurement semantics",
                )),
            ),
            (
                RecipientWorkloadMeasureV1::ScheduledMinutes,
                RecipientMembershipV1::StartInstant,
                RecipientOverlapV1::Union,
                "2",
                Err(RecipientReportError(
                    "unsupported report measurement semantics",
                )),
            ),
        ] {
            let mut altered = schedule.clone();
            let group = &mut altered.fairness_actuals.as_mut().ok_or("missing actuals")?[0];
            group.measure = measure;
            group.membership = membership;
            group.overlap = overlap;
            group.periods[0].actuals[0].actual = RecipientFractionV1 {
                numerator: numerator.into(),
                denominator: "2".into(),
            };
            assert_eq!(altered.validate(privacy), expected);
        }
        let mut unrelated = schedule.clone();
        unrelated.locations.push(RecipientEntityV1 {
            entity_id: "018f0f04-0000-7000-8000-000000000004".parse()?,
            label: RecipientLabelV1::Generated("Unrelated location".into()),
        });
        assert_eq!(
            unrelated.validate(privacy),
            Err(RecipientReportError("unreferenced report work label"))
        );
        schedule.assignments.push(RecipientAssignmentV1 {
            assignment_id: "native-assignment".into(),
            person_id,
            shift_id,
        });
        assert_eq!(
            schedule.validate(privacy),
            Err(RecipientReportError("inconsistent staffing count"))
        );
        schedule.staffing.as_mut().ok_or("missing staffing")?[0].assigned_count = 1;
        schedule.validate(privacy)?;
        let actuals = &mut schedule
            .fairness_actuals
            .as_mut()
            .ok_or("missing actuals")?[0]
            .periods[0]
            .actuals;
        let actual = actuals.pop().ok_or("missing person")?;
        assert_eq!(
            schedule.validate(privacy),
            Err(RecipientReportError("incomplete fairness population"))
        );
        schedule
            .fairness_actuals
            .as_mut()
            .ok_or("missing actuals")?[0]
            .periods[0]
            .actuals
            .push(actual);
        schedule.people[0].label = RecipientLabelV1::Authored("Private name".into());
        assert_eq!(
            schedule.validate(privacy),
            Err(RecipientReportError("unapproved authored label"))
        );
        schedule.validate(RecipientReportOptionsV1 {
            include_person_names: true,
            ..privacy
        })?;
        Ok(())
    }

    #[test]
    fn recipient_admission_retains_missing_history_without_inventing_current_facts()
    -> Result<(), Box<dyn std::error::Error>> {
        let person_id = "018f0f04-0000-7000-8000-000000000001".parse()?;
        let shift_id = "018f0f04-0000-7000-8000-000000000002".parse()?;
        let old_shift = "018f0f04-0000-7000-8000-000000000003".parse()?;
        let current = BTreeSet::from([(person_id, shift_id)]);
        let mut comparison = RecipientChangesV1 {
            basis: RecipientChangeBasisV1::ExternalStartingPlan,
            original: None,
            rows: vec![
                RecipientChangeRowV1 {
                    person_id,
                    shift_id,
                    membership: RecipientPairChangeV1::Added,
                    previous: None,
                },
                RecipientChangeRowV1 {
                    person_id,
                    shift_id: old_shift,
                    membership: RecipientPairChangeV1::Removed,
                    previous: Some(RecipientHistoricalEndpointV1::Unavailable),
                },
            ],
        };
        WorkforceRecipientV1::validate_changes(
            std::slice::from_ref(&comparison),
            &current,
            RecipientReportOptionsV1::default(),
        )?;
        comparison.rows.remove(0);
        assert_eq!(
            WorkforceRecipientV1::validate_changes(
                std::slice::from_ref(&comparison),
                &current,
                RecipientReportOptionsV1::default()
            ),
            Err(RecipientReportError("incomplete current change census"))
        );
        fraction(&RecipientFractionV1 {
            numerator: "-170141183460469231731687303715884105728".into(),
            denominator: "2".into(),
        })?;
        assert!(
            fraction(&RecipientFractionV1 {
                numerator: "01".into(),
                denominator: "2".into()
            })
            .is_err()
        );
        assert!(
            fraction(&RecipientFractionV1 {
                numerator: "1".into(),
                denominator: "0".into()
            })
            .is_err()
        );
        Ok(())
    }
}
