use eutheto_domain_ir::ConflictGroupV1;
use eutheto_planning_ir::{
    BudgetedValidationError, Literal, PlanningIrLimitsV1, PlanningPreparationBudget,
    PlanningPreparationError, PlanningProblem, ValidationError, validate, validate_with_budget,
};
use eutheto_types::RuleId;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::mem::size_of;

/// A strict failure while mapping backend-returned assumption literals.
#[derive(Debug)]
pub enum AssumptionMappingError {
    /// The planning problem is not valid canonical IR.
    InvalidProblem(ValidationError),
    /// Cooperative preparation admission/control failed.
    Preparation(PlanningPreparationError),
    /// A conflict must contain at least one literal.
    Empty,
    /// The returned set exceeds the caller's assumption bound.
    LimitExceeded,
    /// The backend returned the same literal more than once.
    DuplicateLiteral(Literal),
    /// No assumption uses the returned Boolean variable.
    UnknownLiteral(Literal),
    /// An assumption uses the variable, but with the opposite polarity.
    WrongPolarity(Literal),
}

impl fmt::Display for AssumptionMappingError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidProblem(error) => write!(formatter, "invalid planning problem: {error}"),
            Self::Preparation(error) => write!(formatter, "mapping preparation failed: {error}"),
            Self::Empty => formatter.write_str("assumption conflict is empty"),
            Self::LimitExceeded => formatter.write_str("assumption conflict exceeds its limit"),
            Self::DuplicateLiteral(_) => {
                formatter.write_str("assumption conflict contains a duplicate literal")
            }
            Self::UnknownLiteral(_) => {
                formatter.write_str("assumption conflict contains an unknown literal")
            }
            Self::WrongPolarity(_) => formatter
                .write_str("assumption conflict contains a literal with the wrong polarity"),
        }
    }
}

impl std::error::Error for AssumptionMappingError {}

/// Owned mapped groups whose transient admission remains live until explicit release.
#[must_use]
pub struct MappedAssumptionGroups {
    groups: Vec<ConflictGroupV1>,
    transient_bytes: u64,
}

impl MappedAssumptionGroups {
    /// Returns the mapped groups while this reservation remains live.
    #[must_use]
    pub fn groups(&self) -> &[ConflictGroupV1] {
        &self.groups
    }

    /// Transfers the groups and their owned reservation to a caller that tracks both lifetimes.
    #[must_use]
    pub fn into_parts(self) -> (Vec<ConflictGroupV1>, u64) {
        (self.groups, self.transient_bytes)
    }

    /// Drops the mapped groups and then releases their retained reservation.
    ///
    /// # Errors
    ///
    /// Returns an accounting error if the reservation does not belong to `budget`.
    pub fn release(
        self,
        budget: &mut PlanningPreparationBudget,
    ) -> Result<(), AssumptionMappingError> {
        let transient_bytes = self.transient_bytes;
        drop(self.groups);
        budget
            .release_transient(transient_bytes)
            .map_err(AssumptionMappingError::Preparation)
    }
}

/// Maps one complete backend-returned literal set to canonical domain conflict groups.
///
/// The planning problem and the entire returned set are validated before any result is exposed.
/// Empty, oversized, duplicate, unknown, and wrong-polarity inputs have distinct errors.
///
/// # Errors
/// Returns [`AssumptionMappingError`] for an invalid problem or returned literal set.
pub fn map_assumption_literals(
    problem: &PlanningProblem,
    literals: &[Literal],
    limits: PlanningIrLimitsV1,
) -> Result<Vec<ConflictGroupV1>, AssumptionMappingError> {
    // Preserve the historical owned-Vec API. The budgeted API below is the ownership-aware seam
    // used by diagnostic preparation; its reservation must remain attached to returned groups.
    map_assumption_literals_impl(problem, literals, limits, None)
}

/// Maps a backend core after charging validation/index/output work under inherited control.
///
/// The returned reservation remains live until [`MappedAssumptionGroups::release`] is called.
///
/// # Errors
/// Returns [`AssumptionMappingError`] for invalid model/core data or preparation interruption.
pub fn map_assumption_literals_with_budget(
    problem: &PlanningProblem,
    literals: &[Literal],
    limits: PlanningIrLimitsV1,
    budget: &mut PlanningPreparationBudget,
) -> Result<MappedAssumptionGroups, AssumptionMappingError> {
    budget
        .precharge_serialized(problem, limits.max_ir_bytes)
        .map_err(AssumptionMappingError::Preparation)?;
    let units = [
        u64::try_from(problem.variables.len()).unwrap_or(u64::MAX),
        u64::try_from(problem.constraints.len()).unwrap_or(u64::MAX),
        u64::try_from(problem.assumptions.len()).unwrap_or(u64::MAX),
        u64::try_from(problem.projections.len()).unwrap_or(u64::MAX),
        u64::try_from(problem.provenance.len()).unwrap_or(u64::MAX),
        u64::try_from(literals.len()).unwrap_or(u64::MAX),
    ]
    .into_iter()
    .try_fold(0_u64, u64::checked_add)
    .ok_or(AssumptionMappingError::Preparation(
        PlanningPreparationError::ArithmeticOverflow,
    ))?;
    let nested_rules = problem
        .assumptions
        .iter()
        .map(|assumption| u64::try_from(assumption.required_rules.len()).unwrap_or(u64::MAX))
        .try_fold(0_u64, u64::checked_add)
        .ok_or(AssumptionMappingError::Preparation(
            PlanningPreparationError::ArithmeticOverflow,
        ))?;
    // Account for B-tree nodes, cloned literals/group identities, and every RuleId retained by
    // the returned groups before building any index or output vector.
    let transient = units
        .checked_mul(256)
        .and_then(|bytes| {
            nested_rules
                .checked_mul((size_of::<RuleId>() as u64).saturating_add(256))
                .and_then(|nested| bytes.checked_add(nested))
        })
        .ok_or(AssumptionMappingError::Preparation(
            PlanningPreparationError::ArithmeticOverflow,
        ))?;
    let work = units
        .checked_mul(4)
        .ok_or(AssumptionMappingError::Preparation(
            PlanningPreparationError::ArithmeticOverflow,
        ))?;
    budget
        .reserve_work(work)
        .map_err(AssumptionMappingError::Preparation)?;
    budget
        .reserve_transient(transient)
        .map_err(AssumptionMappingError::Preparation)?;
    match map_assumption_literals_impl(problem, literals, limits, Some(budget)) {
        Ok(groups) => Ok(MappedAssumptionGroups {
            groups,
            transient_bytes: transient,
        }),
        Err(error) => {
            let _ = budget.release_transient(transient);
            Err(error)
        }
    }
}

fn map_assumption_literals_impl(
    problem: &PlanningProblem,
    literals: &[Literal],
    limits: PlanningIrLimitsV1,
    mut budget: Option<&mut PlanningPreparationBudget>,
) -> Result<Vec<ConflictGroupV1>, AssumptionMappingError> {
    if let Some(budget) = budget.as_deref_mut() {
        validate_with_budget(problem, limits, budget)
            .map(|_| ())
            .map_err(|error| match error {
                BudgetedValidationError::Invalid(error) => {
                    AssumptionMappingError::InvalidProblem(error)
                }
                BudgetedValidationError::Preparation(error) => {
                    AssumptionMappingError::Preparation(error)
                }
            })?;
    } else {
        validate(problem, limits).map_err(AssumptionMappingError::InvalidProblem)?;
    }
    if literals.is_empty() {
        return Err(AssumptionMappingError::Empty);
    }
    if u64::try_from(literals.len()).map_or(true, |count| count > limits.max_assumptions) {
        return Err(AssumptionMappingError::LimitExceeded);
    }

    let assumptions_by_literal = problem
        .assumptions
        .iter()
        .map(|assumption| (assumption.literal.clone(), assumption))
        .collect::<BTreeMap<_, _>>();
    let assumption_variables = problem
        .assumptions
        .iter()
        .map(|assumption| assumption.literal.variable.clone())
        .collect::<BTreeSet<_>>();
    let mut seen = BTreeSet::new();
    let mut mapped = Vec::with_capacity(literals.len());

    for literal in literals {
        if let Some(budget) = budget.as_deref_mut() {
            budget
                .reserve_work(1)
                .map_err(AssumptionMappingError::Preparation)?;
        }
        if !seen.insert(literal.clone()) {
            return Err(AssumptionMappingError::DuplicateLiteral(literal.clone()));
        }
        let Some(assumption) = assumptions_by_literal.get(literal) else {
            return if assumption_variables.contains(&literal.variable) {
                Err(AssumptionMappingError::WrongPolarity(literal.clone()))
            } else {
                Err(AssumptionMappingError::UnknownLiteral(literal.clone()))
            };
        };
        mapped.push(ConflictGroupV1 {
            group_id: assumption.id.clone(),
            required_rules: assumption.required_rules.clone(),
        });
    }

    // Planning validation already proved unique group identities and non-empty canonical rule sets.
    if let Some(budget) = budget {
        budget
            .reserve_work(u64::try_from(mapped.len()).unwrap_or(u64::MAX))
            .map_err(AssumptionMappingError::Preparation)?;
    }
    mapped.sort();
    Ok(mapped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use eutheto_domain_ir::AssumptionGroupId;
    use eutheto_planning_ir::{
        Assumption, BoolVariable, BoolVariableId, Capability, CompilerId, ObjectivePlan,
        PLANNING_IR_SCHEMA_VERSION, PlanningMetadata, ProvenanceId, ProvenanceRecord,
        ProvenanceSourceKind, Variable,
    };
    use eutheto_types::{PackId, RuleId, ScenarioId};
    use std::collections::{BTreeMap, BTreeSet};

    fn problem() -> Result<PlanningProblem, Box<dyn std::error::Error>> {
        let left_variable = BoolVariableId::new("tests.left")?;
        let right_variable = BoolVariableId::new("tests.right")?;
        let left_provenance = ProvenanceId::new("tests.left")?;
        let right_provenance = ProvenanceId::new("tests.right")?;
        Ok(PlanningProblem {
            schema_version: PLANNING_IR_SCHEMA_VERSION,
            variables: vec![
                Variable::Boolean(BoolVariable {
                    id: left_variable.clone(),
                    provenance: left_provenance.clone(),
                }),
                Variable::Boolean(BoolVariable {
                    id: right_variable.clone(),
                    provenance: right_provenance.clone(),
                }),
            ],
            constraints: Vec::new(),
            objectives: ObjectivePlan::default(),
            assumptions: vec![
                Assumption {
                    id: AssumptionGroupId::new("tests.left")?,
                    literal: Literal::positive(left_variable),
                    required_rules: vec!["01890a5d-ac96-7b64-9f74-bbfcf30f9f01".parse::<RuleId>()?],
                    provenance: left_provenance.clone(),
                },
                Assumption {
                    id: AssumptionGroupId::new("tests.right")?,
                    literal: Literal::negative(right_variable),
                    required_rules: vec!["01890a5d-ac96-7b64-9f74-bbfcf30f9f02".parse::<RuleId>()?],
                    provenance: right_provenance.clone(),
                },
            ],
            projections: Vec::new(),
            provenance: vec![
                ProvenanceRecord {
                    id: left_provenance,
                    source_kind: ProvenanceSourceKind::RequiredRule,
                    source_id: "tests.left".to_owned(),
                    entity_refs: Vec::new(),
                    message_key: "tests.left".to_owned(),
                    parameters: BTreeMap::new(),
                    parent: None,
                },
                ProvenanceRecord {
                    id: right_provenance,
                    source_kind: ProvenanceSourceKind::RequiredRule,
                    source_id: "tests.right".to_owned(),
                    entity_refs: Vec::new(),
                    message_key: "tests.right".to_owned(),
                    parameters: BTreeMap::new(),
                    parent: None,
                },
            ],
            metadata: PlanningMetadata {
                pack_id: PackId::new("official.test")?,
                scenario_id: "01890a5d-ac96-7b64-9f74-bbfcf30f9f80".parse::<ScenarioId>()?,
                scenario_revision: 1,
                projection_version: 1,
                compiler_id: CompilerId::new("official.test")?,
                compiler_version: "1.0.0".to_owned(),
                compile_metadata: BTreeMap::new(),
                display_text: BTreeMap::new(),
            },
            declared_capabilities: BTreeSet::from([Capability::Assumptions]),
            split_authorization: None,
        })
    }

    #[test]
    fn maps_complete_set_to_canonical_typed_groups() -> Result<(), Box<dyn std::error::Error>> {
        let problem = problem()?;
        let groups = map_assumption_literals(
            &problem,
            &[
                problem.assumptions[1].literal.clone(),
                problem.assumptions[0].literal.clone(),
            ],
            PlanningIrLimitsV1::DEFAULT,
        )?;
        assert_eq!(groups[0].group_id, problem.assumptions[0].id);
        assert_eq!(groups[1].group_id, problem.assumptions[1].id);
        Ok(())
    }

    #[test]
    fn rejects_every_invalid_returned_literal_shape() -> Result<(), Box<dyn std::error::Error>> {
        let problem = problem()?;
        assert!(matches!(
            map_assumption_literals(&problem, &[], PlanningIrLimitsV1::DEFAULT),
            Err(AssumptionMappingError::Empty)
        ));
        let literal = problem.assumptions[0].literal.clone();
        assert!(matches!(
            map_assumption_literals(
                &problem,
                &[literal.clone(), literal],
                PlanningIrLimitsV1::DEFAULT,
            ),
            Err(AssumptionMappingError::DuplicateLiteral(_))
        ));
        assert!(matches!(
            map_assumption_literals(
                &problem,
                &[problem.assumptions[0].literal.negated()],
                PlanningIrLimitsV1::DEFAULT,
            ),
            Err(AssumptionMappingError::WrongPolarity(_))
        ));
        assert!(matches!(
            map_assumption_literals(
                &problem,
                &[Literal::positive(BoolVariableId::new("tests.unknown")?)],
                PlanningIrLimitsV1::DEFAULT,
            ),
            Err(AssumptionMappingError::UnknownLiteral(_))
        ));
        Ok(())
    }
}
