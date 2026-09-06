use super::{
    AssignmentConstructionIssue, AssignmentRuleError, AssignmentRuleLimit,
    budget::{OperationBudget, add, count, within},
    identity::{IdentityKind, PlanningIdentities},
    input::AssignmentInput,
};
use crate::{ids::ShiftId, model::AssignmentPair};
use eutheto_domain_ir::{
    DomainAssignmentId, DomainEntityId, DomainEntityKindId, DomainEntityRef, ScoreCategoryId,
};
use eutheto_planning_ir::{
    BoolVariable, ComparisonOp, Constraint, ConstraintRecord, InclusiveRange, IntDomain,
    IntVariable, IntVariableId, LinearComparison, LinearExpression, LinearTerm, Literal,
    ObjectiveTerm, ObjectiveTermId, ObjectiveTermKind, PlanningConstraintId, PlanningIrLimitsV1,
    ProjectionExpression, ProjectionId, ProvenanceId, ProvenanceParameter, ProvenanceRecord,
    ProvenanceSourceKind, SolutionProjection, Variable,
};
use eutheto_types::PersonId;
use std::collections::BTreeMap;

pub(super) const RANK_LEVEL: &str = "official.workforce.objective.assignment.rank";
pub(super) const RANK_CATEGORY: &str = "official.workforce.score.assignment.rank";
pub(super) const ASSIGNMENT_KIND: &str = "official.workforce.assignment";
pub(super) const WORKFORCE_PROJECTION_VERSION: u32 = 1;

/// Original source universe: pruning must never change a surviving pair's coefficient.
pub(super) struct RankIndex<'a> {
    people: &'a [PersonId],
    shifts: Vec<ShiftId>,
}

impl<'a> RankIndex<'a> {
    pub fn new(
        input: &'a AssignmentInput,
        budget: &mut OperationBudget<'_>,
    ) -> Result<Self, AssignmentRuleError> {
        let length = count(input.shifts.len())?;
        budget.reserve(0, length, length.checked_mul(16).ok_or_else(arithmetic)?)?;
        budget.steps(length)?;
        let mut shifts: Vec<_> = input.shifts.iter().map(|shift| shift.id).collect();
        budget.sort_work(shifts.len())?;
        shifts.sort_unstable();
        // AssignmentInput already sorts all source people, including inactive people.
        Ok(Self {
            people: &input.people,
            shifts,
        })
    }

    pub fn rank(
        &self,
        pair: AssignmentPair,
        limits: PlanningIrLimitsV1,
        budget: &mut OperationBudget<'_>,
    ) -> Result<i64, AssignmentRuleError> {
        let people = count(self.people.len())?;
        let shifts = count(self.shifts.len())?;
        budget.steps(
            u64::from(u64::BITS - people.leading_zeros())
                + u64::from(u64::BITS - shifts.leading_zeros()),
        )?;
        let person = count(
            self.people
                .binary_search(&pair.person_id)
                .map_err(|_| invalid())?,
        )?;
        let shift = count(
            self.shifts
                .binary_search(&pair.shift_id)
                .map_err(|_| invalid())?,
        )?;
        let rank = add(
            add(person.checked_mul(shifts).ok_or_else(arithmetic)?, shift)?,
            1,
        )?;
        let maximum = limits
            .max_abs_coefficient
            .min(PlanningIrLimitsV1::DEFAULT.max_abs_coefficient);
        within(
            rank,
            u64::try_from(maximum).map_err(|_| arithmetic())?,
            AssignmentRuleLimit::PerRecord,
        )?;
        i64::try_from(rank).map_err(|_| arithmetic())
    }
}

pub(super) struct RankIds {
    pub integer: IntVariableId,
    pub channels: [PlanningConstraintId; 2],
    pub term: ObjectiveTermId,
    pub projection: ProjectionId,
    pub fact: ProvenanceId,
}

impl RankIds {
    pub fn derive(
        pair: AssignmentPair,
        identities: &mut PlanningIdentities,
        budget: &mut OperationBudget<'_>,
    ) -> Result<Self, AssignmentRuleError> {
        let key = ("assignment_rank", pair.person_id, pair.shift_id);
        let integer = IntVariableId::new(identities.derive(IdentityKind::Integer, &key, budget)?)
            .map_err(|_| invalid())?;
        let term =
            ObjectiveTermId::new(identities.derive(IdentityKind::ObjectiveTerm, &key, budget)?)
                .map_err(|_| invalid())?;
        let fact = ProvenanceId::new(identities.derive(IdentityKind::Provenance, &key, budget)?)
            .map_err(|_| invalid())?;
        let channel =
            |selected, identities: &mut PlanningIdentities, budget: &mut OperationBudget<'_>| {
                PlanningConstraintId::new(identities.derive(
                    IdentityKind::Constraint,
                    &(
                        "assignment_rank_channel",
                        pair.person_id,
                        pair.shift_id,
                        selected,
                    ),
                    budget,
                )?)
                .map_err(|_| invalid())
            };
        let channels = [
            channel(true, identities, budget)?,
            channel(false, identities, budget)?,
        ];
        let projection = ProjectionId::new(identities.derive(
            IdentityKind::Projection,
            &("assignment", pair.person_id, pair.shift_id),
            budget,
        )?)
        .map_err(|_| invalid())?;
        Ok(Self {
            integer,
            channels,
            term,
            projection,
            fact,
        })
    }
}

pub(super) struct RankEmission {
    pub variable: Variable,
    pub channels: [ConstraintRecord; 2],
    pub term: ObjectiveTerm,
    pub projection: SolutionProjection,
    pub fact: ProvenanceRecord,
}

/// The caller precharges either the final IR or a bounded one-pair measurement temporary.
/// Single-term expressions and one-range domains are already canonical; do not allocate
/// intermediate maps/vectors just to normalize these known shapes.
pub(super) fn emit_rank(
    pair: AssignmentPair,
    rank: i64,
    decision: &BoolVariable,
    ids: RankIds,
    budget: &mut OperationBudget<'_>,
) -> Result<RankEmission, AssignmentRuleError> {
    budget.steps(16)?;
    let expression = |coefficient| LinearExpression {
        constant: 0,
        terms: vec![LinearTerm {
            variable: ids.integer.clone(),
            coefficient,
        }],
    };
    let [positive_id, negative_id] = ids.channels;
    let channel = |id, selected| ConstraintRecord {
        id,
        body: Constraint::LinearComparison(LinearComparison {
            expression: expression(1),
            op: ComparisonOp::Equal,
            rhs: i64::from(selected),
        }),
        enforcement: vec![Literal {
            variable: decision.id.clone(),
            positive: selected,
        }],
        provenance: ids.fact.clone(),
        tags: Vec::new(),
    };
    let channels = [channel(positive_id, true), channel(negative_id, false)];
    let term = ObjectiveTerm {
        id: ids.term,
        expression: expression(rank),
        kind: ObjectiveTermKind::Penalty,
        category: ScoreCategoryId::new(RANK_CATEGORY).map_err(|_| invalid())?,
        provenance: ids.fact.clone(),
    };
    let variable = Variable::Integer(IntVariable {
        id: ids.integer,
        domain: IntDomain {
            inclusive_ranges: vec![InclusiveRange { start: 0, end: 1 }],
        },
        provenance: ids.fact.clone(),
    });
    let pair_text = format!("{}.{}", pair.person_id, pair.shift_id);
    let projection = SolutionProjection {
        id: ids.projection,
        assignment_id: DomainAssignmentId::new(format!("{ASSIGNMENT_KIND}.{pair_text}"))
            .map_err(|_| invalid())?,
        entity: DomainEntityRef {
            kind: DomainEntityKindId::new(ASSIGNMENT_KIND).map_err(|_| invalid())?,
            id: DomainEntityId::new(pair_text).map_err(|_| invalid())?,
        },
        required: true,
        expression: ProjectionExpression::Boolean(decision.id.clone()),
        provenance: decision.provenance.clone(),
    };
    let fact = ProvenanceRecord {
        id: ids.fact,
        source_kind: ProvenanceSourceKind::Derived,
        source_id: RANK_LEVEL.to_owned(),
        entity_refs: Vec::new(),
        message_key: "official.workforce.assignment.rank".to_owned(),
        parameters: BTreeMap::from([("rank".to_owned(), ProvenanceParameter::Integer(rank))]),
        parent: Some(decision.provenance.clone()),
    };
    Ok(RankEmission {
        variable,
        channels,
        term,
        projection,
        fact,
    })
}

fn invalid() -> AssignmentRuleError {
    AssignmentRuleError::InvalidConstruction(AssignmentConstructionIssue::InvalidRecord)
}
fn arithmetic() -> AssignmentRuleError {
    AssignmentRuleError::InvalidConstruction(AssignmentConstructionIssue::ArithmeticOverflow)
}
