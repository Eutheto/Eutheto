use super::{
    GenerationPreview, MAX_SHIFT_CHANGES, PriorShift, ShiftChange, ShiftChangeKind, TemporalError,
    TemporalIssueKind, check_cancelled,
    generation::{self, ShiftSpec},
    issue,
};
use crate::{
    commands,
    ids::{ShiftId, ShiftTemplateId},
    model::{Coverage, ShiftTemplate, WorkforceDomainV1, WorkforceEntity, planning_dates},
    validation::{common::invalid, validate_document_controlled},
};
use eutheto_domain_api::{DOMAIN_BATCH_SCHEMA_VERSION, DomainBatchCommand, DomainPackError};
use eutheto_types::{
    CancellationToken, DomainCommandEnvelope, EntityId, OperationControl, ScenarioDocument,
    collect_document_owned_uuids,
};
use jiff::civil::Date;
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};

/// Pure preview of the exact prospective document, including one compact identity command.
/// Does not apply the user's prospective edits, remove dormant definitions, or persist approval.
/// The host must compare `prospective_hash` to its target before applying the returned batch.
///
/// # Errors
/// Rejects invalid documents, different scenarios, identity transitions/collisions, unresolved
/// prospective time, over-budget reconciliation/output and cancellation. Unresolved prior
/// occurrences are represented without inventing their instants.
pub fn preview_generation(
    before: &ScenarioDocument,
    prospective: &ScenarioDocument,
    cancellation: &CancellationToken,
) -> Result<GenerationPreview, TemporalError> {
    preview_generation_checked(
        before,
        prospective,
        &OperationControl::Cancellation(cancellation.clone()),
        &mut || check_cancelled(cancellation),
    )
    .map(|(preview, _, _)| preview)
}

fn checked_prospective_hash(
    before: &ScenarioDocument,
    prospective: &ScenarioDocument,
) -> Result<[u8; 32], TemporalError> {
    if before.scenario_id != prospective.scenario_id {
        return Err(issue(TemporalIssueKind::DifferentScenario, None, None));
    }
    let mut hasher = blake3::Hasher::new();
    serde_json::to_writer(&mut hasher, prospective)
        .map_err(|_| invalid("document", "cannot hash prospective document"))?;
    Ok(*hasher.finalize().as_bytes())
}

/// The same whole-document authority, charging caller work without changing reconciliation.
/// Returns its decoded source models for setup metadata, avoiding another document decode.
pub(crate) fn preview_generation_checked(
    before: &ScenarioDocument,
    prospective: &ScenarioDocument,
    control: &OperationControl,
    checkpoint: &mut impl FnMut() -> Result<(), TemporalError>,
) -> Result<(GenerationPreview, WorkforceDomainV1, WorkforceDomainV1), TemporalError> {
    let mut checked = || {
        control.check().map_err(DomainPackError::from)?;
        checkpoint()
    };
    let checkpoint = &mut checked;
    checkpoint()?;
    let before_domain = validate_document_controlled(before, Some(control))?;
    let prospective_domain = validate_document_controlled(prospective, Some(control))?;
    let before_dates = planning_dates(&before.settings)?;
    let prospective_dates = planning_dates(&prospective.settings)?;
    let prospective_hash = checked_prospective_hash(before, prospective)?;
    checkpoint()?;
    let before_owners = generation::owners(&before_domain, &mut |_| checkpoint())?;
    let prospective_owners = generation::owners(&prospective_domain, &mut |_| checkpoint())?;
    check_identity_continuity(&before_owners, &prospective_owners, checkpoint)?;
    let candidates = generation::collect_specs(
        &prospective_domain,
        &prospective.settings,
        &prospective_owners,
        &mut |_| checkpoint(),
    )?;
    let reconciliation =
        build_reconciliation(before, prospective, &before_owners, &candidates, checkpoint)?;
    // Exercise exactly the ordinary atomic command that the caller will receive.
    let reconciled_domain = reconciliation
        .as_ref()
        .map(|batch| {
            let mutation = commands::apply_batch_controlled(prospective, batch, control)?;
            validate_document_controlled(&mutation.document, Some(control))
        })
        .transpose()?;
    let after_domain = reconciled_domain.as_ref().unwrap_or(&prospective_domain);
    let after_specs = if reconciled_domain.is_some() {
        generation::collect_specs(
            after_domain,
            &prospective.settings,
            &generation::owners(after_domain, &mut |_| checkpoint())?,
            &mut |_| checkpoint(),
        )?
    } else {
        candidates
    };
    let prior_specs =
        generation::collect_prior_specs(&before_domain, &before.settings, checkpoint)?;
    let mut prior_states = BTreeMap::new();
    for spec in prior_specs {
        checkpoint()?;
        let Some(id) = spec.id() else { continue };
        let state = match spec.resolve(&before.settings, before_dates) {
            Ok(shift) => PriorShift::Resolved(shift),
            Err(TemporalError::Issue(issue)) => PriorShift::Unresolved {
                id,
                origin: spec.origin(),
                issue: issue.kind,
            },
            Err(error) => return Err(error),
        };
        prior_states.insert(id, (state, spec));
    }
    // Metadata is compared once per template, never copied into each generated occurrence.
    let unchanged_templates = unchanged_templates(&before_domain, after_domain, checkpoint)?;
    let mut after = Vec::with_capacity(after_specs.len());
    let mut changes = Vec::new();
    let mut seen = BTreeSet::new();
    for spec in after_specs {
        checkpoint()?;
        let shift = spec.resolve(&prospective.settings, prospective_dates)?;
        let kind = match prior_states.get(&shift.id) {
            None => Some(ShiftChangeKind::Added),
            Some((PriorShift::Resolved(prior), prior_spec))
                if *prior == shift && same_metadata(*prior_spec, spec, &unchanged_templates) =>
            {
                None
            }
            Some(_) => Some(ShiftChangeKind::Changed),
        };
        if let Some(kind) = kind {
            push_change(&mut changes, shift.id, kind)?;
        }
        seen.insert(shift.id);
        after.push(shift);
    }
    for id in prior_states.keys().filter(|id| !seen.contains(id)) {
        checkpoint()?;
        push_change(&mut changes, *id, ShiftChangeKind::Removed)?;
    }
    after.sort_unstable_by_key(|shift| (shift.interval.starts_at.instant, shift.id));
    changes.sort_unstable_by_key(|change| change.id);
    checkpoint()?;
    let preview = GenerationPreview {
        prospective_hash,
        before: prior_states.into_values().map(|(state, _)| state).collect(),
        after,
        changes,
        reconciliation,
    };
    Ok((
        preview,
        before_domain,
        reconciled_domain.unwrap_or(prospective_domain),
    ))
}

fn check_identity_continuity(
    before_owners: &generation::Owners,
    prospective_owners: &generation::Owners,
    checkpoint: &mut impl FnMut() -> Result<(), TemporalError>,
) -> Result<(), TemporalError> {
    // Identity continuity includes dormant and detached owners, not just generated output.
    for ((template, date), owner) in prospective_owners {
        checkpoint()?;
        if before_owners
            .get(&(*template, *date))
            .is_some_and(|prior| prior.id != owner.id)
        {
            return Err(identity_issue(
                TemporalIssueKind::IdentityTransition,
                *template,
                *date,
            ));
        }
    }
    Ok(())
}

fn identity_issue(kind: TemporalIssueKind, template: ShiftTemplateId, date: Date) -> TemporalError {
    issue(kind, Some(template.as_entity_id()), Some(date))
}

fn build_reconciliation(
    before: &ScenarioDocument,
    prospective: &ScenarioDocument,
    before_owners: &generation::Owners,
    candidates: &[ShiftSpec<'_>],
    checkpoint: &mut impl FnMut() -> Result<(), TemporalError>,
) -> Result<Option<DomainBatchCommand>, TemporalError> {
    let mut occupied = collect_document_owned_uuids(before);
    let prospective_occupied = collect_document_owned_uuids(prospective);
    occupied.extend(&prospective_occupied);
    let raw_definitions = raw_definitions(before, checkpoint)?;
    let mut additions: BTreeMap<ShiftTemplateId, Map<String, Value>> = BTreeMap::new();
    for spec in candidates {
        checkpoint()?;
        let ShiftSpec::Generated { template, date, id } = *spec else {
            continue;
        };
        let prior = before_owners.get(&(template.id, date));
        if id.is_some() {
            continue;
        }
        let id = if let Some(prior) = prior {
            if prospective_occupied.contains(&prior.id.as_entity_id().as_uuid()) {
                return Err(identity_issue(
                    TemporalIssueKind::IdentityCollision,
                    template.id,
                    date,
                ));
            }
            prior.id
        } else {
            let derived = derive_id(template.id, date)?;
            if occupied.contains(&derived.as_entity_id().as_uuid()) {
                return Err(identity_issue(
                    TemporalIssueKind::IdentityCollision,
                    template.id,
                    date,
                ));
            }
            derived
        };
        // Reuse from the before document is the sole exemption from union collision checks.
        // Each missing (template,date) is unique; reserve every chosen ID for later candidates.
        occupied.insert(id.as_entity_id().as_uuid());
        let (key, value) = raw_definitions.get(&id).map_or_else(
            || (id.to_string(), json!({"id": id, "localStartDate": date})),
            |(key, value)| ((*key).clone(), (*value).clone()),
        );
        additions.entry(template.id).or_default().insert(key, value);
    }
    Ok(if additions.is_empty() {
        None
    } else {
        let templates: Vec<_> = additions.into_iter().map(|(template_id, identities)| json!({"templateId": template_id, "occurrenceIdentities": identities})).collect();
        Some(DomainBatchCommand {
            schema_version: DOMAIN_BATCH_SCHEMA_VERSION,
            pack_id: prospective.domain_pack.id.clone(),
            scenario_schema_version: prospective.domain_pack.schema_version,
            label: None,
            commands: vec![DomainCommandEnvelope {
                command_type: commands::ADD_OCCURRENCE_IDENTITIES.to_owned(),
                payload: json!({"templates": templates}),
            }],
        })
    })
}

/// Frozen occurrence-id-v1 byte contract. Only truly unseen occurrences use it.
fn derive_id(template: ShiftTemplateId, date: Date) -> Result<ShiftId, TemporalError> {
    let year = i32::from(date.year());
    let date_text = if year < 0 {
        format!(
            "-{:04}-{:02}-{:02}",
            year.unsigned_abs(),
            date.month(),
            date.day()
        )
    } else {
        format!("{year:04}-{:02}-{:02}", date.month(), date.day())
    };
    let length = u8::try_from(date_text.len())
        .map_err(|_| identity_issue(TemporalIssueKind::DateOverflow, template, date))?;
    let uuid = template.as_entity_id().as_uuid();
    let template_bytes = uuid.as_bytes();
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"eutheto/workforce/occurrence-id/v1\0");
    hasher.update(template_bytes);
    hasher.update(&[length]);
    hasher.update(date_text.as_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0; 16];
    bytes[..6].copy_from_slice(&template_bytes[..6]);
    bytes[6..].copy_from_slice(&digest.as_bytes()[..10]);
    bytes[6] = (bytes[6] & 0x0f) | 0x70;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    ShiftId::try_from(EntityId::from_uuid(uuid::Uuid::from_bytes(bytes)))
        .map_err(|_| invalid("occurrenceId", "invalid derived occurrence identity").into())
}

fn raw_definitions<'a>(
    document: &'a ScenarioDocument,
    checkpoint: &mut impl FnMut() -> Result<(), TemporalError>,
) -> Result<BTreeMap<ShiftId, (&'a String, &'a Value)>, TemporalError> {
    let mut definitions = BTreeMap::new();
    for entity in document.domain.entities.values() {
        checkpoint()?;
        if let Some(identities) = entity
            .get("occurrenceIdentities")
            .and_then(Value::as_object)
        {
            for (key, value) in identities {
                checkpoint()?;
                let id = key
                    .parse()
                    .map_err(|_| invalid("occurrenceIdentities", "invalid occurrence identity"))?;
                definitions.insert(id, (key, value));
            }
        }
    }
    Ok(definitions)
}

fn push_change(
    changes: &mut Vec<ShiftChange>,
    id: ShiftId,
    kind: ShiftChangeKind,
) -> Result<(), TemporalError> {
    if changes.len() == MAX_SHIFT_CHANGES {
        return Err(issue(TemporalIssueKind::OutputLimit, None, None));
    }
    changes.push(ShiftChange { id, kind });
    Ok(())
}

fn unchanged_templates(
    before: &WorkforceDomainV1,
    after: &WorkforceDomainV1,
    checkpoint: &mut impl FnMut() -> Result<(), TemporalError>,
) -> Result<BTreeSet<ShiftTemplateId>, TemporalError> {
    let mut unchanged = BTreeSet::new();
    for (id, entity) in &after.entities {
        checkpoint()?;
        if let (WorkforceEntity::ShiftTemplate(after), Some(WorkforceEntity::ShiftTemplate(before))) =
            (entity, before.entities.get(id))
            && template_metadata_eq(before, after)
        {
            unchanged.insert(after.id);
        }
    }
    Ok(unchanged)
}

fn same_metadata(
    before: ShiftSpec<'_>,
    after: ShiftSpec<'_>,
    unchanged: &BTreeSet<ShiftTemplateId>,
) -> bool {
    match (before, after) {
        (
            ShiftSpec::Generated {
                template: before, ..
            },
            ShiftSpec::Generated {
                template: after, ..
            },
        ) => before.id == after.id && unchanged.contains(&after.id),
        (ShiftSpec::Stored(before), ShiftSpec::Stored(after)) => {
            before.assignment_type_id == after.assignment_type_id
                && before.location_id == after.location_id
                && before.reporting_attribution == after.reporting_attribution
                && tags_eq(&before.tags, &after.tags)
                && coverage_eq(&before.coverage, &after.coverage)
        }
        _ => false,
    }
}

fn template_metadata_eq(before: &ShiftTemplate, after: &ShiftTemplate) -> bool {
    before.assignment_type_id == after.assignment_type_id
        && before.location_id == after.location_id
        && before.reporting_attribution == after.reporting_attribution
        && tags_eq(&before.tags, &after.tags)
        && coverage_eq(&before.coverage, &after.coverage)
}

fn tags_eq(before: &[String], after: &[String]) -> bool {
    if before == after {
        return true;
    }
    if before.len() != after.len() {
        return false;
    }
    let mut before: Vec<_> = before.iter().collect();
    let mut after: Vec<_> = after.iter().collect();
    before.sort_unstable();
    after.sort_unstable();
    before == after
}

fn coverage_eq(before: &Coverage, after: &Coverage) -> bool {
    if before == after {
        return true;
    }
    let mut before = before.clone();
    let mut after = after.clone();
    normalize_coverage(&mut before);
    normalize_coverage(&mut after);
    before == after
}

fn normalize_coverage(coverage: &mut Coverage) {
    let (Coverage::Exact {
        qualification_minimums,
        ..
    }
    | Coverage::AtLeast {
        qualification_minimums,
        ..
    }) = coverage;
    for minimum in &mut *qualification_minimums {
        minimum.qualifications.all_qualification_ids.sort_unstable();
        minimum.qualifications.any_qualification_ids.sort_unstable();
    }
    qualification_minimums.sort_unstable_by(|left, right| {
        left.minimum
            .cmp(&right.minimum)
            .then_with(|| {
                left.qualifications
                    .all_qualification_ids
                    .cmp(&right.qualifications.all_qualification_ids)
            })
            .then_with(|| {
                left.qualifications
                    .any_qualification_ids
                    .cmp(&right.qualifications.any_qualification_ids)
            })
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn occurrence_id_v1_matches_frozen_byte_vectors() -> Result<(), Box<dyn std::error::Error>> {
        // Expected bytes were computed from independently assembled fixed input hex.
        for (template, date, expected) in [
            (
                "018f7b40-a000-7000-8000-000000000006",
                Date::new(2026, 11, 1)?,
                "018f7b40-a000-7138-9a85-100eb0dc9bea",
            ),
            (
                "018f7b40-a000-7000-8000-000000000006",
                Date::new(0, 1, 1)?,
                "018f7b40-a000-72ad-80d7-12926953c03e",
            ),
            (
                "018f7b40-a000-7000-8000-000000000006",
                Date::new(-1, 1, 1)?,
                "018f7b40-a000-7df1-bda7-2189dccf3119",
            ),
            (
                "018f7b40-a001-7000-8000-000000000006",
                Date::new(2026, 11, 1)?,
                "018f7b40-a001-716a-b3c4-f8fd56f22ec1",
            ),
            (
                "018f7b40-a000-7000-8000-000000000006",
                Date::new(9999, 12, 31)?,
                "018f7b40-a000-747e-85f9-6ba370071cd0",
            ),
        ] {
            assert_eq!(derive_id(template.parse()?, date)?.to_string(), expected);
        }
        Ok(())
    }
}
