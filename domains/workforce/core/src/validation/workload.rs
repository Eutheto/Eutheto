use super::{
    common::{Result, require},
    context::Context,
};
use crate::model::{
    PersonSelection, WorkforceEntity, WorkforceScorePolicy, WorkloadPolicy, WorkloadTargetMode,
};
use std::collections::BTreeSet;

impl Context<'_> {
    pub(super) fn score_references(&self, score: &WorkforceScorePolicy) -> Result {
        for policy in score.workload_policies.values() {
            self.checkpoint()?;
            self.workload_membership(policy.bucket_id, policy.calendar_id, policy.membership)?;
            self.people(&policy.peer_group.people, true)?;
            if let Some(ids) = &policy.peer_group.team_ids {
                for id in ids {
                    self.checkpoint()?;
                    self.team(*id)?;
                }
            }
            if let WorkloadTargetMode::Explicit { targets } = &policy.target_mode {
                for target in targets {
                    self.checkpoint()?;
                    self.person(target.person_id)?;
                }
            }
            if !matches!(
                policy.target_mode,
                WorkloadTargetMode::WeightedEqualShare {}
            ) {
                self.workload_targets(policy)?;
            }
        }
        Ok(())
    }

    fn workload_targets(&self, policy: &WorkloadPolicy) -> Result {
        // Index each policy filter once. Avoid quadratic tag/identity scans for each person.
        let (selected, all_tags, any_tags) = match &policy.peer_group.people {
            PersonSelection::All {} => (BTreeSet::new(), BTreeSet::new(), BTreeSet::new()),
            PersonSelection::Selected { person_ids } => (
                person_ids.iter().copied().collect(),
                BTreeSet::new(),
                BTreeSet::new(),
            ),
            PersonSelection::Filter { all_tags, any_tags } => (
                BTreeSet::new(),
                all_tags.iter().map(String::as_str).collect(),
                any_tags.iter().map(String::as_str).collect(),
            ),
        };
        let teams: BTreeSet<_> = policy
            .peer_group
            .team_ids
            .iter()
            .flatten()
            .copied()
            .collect();
        let explicit_targets: BTreeSet<_> = match &policy.target_mode {
            WorkloadTargetMode::Explicit { targets } => {
                targets.iter().map(|target| target.person_id).collect()
            }
            _ => BTreeSet::new(),
        };
        let mut peers = 0;
        for entity in self.domain.entities.values() {
            self.checkpoint()?;
            let WorkforceEntity::Person(person) = entity else {
                continue;
            };
            let matches_people = match &policy.peer_group.people {
                PersonSelection::All {} => true,
                PersonSelection::Selected { .. } => selected.contains(&person.id),
                PersonSelection::Filter { .. } => {
                    self.matches_tags(&person.tags, &all_tags, &any_tags)?
                }
            };
            if !matches_people {
                continue;
            }
            if policy.peer_group.team_ids.is_some() {
                let mut selected_team = false;
                for id in &person.team_ids {
                    self.checkpoint()?;
                    if teams.contains(id) {
                        selected_team = true;
                        break;
                    }
                }
                if !selected_team {
                    continue;
                }
            }
            peers += 1;
            match &policy.target_mode {
                WorkloadTargetMode::Explicit { .. } => require(
                    explicit_targets.contains(&person.id),
                    "workloadPolicies.targets",
                    "explicit targets must cover every peer exactly",
                )?,
                WorkloadTargetMode::PersonTargets {} => require(
                    person.workload_target.is_some_and(|target| {
                        target.bucket_id == policy.bucket_id
                            && target.calendar_id == policy.calendar_id
                            && target.membership == policy.membership
                    }),
                    "workloadPolicies.targetMode",
                    "every peer needs a matching person workload target",
                )?,
                WorkloadTargetMode::WeightedEqualShare {} => {}
            }
        }
        if let WorkloadTargetMode::Explicit { targets } = &policy.target_mode {
            require(
                peers == targets.len(),
                "workloadPolicies.targets",
                "explicit targets include a person outside the peer group",
            )?;
        }
        Ok(())
    }

    fn matches_tags(
        &self,
        tags: &[String],
        all_tags: &BTreeSet<&str>,
        any_tags: &BTreeSet<&str>,
    ) -> Result<bool> {
        if all_tags.is_empty() && any_tags.is_empty() {
            return Ok(true);
        }
        let mut matched_all = 0;
        let mut matched_any = any_tags.is_empty();
        for tag in tags {
            self.checkpoint()?;
            matched_all += usize::from(all_tags.contains(tag.as_str()));
            matched_any = matched_any || any_tags.contains(tag.as_str());
        }
        Ok(matched_all == all_tags.len() && matched_any)
    }
}
