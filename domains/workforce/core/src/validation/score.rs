use super::common::{require, tags, text, token, unique, weight};
use crate::model::{
    DeviationPenalty, PeerGroup, PersonSelection, WorkforceScorePolicy, WorkloadPolicy,
    WorkloadTargetMode,
};
use eutheto_domain_api::DomainPackError;
use eutheto_domain_ir::MAX_SCORE_LEVELS;
use std::collections::BTreeSet;

/// Initial field bounds, independent of whole-document and solver resource budgets.
pub const MAX_POLICY_DEFINITIONS: usize = 256;
pub const MAX_PENALTY_BREAKPOINTS: usize = 32;
pub const MAX_WEIGHT: u32 = 1_000_000;
pub const MAX_REFERENCE_ITEMS: usize = 10_000;
pub const MAX_TOKEN_BYTES: usize = 64;
pub const MAX_DISPLAY_BYTES: usize = 256;

/// Validates policy-local shape, bounds and references between levels and priorities.
///
/// Person/bucket/calendar references and peer-target agreement require whole-document
/// validation. Missing policy, solver support and objective overflow are separate gates.
///
/// # Errors
///
/// Rejects malformed policy data without echoing submitted display text.
pub fn validate_score_policy_shape(policy: &WorkforceScorePolicy) -> Result<(), DomainPackError> {
    require(
        policy.id.as_uuid().get_version_num() == 7,
        "id",
        "expected UUIDv7 identity",
    )?;
    token(&policy.profile_key, "profileKey")?;
    require(
        !policy.levels.is_empty() && policy.levels.len() < MAX_SCORE_LEVELS,
        "levels",
        "ordered levels must leave capacity for the final assignment-rank level",
    )?;
    let mut levels = BTreeSet::new();
    for level in &policy.levels {
        token(&level.level_key, "levels.levelKey")?;
        text(&level.label, "levels.label")?;
        require(
            levels.insert(level.level_key.as_str()),
            "levels",
            "duplicate objective level",
        )?;
    }
    require(
        policy.priority_mapping.len() == 4,
        "priorityMapping",
        "every priority needs one mapping",
    )?;
    let mut priorities = BTreeSet::new();
    for mapping in &policy.priority_mapping {
        weight(mapping.scale, "priorityMapping.scale")?;
        require(
            levels.contains(mapping.level_key.as_str()),
            "priorityMapping.levelKey",
            "unknown objective level",
        )?;
        require(
            priorities.insert(mapping.priority),
            "priorityMapping",
            "duplicate priority mapping",
        )?;
    }
    require(
        policy.workload_policies.len() <= MAX_POLICY_DEFINITIONS,
        "workloadPolicies",
        "too many workload policies",
    )?;
    for (id, workload) in &policy.workload_policies {
        require(
            *id == workload.id,
            "workloadPolicies.id",
            "owned identity does not match its key",
        )?;
        validate_workload_policy_shape(workload)?;
    }
    Ok(())
}

fn validate_workload_policy_shape(policy: &WorkloadPolicy) -> Result<(), DomainPackError> {
    peer_group(&policy.peer_group)?;
    if let WorkloadTargetMode::Explicit { targets } = &policy.target_mode {
        require(
            targets.len() <= MAX_REFERENCE_ITEMS,
            "workloadPolicies.targets",
            "too many explicit targets",
        )?;
        let mut people = BTreeSet::new();
        require(
            targets.iter().all(|target| people.insert(target.person_id)),
            "workloadPolicies.targets",
            "targets must have unique person identities",
        )?;
    }
    if let DeviationPenalty::Piecewise { breakpoints } = &policy.penalty {
        require(
            !breakpoints.is_empty() && breakpoints.len() <= MAX_PENALTY_BREAKPOINTS,
            "workloadPolicies.penalty",
            "piecewise penalty needs a bounded nonempty segment list",
        )?;
        require(
            breakpoints[0].from_deviation == 0,
            "workloadPolicies.penalty",
            "first segment must start at zero",
        )?;
        require(
            breakpoints.iter().all(|point| point.slope <= MAX_WEIGHT),
            "workloadPolicies.penalty",
            "piecewise slope exceeds the weight bound",
        )?;
        require(
            breakpoints
                .windows(2)
                .all(|pair| pair[0].from_deviation < pair[1].from_deviation),
            "workloadPolicies.penalty",
            "segment starts must increase",
        )?;
    }
    Ok(())
}

fn peer_group(group: &PeerGroup) -> Result<(), DomainPackError> {
    match &group.people {
        PersonSelection::All {} => {}
        PersonSelection::Selected { person_ids } => {
            unique(person_ids, true, "workloadPolicies.peerGroup.personIds")?;
        }
        PersonSelection::Filter { all_tags, any_tags } => {
            require(
                !all_tags.is_empty() || !any_tags.is_empty(),
                "workloadPolicies.peerGroup",
                "empty filter must be explicit all",
            )?;
            tags(all_tags, "workloadPolicies.peerGroup.allTags")?;
            tags(any_tags, "workloadPolicies.peerGroup.anyTags")?;
        }
    }
    if let Some(teams) = &group.team_ids {
        unique(teams, true, "workloadPolicies.peerGroup.teamIds")?;
    }
    Ok(())
}
