use super::{
    AssignmentRuleError, AssignmentRuleLimit, baseline,
    budget::{OperationBudget, count, within},
    identity::{IdentityKind, PREFERENCE_LEVEL_PREFIX, PlanningIdentities},
};
use crate::{
    generated_workforce_pack_contract::WORKFORCE_PACK_ID,
    model::{AssignmentPair, LockState, WorkforceDomainV1, WorkforceScorePolicy},
};
use eutheto_domain_api::DomainPackError;
use eutheto_domain_ir::{
    AUTHORED_OBJECTIVE_PROFILE_ID, DomainContractError, RunObjectiveContext, RunObjectiveOperation,
    ScoreCategoryId,
};
use eutheto_planning_ir::{
    IntVariableId, LinearExpression, LinearTerm, ObjectiveTerm, ObjectiveTermId, ObjectiveTermKind,
    PlanningIrLimitsV1, ProvenanceId, ProvenanceParameter, ProvenanceRecord, ProvenanceSourceKind,
};
use eutheto_types::{AssignmentId, ScenarioId};
use std::{
    collections::{BTreeMap, BTreeSet},
    mem::size_of,
};

pub(super) const CHANGE_LEVEL: &str = "official.workforce.objective.pair_change";
pub(super) const CHANGE_CATEGORY: &str = "official.workforce.score.pair_change";
pub(super) const SOFT_LEVEL: &str = "official.workforce.objective.soft_lock";
pub(super) const SOFT_CATEGORY: &str = "official.workforce.score.soft_lock";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ProfileKind {
    Authored,
    Balanced,
    PreferenceFocused,
    FairnessFirst,
    MinimalChanges,
}

impl ProfileKind {
    pub fn parse(
        context: Option<&RunObjectiveContext>,
        scenario_id: ScenarioId,
    ) -> Result<Option<Self>, DomainPackError> {
        let Some(context) = context else {
            return Ok(None);
        };
        if context.pack_id.as_str() != WORKFORCE_PACK_ID {
            return Err(invalid_context("invalid objective pack"));
        }
        context
            .validate_for(&context.pack_id, scenario_id)
            .map_err(map_context_error)?;
        let profile = match context.profile_id.as_str() {
            AUTHORED_OBJECTIVE_PROFILE_ID => Self::Authored,
            "official.workforce.objective.balanced" => Self::Balanced,
            "official.workforce.objective.preference-focused" => Self::PreferenceFocused,
            "official.workforce.objective.fairness-first" => Self::FairnessFirst,
            "official.workforce.objective.minimal-changes" => Self::MinimalChanges,
            _ => return Err(invalid_context("unsupported objective profile")),
        };
        if profile == Self::Authored
            && (context.operation != RunObjectiveOperation::Optimize || context.has_baseline())
        {
            return Err(invalid_context("unsupported objective operation"));
        }
        if profile == Self::MinimalChanges && !context.has_baseline() {
            return Err(invalid_context("minimal-changes requires a baseline"));
        }
        Ok(Some(profile))
    }

    pub const fn named(self) -> bool {
        !matches!(self, Self::Authored)
    }

    pub const fn changes(self, operation: RunObjectiveOperation) -> bool {
        matches!(operation, RunObjectiveOperation::Repair) || matches!(self, Self::MinimalChanges)
    }

    pub const fn id(self) -> &'static str {
        match self {
            Self::Authored => AUTHORED_OBJECTIVE_PROFILE_ID,
            Self::Balanced => "official.workforce.objective.balanced",
            Self::PreferenceFocused => "official.workforce.objective.preference-focused",
            Self::FairnessFirst => "official.workforce.objective.fairness-first",
            Self::MinimalChanges => "official.workforce.objective.minimal-changes",
        }
    }

    pub const fn splits_workload(self) -> bool {
        matches!(self, Self::PreferenceFocused | Self::FairnessFirst)
    }
}

pub(super) fn supports_soft(context: Option<&RunObjectiveContext>) -> bool {
    context.is_some_and(|context| {
        matches!(
            context.profile_id.as_str(),
            "official.workforce.objective.balanced"
                | "official.workforce.objective.preference-focused"
                | "official.workforce.objective.fairness-first"
                | "official.workforce.objective.minimal-changes"
        )
    })
}

#[allow(clippy::needless_pass_by_value)] // map_err requires an owned error callback.
fn map_context_error(error: DomainContractError) -> DomainPackError {
    match error {
        DomainContractError::UnsupportedVersion(version) => {
            DomainPackError::UnsupportedVersion(version)
        }
        _ => invalid_context("invalid objective context"),
    }
}

fn invalid_context(message: &'static str) -> DomainPackError {
    DomainPackError::InvalidPayload {
        path: "objectiveContext".to_owned(),
        message: message.to_owned(),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PrefixTier {
    PairChange,
    SoftLocks,
}

impl PrefixTier {
    pub const fn id(self) -> &'static str {
        match self {
            Self::PairChange => CHANGE_LEVEL,
            Self::SoftLocks => SOFT_LEVEL,
        }
    }

    pub const fn category(self) -> &'static str {
        match self {
            Self::PairChange => CHANGE_CATEGORY,
            Self::SoftLocks => SOFT_CATEGORY,
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct PreferenceLevel {
    pub key: String,
    pub id: String,
}

#[derive(Clone, Debug)]
pub(super) struct Layout {
    pub kind: Option<ProfileKind>,
    pub prefix: Vec<PrefixTier>,
    pub levels: Vec<PreferenceLevel>,
    slots: Vec<[Option<usize>; 2]>,
}

impl Layout {
    pub fn empty(kind: Option<ProfileKind>) -> Self {
        Self {
            kind,
            prefix: Vec::new(),
            levels: Vec::new(),
            slots: Vec::new(),
        }
    }

    pub fn without_policy(
        kind: ProfileKind,
        operation: RunObjectiveOperation,
        limits: PlanningIrLimitsV1,
        budget: &mut OperationBudget<'_>,
    ) -> Result<Self, AssignmentRuleError> {
        let changes = kind.changes(operation);
        let prefix_len: usize = if changes { 2 } else { 1 };
        let total = prefix_len.checked_add(1).ok_or_else(arithmetic)?;
        within(
            count(total)?,
            limits.max_objective_levels,
            AssignmentRuleLimit::PerRecord,
        )?;
        budget.reserve(
            0,
            count(prefix_len)?,
            count(
                prefix_len
                    .checked_mul(size_of::<PrefixTier>())
                    .ok_or_else(arithmetic)?,
            )?,
        )?;
        let mut prefix = Vec::with_capacity(prefix_len);
        if changes {
            prefix.push(PrefixTier::PairChange);
        }
        prefix.push(PrefixTier::SoftLocks);
        Ok(Self {
            kind: Some(kind),
            prefix,
            levels: Vec::new(),
            slots: Vec::new(),
        })
    }

    #[allow(clippy::too_many_lines)]
    pub fn new(
        policy: &WorkforceScorePolicy,
        context: Option<&RunObjectiveContext>,
        scenario_id: ScenarioId,
        active_preferences: bool,
        limits: PlanningIrLimitsV1,
        budget: &mut OperationBudget<'_>,
    ) -> Result<Self, AssignmentRuleError> {
        let kind = ProfileKind::parse(context, scenario_id)
            .map_err(AssignmentRuleError::InvalidDocument)?;
        let Some(kind) = kind.filter(|kind| kind.named()) else {
            if !active_preferences {
                return Ok(Self {
                    kind,
                    prefix: Vec::new(),
                    levels: Vec::new(),
                    slots: vec![[None, None]; policy.levels.len()],
                });
            }
            let levels = policy
                .levels
                .iter()
                .map(|level| PreferenceLevel {
                    key: level.level_key.clone(),
                    id: format!("{PREFERENCE_LEVEL_PREFIX}{}", level.level_key),
                })
                .collect::<Vec<_>>();
            let slots = (0..levels.len())
                .map(|index| [Some(index), Some(index)])
                .collect();
            return Ok(Self {
                kind,
                prefix: Vec::new(),
                levels,
                slots,
            });
        };

        let changes = context.is_some_and(|context| kind.changes(context.operation));
        let mut prefix = Vec::with_capacity(2);
        if changes {
            prefix.push(PrefixTier::PairChange);
        }
        prefix.push(PrefixTier::SoftLocks);

        let mapped = policy
            .priority_mapping
            .iter()
            .map(|mapping| mapping.level_key.as_str())
            .collect::<BTreeSet<_>>();
        let mut mapped_levels = Vec::with_capacity(mapped.len());
        for (authored_index, source) in policy.levels.iter().enumerate() {
            budget.step()?;
            if mapped.contains(source.level_key.as_str()) {
                mapped_levels.push((authored_index, source));
            }
        }
        let mut levels =
            Vec::with_capacity(mapped_levels.len() * if kind.splits_workload() { 2 } else { 1 });
        let mut slots = vec![[None, None]; policy.levels.len()];
        if kind.splits_workload() {
            let partitions = if kind == ProfileKind::FairnessFirst {
                [true, false]
            } else {
                [false, true]
            };
            for workload in partitions {
                for (authored_index, source) in &mapped_levels {
                    let index = levels.len();
                    let partition = if workload {
                        "workload_balance"
                    } else {
                        "non_workload"
                    };
                    let digest = blake3::hash(source.level_key.as_bytes()).to_hex();
                    let id = format!(
                        "official.workforce.objective.preference.{}.{}.{}",
                        if kind == ProfileKind::FairnessFirst {
                            "fairness_first"
                        } else {
                            "preference_focused"
                        },
                        partition,
                        digest
                    );
                    let key = format!("{}.{}", source.level_key, partition);
                    budget.reserve(0, 1, count(id.len() + key.len())?)?;
                    levels.push(PreferenceLevel { key, id });
                    slots[*authored_index][usize::from(workload)] = Some(index);
                }
            }
        } else {
            for (authored_index, source) in mapped_levels {
                let index = levels.len();
                let id = format!("{PREFERENCE_LEVEL_PREFIX}{}", source.level_key);
                budget.reserve(0, 1, count(id.len() + source.level_key.len())?)?;
                levels.push(PreferenceLevel {
                    key: source.level_key.clone(),
                    id,
                });
                slots[authored_index] = [Some(index), Some(index)];
            }
        }
        let total = prefix
            .len()
            .checked_add(levels.len())
            .and_then(|value| value.checked_add(1))
            .ok_or_else(arithmetic)?;
        within(
            count(total)?,
            limits.max_objective_levels,
            AssignmentRuleLimit::PerRecord,
        )?;
        Ok(Self {
            kind: Some(kind),
            prefix,
            levels,
            slots,
        })
    }

    pub fn preference_level(&self, authored_index: usize, workload: bool) -> Option<usize> {
        self.slots
            .get(authored_index)
            .and_then(|slots| slots[usize::from(workload)])
    }

    pub const fn changes(&self, context: Option<&RunObjectiveContext>) -> bool {
        match (self.kind, context) {
            (Some(kind), Some(context)) => kind.changes(context.operation),
            _ => false,
        }
    }
}

pub(super) fn snapshot_pairs<'a>(
    input: &'a super::input::AssignmentInput,
    scenario_id: ScenarioId,
    context: Option<&RunObjectiveContext>,
    budget: &mut OperationBudget<'_>,
) -> Result<Option<&'a [AssignmentPair]>, DomainPackError> {
    baseline::selected_baseline_pairs(
        &input.domain,
        input.captured_original.as_ref(),
        scenario_id,
        context,
        budget,
    )
}
pub(super) enum SortedPairs<'a> {
    Slice(&'a [AssignmentPair]),
    References(Vec<&'a AssignmentPair>),
}

impl SortedPairs<'_> {
    pub(super) fn len(&self) -> usize {
        match self {
            Self::Slice(pairs) => pairs.len(),
            Self::References(pairs) => pairs.len(),
        }
    }

    pub(super) fn pair(&self, index: usize) -> &AssignmentPair {
        match self {
            Self::Slice(pairs) => &pairs[index],
            Self::References(pairs) => pairs[index],
        }
    }

    pub(super) fn contains(
        &self,
        pair: AssignmentPair,
        budget: &mut OperationBudget<'_>,
    ) -> Result<bool, AssignmentRuleError> {
        budget.steps(u64::from(usize::BITS - self.len().leading_zeros()))?;
        Ok(match self {
            Self::Slice(pairs) => pairs.binary_search(&pair).is_ok(),
            Self::References(pairs) => pairs
                .binary_search_by(|candidate| (**candidate).cmp(&pair))
                .is_ok(),
        })
    }
}

pub(super) fn sorted_baseline_pairs<'a>(
    input: &'a super::input::AssignmentInput,
    scenario_id: ScenarioId,
    context: Option<&RunObjectiveContext>,
    budget: &mut OperationBudget<'_>,
) -> Result<Option<SortedPairs<'a>>, DomainPackError> {
    snapshot_pairs(input, scenario_id, context, budget)?
        .map(|pairs| sort_pairs(pairs, budget))
        .transpose()
}
pub(super) fn sort_pairs<'a>(
    pairs: &'a [AssignmentPair],
    budget: &mut OperationBudget<'_>,
) -> Result<SortedPairs<'a>, DomainPackError> {
    let mut ordered = true;
    for pair_window in pairs.windows(2) {
        budget
            .step()
            .map_err(|error| super::authority::operation_error(&error))?;
        if pair_window[0] > pair_window[1] {
            ordered = false;
            break;
        }
    }
    if ordered {
        return Ok(SortedPairs::Slice(pairs));
    }
    budget
        .reserve(
            0,
            count(pairs.len()).map_err(|error| super::authority::operation_error(&error))?,
            count(
                pairs
                    .len()
                    .checked_mul(size_of::<&AssignmentPair>())
                    .ok_or(DomainPackError::ResourceLimitExceeded)?,
            )
            .map_err(|error| super::authority::operation_error(&error))?,
        )
        .map_err(|error| super::authority::operation_error(&error))?;
    let mut sorted = pairs.iter().collect::<Vec<_>>();
    budget
        .sort_work(sorted.len())
        .map_err(|error| super::authority::operation_error(&error))?;
    sorted.sort_unstable();
    Ok(SortedPairs::References(sorted))
}

pub(super) fn prefix_values(
    layout: &Layout,
    domain: &WorkforceDomainV1,
    selected: &[AssignmentPair],
    baseline: Option<&SortedPairs<'_>>,
    budget: &mut OperationBudget<'_>,
) -> Result<Vec<i64>, DomainPackError> {
    let mut values = Vec::with_capacity(layout.prefix.len());
    for tier in &layout.prefix {
        let value = match tier {
            PrefixTier::PairChange => {
                let baseline =
                    baseline.ok_or_else(|| super::authority::contract("baseline_required"))?;
                symmetric_difference_size(baseline, selected, budget)?
            }
            PrefixTier::SoftLocks => soft_lock_cost(domain, selected, budget)?,
        };
        values.push(value);
    }
    Ok(values)
}

fn symmetric_difference_size(
    baseline: &SortedPairs<'_>,
    selected: &[AssignmentPair],
    budget: &mut OperationBudget<'_>,
) -> Result<i64, DomainPackError> {
    let (mut base_index, mut selected_index, mut changed) = (0, 0, 0_i64);
    while base_index < baseline.len() && selected_index < selected.len() {
        budget
            .step()
            .map_err(|error| super::authority::operation_error(&error))?;
        match (*baseline.pair(base_index)).cmp(&selected[selected_index]) {
            std::cmp::Ordering::Less => {
                changed = changed
                    .checked_add(1)
                    .ok_or_else(|| super::authority::contract("arithmetic"))?;
                base_index += 1;
            }
            std::cmp::Ordering::Greater => {
                changed = changed
                    .checked_add(1)
                    .ok_or_else(|| super::authority::contract("arithmetic"))?;
                selected_index += 1;
            }
            std::cmp::Ordering::Equal => {
                base_index += 1;
                selected_index += 1;
            }
        }
    }
    changed = changed
        .checked_add(
            i64::try_from(baseline.len() - base_index)
                .map_err(|_| super::authority::contract("arithmetic"))?,
        )
        .and_then(|value| value.checked_add(i64::try_from(selected.len() - selected_index).ok()?))
        .ok_or_else(|| super::authority::contract("arithmetic"))?;
    Ok(changed)
}

pub(super) fn soft_lock_cost(
    domain: &WorkforceDomainV1,
    selected: &[AssignmentPair],
    budget: &mut OperationBudget<'_>,
) -> Result<i64, DomainPackError> {
    let mut cost = 0_i64;
    for lock in domain.locked_assignments.values() {
        budget
            .step()
            .map_err(|error| super::authority::operation_error(&error))?;
        let LockState::Soft { stability_weight } = lock.state else {
            continue;
        };
        let pair = AssignmentPair {
            person_id: lock.person_id,
            shift_id: lock.shift_id,
        };
        if selected.binary_search(&pair).is_err() {
            cost = cost
                .checked_add(i64::from(stability_weight))
                .ok_or_else(|| super::authority::contract("arithmetic"))?;
        }
    }
    if cost > PlanningIrLimitsV1::DEFAULT.max_abs_value {
        return Err(DomainPackError::ResourceLimitExceeded);
    }
    Ok(cost)
}

fn arithmetic() -> AssignmentRuleError {
    AssignmentRuleError::InvalidConstruction(super::AssignmentConstructionIssue::ArithmeticOverflow)
}

#[derive(Clone, Copy, Debug)]
pub(super) enum TermSource {
    PairChange { baseline_member: bool },
    SoftLock { lock_id: AssignmentId, weight: u32 },
}

#[derive(Clone, Copy, Debug)]
pub(super) struct TermShape {
    pub tier: PrefixTier,
    pub pair: AssignmentPair,
    pub candidate_index: Option<usize>,
    pub source: TermSource,
}

#[derive(Clone, Debug)]
pub(super) struct TierShape {
    pub tier: PrefixTier,
    pub lower_bound: i64,
    pub upper_bound: i64,
    pub terms: Vec<TermShape>,
}

#[derive(Clone, Debug)]
pub(super) struct Shape {
    pub tiers: Vec<TierShape>,
    pub term_count: u64,
}

pub(super) fn shape(
    layout: &Layout,
    domain: &WorkforceDomainV1,
    candidates: &[AssignmentPair],
    baseline: Option<&SortedPairs<'_>>,
    limits: PlanningIrLimitsV1,
    budget: &mut OperationBudget<'_>,
) -> Result<Shape, AssignmentRuleError> {
    let mut tiers = Vec::with_capacity(layout.prefix.len());
    let mut term_count = 0_u64;
    for tier in &layout.prefix {
        let mut terms = Vec::new();
        let (lower_bound, upper_bound) = match tier {
            PrefixTier::PairChange => {
                let baseline = baseline.ok_or_else(arithmetic)?;
                let mut unavoidable = 0_i64;
                let mut possible = i64::try_from(baseline.len()).map_err(|_| arithmetic())?;
                for (candidate_index, pair) in candidates.iter().copied().enumerate() {
                    budget.step()?;
                    let baseline_member = baseline.contains(pair, budget)?;
                    if !baseline_member {
                        possible = possible.checked_add(1).ok_or_else(arithmetic)?;
                    }
                    budget.reserve(0, 1, size_of::<TermShape>() as u64)?;
                    terms.push(TermShape {
                        tier: *tier,
                        pair,
                        candidate_index: Some(candidate_index),
                        source: TermSource::PairChange { baseline_member },
                    });
                }
                for index in 0..baseline.len() {
                    budget.step()?;
                    let pair = *baseline.pair(index);
                    if candidate_index(candidates, pair, budget)?.is_some() {
                        continue;
                    }
                    unavoidable = unavoidable.checked_add(1).ok_or_else(arithmetic)?;
                    budget.reserve(0, 1, size_of::<TermShape>() as u64)?;
                    terms.push(TermShape {
                        tier: *tier,
                        pair,
                        candidate_index: None,
                        source: TermSource::PairChange {
                            baseline_member: true,
                        },
                    });
                }
                (unavoidable, possible)
            }
            PrefixTier::SoftLocks => {
                let (mut fixed, mut total) = (0_i64, 0_i64);
                for lock in domain.locked_assignments.values() {
                    budget.step()?;
                    let LockState::Soft { stability_weight } = lock.state else {
                        continue;
                    };
                    if stability_weight == 0 {
                        continue;
                    }
                    let pair = AssignmentPair {
                        person_id: lock.person_id,
                        shift_id: lock.shift_id,
                    };
                    let candidate = candidate_index(candidates, pair, budget)?;
                    let weight = i64::from(stability_weight);
                    total = total.checked_add(weight).ok_or_else(arithmetic)?;
                    if candidate.is_none() {
                        fixed = fixed.checked_add(weight).ok_or_else(arithmetic)?;
                    }
                    budget.reserve(0, 1, size_of::<TermShape>() as u64)?;
                    terms.push(TermShape {
                        tier: *tier,
                        pair,
                        candidate_index: candidate,
                        source: TermSource::SoftLock {
                            lock_id: lock.id,
                            weight: stability_weight,
                        },
                    });
                }
                (fixed, total)
            }
        };
        within(
            u64::try_from(upper_bound).map_err(|_| arithmetic())?,
            u64::try_from(limits.max_abs_value).map_err(|_| arithmetic())?,
            AssignmentRuleLimit::PerRecord,
        )?;
        within(
            u64::try_from(lower_bound).map_err(|_| arithmetic())?,
            u64::try_from(limits.max_abs_value).map_err(|_| arithmetic())?,
            AssignmentRuleLimit::PerRecord,
        )?;
        term_count = term_count
            .checked_add(count(terms.len())?)
            .ok_or_else(arithmetic)?;
        tiers.push(TierShape {
            tier: *tier,
            lower_bound,
            upper_bound,
            terms,
        });
    }
    Ok(Shape { tiers, term_count })
}

pub(super) fn emit_term(
    shape: TermShape,
    integer: Option<IntVariableId>,
    parent: ProvenanceId,
    identities: &mut PlanningIdentities,
    budget: &mut OperationBudget<'_>,
) -> Result<(ObjectiveTerm, ProvenanceRecord), AssignmentRuleError> {
    if shape.candidate_index.is_some() != integer.is_some() {
        return Err(arithmetic());
    }
    let (term_id, fact_id) = term_ids(shape.source, shape.pair, identities, budget)?;
    let (constant, coefficient) = match shape.source {
        TermSource::PairChange {
            baseline_member: true,
        } => (1, -1),
        TermSource::PairChange {
            baseline_member: false,
        } => (0, 1),
        TermSource::SoftLock { weight, .. } => (i64::from(weight), -i64::from(weight)),
    };
    let expression = LinearExpression {
        constant,
        terms: integer
            .map(|variable| {
                vec![LinearTerm {
                    variable,
                    coefficient,
                }]
            })
            .unwrap_or_default(),
    };
    let (level_id, category) = (shape.tier.id(), shape.tier.category());
    let message_key = match shape.tier {
        PrefixTier::PairChange => {
            if !matches!(shape.source, TermSource::PairChange { .. }) {
                return Err(arithmetic());
            }
            "official.workforce.objective.pair_change.contribution"
        }
        PrefixTier::SoftLocks => {
            if !matches!(shape.source, TermSource::SoftLock { .. }) {
                return Err(arithmetic());
            }
            "official.workforce.objective.soft_lock.contribution"
        }
    };
    let fact = ProvenanceRecord {
        id: fact_id.clone(),
        source_kind: ProvenanceSourceKind::Derived,
        source_id: level_id.to_owned(),
        entity_refs: Vec::new(),
        message_key: message_key.to_owned(),
        parameters: BTreeMap::new(),
        parent: Some(parent),
    };
    let term = ObjectiveTerm {
        id: term_id,
        expression,
        kind: ObjectiveTermKind::Penalty,
        category: ScoreCategoryId::new(category).map_err(|_| arithmetic())?,
        provenance: fact.id.clone(),
    };
    budget.steps(4)?;
    Ok((term, fact))
}

pub(super) fn prefix_level_fact(
    kind: ProfileKind,
    tier: PrefixTier,
    identities: &mut PlanningIdentities,
    budget: &mut OperationBudget<'_>,
) -> Result<ProvenanceRecord, AssignmentRuleError> {
    let level_id = tier.id();
    let id = ProvenanceId::new(identities.derive(
        IdentityKind::Provenance,
        &("objective-profile-level", kind.id(), level_id),
        budget,
    )?)
    .map_err(|_| arithmetic())?;
    Ok(ProvenanceRecord {
        id,
        source_kind: ProvenanceSourceKind::Fact,
        source_id: kind.id().to_owned(),
        entity_refs: Vec::new(),
        message_key: "official.workforce.objective_profile.level".to_owned(),
        parameters: BTreeMap::from([(
            "level".to_owned(),
            ProvenanceParameter::Text(level_id.to_owned()),
        )]),
        parent: None,
    })
}

pub(super) fn pair_change_fact_id(
    pair: AssignmentPair,
    identities: &mut PlanningIdentities,
    budget: &mut OperationBudget<'_>,
) -> Result<ProvenanceId, AssignmentRuleError> {
    term_ids(
        TermSource::PairChange {
            baseline_member: false,
        },
        pair,
        identities,
        budget,
    )
    .map(|(_, fact)| fact)
}

pub(super) fn soft_lock_fact_id(
    lock_id: AssignmentId,
    pair: AssignmentPair,
    weight: u32,
    identities: &mut PlanningIdentities,
    budget: &mut OperationBudget<'_>,
) -> Result<ProvenanceId, AssignmentRuleError> {
    term_ids(
        TermSource::SoftLock { lock_id, weight },
        pair,
        identities,
        budget,
    )
    .map(|(_, fact)| fact)
}

fn term_ids(
    source: TermSource,
    pair: AssignmentPair,
    identities: &mut PlanningIdentities,
    budget: &mut OperationBudget<'_>,
) -> Result<(ObjectiveTermId, ProvenanceId), AssignmentRuleError> {
    let (objective_key, fact_key) = match source {
        TermSource::PairChange { .. } => {
            let key = ("pair-change", pair);
            (
                identities.derive(IdentityKind::ObjectiveTerm, &key, budget)?,
                identities.derive(IdentityKind::Provenance, &key, budget)?,
            )
        }
        TermSource::SoftLock { lock_id, .. } => {
            let key = ("soft-lock", lock_id);
            (
                identities.derive(IdentityKind::ObjectiveTerm, &key, budget)?,
                identities.derive(IdentityKind::Provenance, &key, budget)?,
            )
        }
    };
    Ok((
        ObjectiveTermId::new(objective_key).map_err(|_| arithmetic())?,
        ProvenanceId::new(fact_key).map_err(|_| arithmetic())?,
    ))
}

fn candidate_index(
    candidates: &[AssignmentPair],
    pair: AssignmentPair,
    budget: &mut OperationBudget<'_>,
) -> Result<Option<usize>, AssignmentRuleError> {
    let steps = u64::from(usize::BITS - candidates.len().leading_zeros());
    budget.steps(steps)?;
    Ok(candidates.binary_search(&pair).ok())
}
