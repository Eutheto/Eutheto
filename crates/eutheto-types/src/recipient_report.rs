// SPDX-License-Identifier: Apache-2.0
//! Inert recipient data. Structural validity and public checksums never grant acceptance custody.

use crate::{
    EntityId, Horizon, IanaTimeZone, LocalWallTime, LocaleTag, PersonId, ResolvedLocalTime,
    Revision, Rfc3339Timestamp, ScenarioId, SolutionId,
};
use serde::{Deserialize, Deserializer, Serialize};

mod validation;
pub use validation::RecipientReportError;

/// Canonical recipient-model ceiling; apply before parsing untrusted encoded data.
pub const MAX_RECIPIENT_REPORT_BYTES: usize = 4 * 1024 * 1024;
/// Whole-report row ceiling, shared by all selected sections rather than reset per group.
pub const MAX_RECIPIENT_REPORT_ROWS: usize = 100_000;
/// Includes inert-data escaping, the fixed renderer and styles.
pub const MAX_RECIPIENT_HTML_BYTES: usize = 32 * 1024 * 1024;
/// Conservative presentation-work ceiling, not a claim about browser heap bytes.
/// Cost is 65536 + 4×canonical bytes + (census rows + 1)×(4096 + 16×largest
/// referenced UTF-8 text length, at least 64). The row allowance covers elements,
/// generated text and all schedule views, including replacement during printing.
pub const MAX_RECIPIENT_PRESENTATION_COST: usize = 64 * 1024 * 1024;
/// The recipient wire is separate from the identity-only Workforce Share V2 contract.
pub const RECIPIENT_REPORT_SCHEMA_VERSION: u32 = 1;

/// Allowed disclosures only. None grants access to source notes, constraints or preferences.
// Independent opt-ins, not mutually exclusive lifecycle states.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecipientReportOptionsV1 {
    #[serde(default)]
    pub include_person_names: bool,
    #[serde(default)]
    pub include_authored_labels: bool,
    #[serde(default)]
    pub include_staffing: bool,
    #[serde(default)]
    pub include_changes: bool,
    #[serde(default)]
    pub include_fairness_actuals: bool,
}

/// One current recipient profile, not a promise of future coordinator/custom behavior.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum RecipientProfileV1 {
    #[serde(rename = "participant-friendly")]
    ParticipantFriendly,
}

/// Host report namespace, distinct from a portable scenario or identity-only pack share.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum RecipientReportFormat {
    #[serde(rename = "eutheto/share-result")]
    EuthetoShareResult,
}

/// Exact self-contained presentation artifacts used for this approval.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecipientRendererV1 {
    pub version: u32,
    pub javascript_sha256: String,
    pub css_sha256: String,
}

/// Generated aliases remain linkable through the included stable identities.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    content = "text",
    rename_all = "camelCase",
    deny_unknown_fields
)]
pub enum RecipientLabelV1 {
    Generated(String),
    Authored(String),
}

/// Provenance classification, not an assertion that source-provided solver history is authentic.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RecipientOriginV1 {
    StoredTerminal,
    StoredCheckpoint,
    VerifiedFile,
}

/// Fresh feasibility is distinct from optimization proof and historical backend metadata.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RecipientVerificationV1 {
    VerifiedFeasible,
}

/// Bounded recorded solver identity; no parameters, score, diagnostic or raw run envelope.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecipientSolverV1 {
    pub backend_id: String,
    pub recorded_version: Option<String>,
}

/// Immutable accepted source, with the visible head observed separately when one exists.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecipientSourceV1 {
    pub scenario_id: ScenarioId,
    pub scenario_revision: Revision,
    pub solution_id: SolutionId,
    pub result_schema_version: u32,
    pub accepted_result_checksum: String,
    pub verification_report_checksum: String,
    pub source_document_hash: String,
    pub origin: RecipientOriginV1,
    pub observed_current_revision: Option<Revision>,
    pub verification: RecipientVerificationV1,
    pub solver: RecipientSolverV1,
}

/// Actual civil time is derived natively from the captured instant and offset, not original intent.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecipientEndpointV1 {
    pub resolved: ResolvedLocalTime,
    pub actual: LocalWallTime,
}

impl RecipientEndpointV1 {
    /// Preserves authored gap/fold intent alongside the actual civil time recipients must use.
    ///
    /// # Errors
    /// Rejects an invalid captured UTC offset.
    pub fn from_resolved(resolved: ResolvedLocalTime) -> Result<Self, jiff::Error> {
        let offset = jiff::tz::Offset::from_seconds(resolved.offset_seconds)?;
        let actual = LocalWallTime::from_datetime(
            resolved
                .instant
                .as_timestamp()
                .to_zoned(jiff::tz::TimeZone::fixed(offset))
                .datetime(),
        );
        Ok(Self { resolved, actual })
    }
}

/// Half-open work or measurement interval; authored and actual endpoint dates remain distinct.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecipientIntervalV1 {
    pub starts_at: RecipientEndpointV1,
    pub ends_at: RecipientEndpointV1,
}

/// Native person identity and the one approved display label, never a hidden raw name.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecipientPersonV1 {
    pub person_id: PersonId,
    pub label: RecipientLabelV1,
}

/// Domain entity reference converted to a host identity without depending on a domain crate.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecipientEntityV1 {
    pub entity_id: EntityId,
    pub label: RecipientLabelV1,
}

/// Only recipient work facts. No coverage requirements, qualifications, notes or policy fields.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecipientShiftV1 {
    pub shift_id: EntityId,
    pub assignment_type_id: EntityId,
    pub location_id: Option<EntityId>,
    pub interval: RecipientIntervalV1,
    pub reporting_date: String,
}

/// The assignment ID comes verbatim from the accepted native result, not a recipient-side codec.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecipientAssignmentV1 {
    pub assignment_id: String,
    pub person_id: PersonId,
    pub shift_id: EntityId,
}

/// Staffing actuals are not a required-coverage or qualification assessment.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecipientStaffingV1 {
    pub shift_id: EntityId,
    pub assigned_count: u32,
}

/// Decimal-string rational values preserve signed/fractional facts beyond JavaScript integers.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecipientFractionV1 {
    pub numerator: String,
    pub denominator: String,
}

/// Exact measured quantity, not an interchangeable generic minutes label.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RecipientWorkloadMeasureV1 {
    AssignmentCount,
    ElapsedMinutes,
    ScheduledMinutes,
}

/// Native attribution of work to measurement periods.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RecipientMembershipV1 {
    ReportingDate,
    StartInstant,
    Intersection,
}

/// Whether overlapping contributions are counted separately or as their elapsed union.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RecipientOverlapV1 {
    Sum,
    Union,
}

/// One member's exact actual, including zero for a qualifying member without contributions.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecipientActualV1 {
    pub person_id: PersonId,
    pub actual: RecipientFractionV1,
}

/// Complete population for one native measurement period, never a visible-page sample.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecipientActualPeriodV1 {
    pub interval: RecipientIntervalV1,
    pub actuals: Vec<RecipientActualV1>,
}

/// One configured context; its position is a display caption, not a native policy identity.
///
/// Separate contexts remain separate even when all their public values happen to match.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecipientActualGroupV1 {
    pub measure: RecipientWorkloadMeasureV1,
    pub membership: RecipientMembershipV1,
    pub overlap: RecipientOverlapV1,
    pub people: Vec<PersonId>,
    pub qualifying_shifts: Vec<EntityId>,
    pub periods: Vec<RecipientActualPeriodV1>,
}

/// The kind of recorded before-source is independent of whether its original result was re-admitted.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RecipientChangeBasisV1 {
    RecordedOriginal,
    RecordedManual,
    ExternalStartingPlan,
}

/// Only Core can supply native readmission; a serialized value cannot recreate that authority.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RecipientOriginalAdmissionV1 {
    FreshlyReadmittedNative,
    NotEstablished,
}

/// Descriptive identity of A, never the carrier entity ID or a substitute for complete native binding.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecipientOriginalV1 {
    pub scenario_id: ScenarioId,
    pub scenario_revision: Revision,
    pub solution_id: SolutionId,
    pub accepted_result_checksum: String,
    pub admission: RecipientOriginalAdmissionV1,
}

/// Membership only: an unchanged pair does not assert unchanged work times, labels or policies.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RecipientPairChangeV1 {
    Added,
    Removed,
    Retained,
}

/// Historical labels/times belong to their own exact source, not the current result's source.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecipientHistoricalFactsV1 {
    pub person_label: RecipientLabelV1,
    pub assignment_type: RecipientEntityV1,
    pub location: Option<RecipientEntityV1>,
    pub interval: RecipientIntervalV1,
    pub reporting_date: String,
}

/// Missing historical display facts do not erase a known pair from the comparison.
// Avoid one extra allocation per available row; admission must charge the full inline size.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "availability",
    content = "facts",
    rename_all = "camelCase",
    deny_unknown_fields
)]
pub enum RecipientHistoricalEndpointV1 {
    Available(RecipientHistoricalFactsV1),
    Unavailable,
}

/// Complete pair census. Added/retained current facts reference the current schedule tables.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecipientChangeRowV1 {
    pub person_id: PersonId,
    pub shift_id: EntityId,
    pub membership: RecipientPairChangeV1,
    pub previous: Option<RecipientHistoricalEndpointV1>,
}

/// A→C and B→C are distinct records; manual/external B never acquires original acceptance.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecipientChangesV1 {
    pub basis: RecipientChangeBasisV1,
    pub original: Option<RecipientOriginalV1>,
    pub rows: Vec<RecipientChangeRowV1>,
}

/// Complete selected schedule and optional source-derived recipient sections.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkforceRecipientV1 {
    pub people: Vec<RecipientPersonV1>,
    pub assignment_types: Vec<RecipientEntityV1>,
    pub locations: Vec<RecipientEntityV1>,
    pub shifts: Vec<RecipientShiftV1>,
    pub assignments: Vec<RecipientAssignmentV1>,
    pub staffing: Option<Vec<RecipientStaffingV1>>,
    pub changes: Option<Vec<RecipientChangesV1>>,
    pub fairness_actuals: Option<Vec<RecipientActualGroupV1>>,
}

/// Closed current domain product; unsupported future content is not silently reinterpreted.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    content = "schedule",
    rename_all = "camelCase",
    deny_unknown_fields
)]
pub enum RecipientContentV1 {
    Workforce(WorkforceRecipientV1),
}

/// One approved immutable model shared by preview, standalone HTML and direct PDF.
///
/// This document contains no library/window tokens or filesystem authority. Those stay native.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecipientReportV1 {
    pub format: RecipientReportFormat,
    #[serde(deserialize_with = "recipient_schema_version")]
    pub schema_version: u32,
    pub profile: RecipientProfileV1,
    pub generated_at: Rfc3339Timestamp,
    pub application_version: String,
    pub renderer: RecipientRendererV1,
    pub title: RecipientLabelV1,
    pub plan_version: String,
    pub locale: LocaleTag,
    pub time_zone: IanaTimeZone,
    pub horizon: Horizon,
    pub source: RecipientSourceV1,
    pub privacy: RecipientReportOptionsV1,
    pub content: RecipientContentV1,
}

impl RecipientReportV1 {
    /// Counts this inline model once and every recursively owned backing capacity.
    ///
    /// Includes unused vector/string capacity, not allocator overhead or RSS. Native
    /// source captures and handles are separate ownership and are not counted here.
    /// Returns `None` if any capacity multiplication or addition overflows.
    #[must_use]
    #[allow(clippy::too_many_lines)] // One exhaustive walk of the closed recipient model.
    pub fn retained_bytes(&self) -> Option<usize> {
        fn label_capacity(label: &RecipientLabelV1) -> usize {
            match label {
                RecipientLabelV1::Generated(text) | RecipientLabelV1::Authored(text) => {
                    text.capacity()
                }
            }
        }

        fn entity_capacity(entity: &RecipientEntityV1) -> usize {
            let RecipientEntityV1 {
                entity_id: _,
                label,
            } = entity;
            label_capacity(label)
        }

        let Self {
            format: _,
            schema_version: _,
            profile: _,
            generated_at: _,
            application_version,
            renderer,
            title,
            plan_version,
            locale,
            time_zone,
            horizon: _,
            source,
            privacy: _,
            content,
        } = self;
        let RecipientRendererV1 {
            version: _,
            javascript_sha256,
            css_sha256,
        } = renderer;
        let RecipientSourceV1 {
            scenario_id: _,
            scenario_revision: _,
            solution_id: _,
            result_schema_version: _,
            accepted_result_checksum,
            verification_report_checksum,
            source_document_hash,
            origin: _,
            observed_current_revision: _,
            verification: _,
            solver,
        } = source;
        let RecipientSolverV1 {
            backend_id,
            recorded_version,
        } = solver;
        let mut bytes = size_of::<Self>();
        for text in [
            application_version,
            javascript_sha256,
            css_sha256,
            plan_version,
            accepted_result_checksum,
            verification_report_checksum,
            source_document_hash,
            backend_id,
        ] {
            bytes = bytes.checked_add(text.capacity())?;
        }
        if let Some(version) = recorded_version {
            bytes = bytes.checked_add(version.capacity())?;
        }
        // These immutable private String wrappers construct with str::to_owned
        // (including deserialization); their backing capacity equals their length.
        bytes = bytes
            .checked_add(label_capacity(title))?
            .checked_add(locale.as_str().len())?
            .checked_add(time_zone.as_str().len())?;

        let RecipientContentV1::Workforce(WorkforceRecipientV1 {
            people,
            assignment_types,
            locations,
            shifts,
            assignments,
            staffing,
            changes,
            fairness_actuals,
        }) = content;
        for (capacity, item_size) in [
            (people.capacity(), size_of::<RecipientPersonV1>()),
            (assignment_types.capacity(), size_of::<RecipientEntityV1>()),
            (locations.capacity(), size_of::<RecipientEntityV1>()),
            (shifts.capacity(), size_of::<RecipientShiftV1>()),
            (assignments.capacity(), size_of::<RecipientAssignmentV1>()),
            (
                staffing.as_ref().map_or(0, Vec::capacity),
                size_of::<RecipientStaffingV1>(),
            ),
            (
                changes.as_ref().map_or(0, Vec::capacity),
                size_of::<RecipientChangesV1>(),
            ),
            (
                fairness_actuals.as_ref().map_or(0, Vec::capacity),
                size_of::<RecipientActualGroupV1>(),
            ),
        ] {
            bytes = bytes.checked_add(capacity.checked_mul(item_size)?)?;
        }
        for RecipientPersonV1 {
            person_id: _,
            label,
        } in people
        {
            bytes = bytes.checked_add(label_capacity(label))?;
        }
        for entity in assignment_types.iter().chain(locations) {
            bytes = bytes.checked_add(entity_capacity(entity))?;
        }
        for RecipientShiftV1 {
            shift_id: _,
            assignment_type_id: _,
            location_id: _,
            interval: _,
            reporting_date,
        } in shifts
        {
            bytes = bytes.checked_add(reporting_date.capacity())?;
        }
        for RecipientAssignmentV1 {
            assignment_id,
            person_id: _,
            shift_id: _,
        } in assignments
        {
            bytes = bytes.checked_add(assignment_id.capacity())?;
        }
        if let Some(changes) = changes {
            for RecipientChangesV1 {
                basis: _,
                original,
                rows,
            } in changes
            {
                if let Some(RecipientOriginalV1 {
                    scenario_id: _,
                    scenario_revision: _,
                    solution_id: _,
                    accepted_result_checksum,
                    admission: _,
                }) = original
                {
                    bytes = bytes.checked_add(accepted_result_checksum.capacity())?;
                }
                bytes = bytes.checked_add(
                    rows.capacity()
                        .checked_mul(size_of::<RecipientChangeRowV1>())?,
                )?;
                for RecipientChangeRowV1 {
                    person_id: _,
                    shift_id: _,
                    membership: _,
                    previous,
                } in rows
                {
                    match previous {
                        Some(RecipientHistoricalEndpointV1::Available(
                            RecipientHistoricalFactsV1 {
                                person_label,
                                assignment_type,
                                location,
                                interval: _,
                                reporting_date,
                            },
                        )) => {
                            bytes = bytes
                                .checked_add(label_capacity(person_label))?
                                .checked_add(entity_capacity(assignment_type))?
                                .checked_add(reporting_date.capacity())?;
                            if let Some(location) = location {
                                bytes = bytes.checked_add(entity_capacity(location))?;
                            }
                        }
                        None | Some(RecipientHistoricalEndpointV1::Unavailable) => {}
                    }
                }
            }
        }
        if let Some(groups) = fairness_actuals {
            for RecipientActualGroupV1 {
                measure: _,
                membership: _,
                overlap: _,
                people,
                qualifying_shifts,
                periods,
            } in groups
            {
                for (capacity, item_size) in [
                    (people.capacity(), size_of::<PersonId>()),
                    (qualifying_shifts.capacity(), size_of::<EntityId>()),
                    (periods.capacity(), size_of::<RecipientActualPeriodV1>()),
                ] {
                    bytes = bytes.checked_add(capacity.checked_mul(item_size)?)?;
                }
                for RecipientActualPeriodV1 {
                    interval: _,
                    actuals,
                } in periods
                {
                    bytes = bytes.checked_add(
                        actuals
                            .capacity()
                            .checked_mul(size_of::<RecipientActualV1>())?,
                    )?;
                    for RecipientActualV1 {
                        person_id: _,
                        actual:
                            RecipientFractionV1 {
                                numerator,
                                denominator,
                            },
                    } in actuals
                    {
                        bytes = bytes
                            .checked_add(numerator.capacity())?
                            .checked_add(denominator.capacity())?;
                    }
                }
            }
        }
        Some(bytes)
    }
}

fn recipient_schema_version<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u32, D::Error> {
    let version = u32::deserialize(deserializer)?;
    if version != RECIPIENT_REPORT_SCHEMA_VERSION {
        return Err(serde::de::Error::custom(
            "unsupported recipient report schema version",
        ));
    }
    Ok(version)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recipient_retained_bytes_counts_spare_and_nested_capacity()
    -> Result<(), Box<dyn std::error::Error>> {
        let endpoint = RecipientEndpointV1::from_resolved(ResolvedLocalTime {
            instant: Rfc3339Timestamp::parse("2024-03-10T08:00:00Z")?,
            local: LocalWallTime::parse("2024-03-10T08:00:00")?,
            offset_seconds: 0,
        })?;
        let interval = RecipientIntervalV1 {
            starts_at: endpoint,
            ends_at: endpoint,
        };
        let mut report = RecipientReportV1 {
            format: RecipientReportFormat::EuthetoShareResult,
            schema_version: RECIPIENT_REPORT_SCHEMA_VERSION,
            profile: RecipientProfileV1::ParticipantFriendly,
            generated_at: endpoint.resolved.instant,
            application_version: String::new(),
            renderer: RecipientRendererV1 {
                version: 1,
                javascript_sha256: String::new(),
                css_sha256: String::new(),
            },
            title: RecipientLabelV1::Generated(String::new()),
            plan_version: String::new(),
            locale: LocaleTag::parse("en")?,
            time_zone: IanaTimeZone::parse("UTC")?,
            horizon: Horizon {
                start: endpoint.resolved.instant,
                end: endpoint.resolved.instant,
            },
            source: RecipientSourceV1 {
                scenario_id: "018f0f04-0000-7000-8000-000000000001".parse()?,
                scenario_revision: Revision::INITIAL,
                solution_id: "018f0f04-0000-7000-8000-000000000002".parse()?,
                result_schema_version: 1,
                accepted_result_checksum: String::new(),
                verification_report_checksum: String::new(),
                source_document_hash: String::new(),
                origin: RecipientOriginV1::StoredTerminal,
                observed_current_revision: None,
                verification: RecipientVerificationV1::VerifiedFeasible,
                solver: RecipientSolverV1 {
                    backend_id: String::new(),
                    recorded_version: None,
                },
            },
            privacy: RecipientReportOptionsV1::default(),
            content: RecipientContentV1::Workforce(WorkforceRecipientV1 {
                people: Vec::with_capacity(3),
                assignment_types: vec![],
                locations: vec![],
                shifts: vec![],
                assignments: vec![],
                staffing: None,
                changes: None,
                fairness_actuals: Some(vec![RecipientActualGroupV1 {
                    measure: RecipientWorkloadMeasureV1::AssignmentCount,
                    membership: RecipientMembershipV1::StartInstant,
                    overlap: RecipientOverlapV1::Sum,
                    people: vec![],
                    qualifying_shifts: vec![],
                    periods: vec![RecipientActualPeriodV1 {
                        interval,
                        actuals: Vec::with_capacity(2),
                    }],
                }]),
            }),
        };
        let RecipientContentV1::Workforce(schedule) = &report.content;
        let groups = schedule
            .fairness_actuals
            .as_ref()
            .ok_or("missing actual groups")?;
        let expected = size_of::<RecipientReportV1>()
            + report.locale.as_str().len()
            + report.time_zone.as_str().len()
            + schedule.people.capacity() * size_of::<RecipientPersonV1>()
            + groups.capacity() * size_of::<RecipientActualGroupV1>()
            + groups[0].periods.capacity() * size_of::<RecipientActualPeriodV1>()
            + groups[0].periods[0].actuals.capacity() * size_of::<RecipientActualV1>();
        assert_eq!(report.retained_bytes(), Some(expected));

        // Lengths stay zero: serialized size cannot reveal either allocation.
        let version = String::with_capacity(257);
        let numerator = String::with_capacity(129);
        let growth = version.capacity() + numerator.capacity();
        report.source.solver.recorded_version = Some(version);
        let RecipientContentV1::Workforce(schedule) = &mut report.content;
        schedule
            .fairness_actuals
            .as_mut()
            .ok_or("missing actual groups")?[0]
            .periods[0]
            .actuals
            .push(RecipientActualV1 {
                person_id: "018f0f04-0000-7000-8000-000000000003".parse()?,
                actual: RecipientFractionV1 {
                    numerator,
                    denominator: String::new(),
                },
            });
        // Initializing a reserved element adds only its backing strings, not its
        // inline size for a second time.
        assert_eq!(report.retained_bytes(), Some(expected + growth));
        Ok(())
    }

    #[test]
    fn recipient_endpoint_keeps_gap_intent_but_displays_the_actual_civil_time()
    -> Result<(), Box<dyn std::error::Error>> {
        let source = ResolvedLocalTime {
            instant: Rfc3339Timestamp::parse("2024-03-10T07:30:00.125Z")?,
            local: LocalWallTime::parse("2024-03-10T02:30:00.125")?,
            offset_seconds: -14_400,
        };
        let endpoint = RecipientEndpointV1::from_resolved(source)?;
        assert_eq!(endpoint.resolved, source);
        assert_eq!(
            endpoint.actual,
            LocalWallTime::parse("2024-03-10T03:30:00.125")?
        );
        let encoded = serde_json::to_vec(&endpoint)?;
        assert_eq!(
            serde_json::from_slice::<RecipientEndpointV1>(&encoded)?,
            endpoint
        );
        assert!(
            RecipientEndpointV1::from_resolved(ResolvedLocalTime {
                offset_seconds: i32::MAX,
                ..source
            })
            .is_err()
        );
        Ok(())
    }

    #[test]
    fn recipient_staffing_wire_refuses_constraint_and_assessment_fields()
    -> Result<(), serde_json::Error> {
        let value = serde_json::json!({
            "shiftId": "018f0f04-0000-7000-8000-000000000001",
            "assignedCount": 2
        });
        let staffing: RecipientStaffingV1 = serde_json::from_value(value.clone())?;
        assert_eq!(staffing.assigned_count, 2);
        for field in [
            "minimum",
            "maximum",
            "preferred",
            "margin",
            "minimumRank",
            "satisfied",
        ] {
            let mut injected = value.clone();
            injected[field] = serde_json::json!(1);
            assert!(serde_json::from_value::<RecipientStaffingV1>(injected).is_err());
        }
        assert!(
            serde_json::from_value::<RecipientReportOptionsV1>(serde_json::json!({
                "includeStaffing": true,
                "includeRequiredCoverage": true
            }))
            .is_err()
        );
        Ok(())
    }
}
