//! Borrowed precharges for the opaque planning-IR normalization and validation phases.
//!
//! These are logical work/payload bounds, like the other `OperationBudget` reservations,
//! not allocator/RSS predictions. They deliberately also cover noncanonical typed inputs:
//! a generic routine may allocate before discovering their error. No semantic validation,
//! model clone, serialization, or generic phase is performed here.
//!
//! Callers must first bound the whole model/metadata bytes (compiler output preflight, or
//! `measure_ir` on a borrowed projector input), use effective limits no larger than the
//! supported defaults, and retain the SAME budget throughout the operation. Do not measure
//! the model again after its IR bytes have already been reserved. Check cancellation directly
//! before AND after each opaque call; a successful precharge does not make that call
//! interruptible. Projection candidate indexes, lookups, domain membership scans, returned
//! assignments/evidence, normalization and Workforce decoding need a separate precharge.

use super::budget::{OperationBudget, add, count};
use super::{AssignmentConstructionIssue, AssignmentRuleError};
use eutheto_domain_ir::{AssignmentValue, DomainEntityRef};
use eutheto_planning_ir::{
    Constraint, IntDomain, LinearExpression, PlanningIrLimitsV1, PlanningProblem,
    ProjectionExpression, ProvenanceParameter, Variable,
};
use std::mem::size_of;

const WORD: u64 = size_of::<usize>() as u64;
const STRING: u64 = size_of::<String>() as u64;
const NODE: u64 = 2 * WORD; // typed VariableNode reference plus discriminant
// A logical tree entry includes its payload, child/index links, and occupancy bookkeeping.
// This is deliberately not a promise about std's allocator or node capacity.
const TREE_LINKS: u64 = 4 * WORD;
const PATH_BYTES: u64 = 128; // longest fixed validation path plus two decimal usize indices

fn mul(left: u64, right: u64) -> Result<u64, AssignmentRuleError> {
    left.checked_mul(right)
        .ok_or(AssignmentRuleError::InvalidConstruction(
            AssignmentConstructionIssue::ArithmeticOverflow,
        ))
}

fn slots(
    budget: &mut OperationBudget<'_>,
    length: u64,
    bytes: u64,
) -> Result<(), AssignmentRuleError> {
    budget.reserve(0, length, mul(length, bytes)?)
}

fn tree_slots(
    budget: &mut OperationBudget<'_>,
    length: u64,
    payload: u64,
) -> Result<(), AssignmentRuleError> {
    slots(budget, length, add(payload, TREE_LINKS)?)
}

fn levels(length: u64) -> u64 {
    u64::from(u64::BITS - length.leading_zeros())
}

// Rust 1.97.1 alloc/collections/btree/node.rs: B=6, maximum11 keys/node,
// minimum5 keys/nonroot. A tree with h node levels needs >=2*6^(h-1)-1 keys.
// Thus 1+floor(log6((n+1)/2)) bounds levels; no floating-point rounding.
pub(super) fn tree_work(
    budget: &mut OperationBudget<'_>,
    operations: u64,
    length: u64,
    key_units: u64,
) -> Result<(), AssignmentRuleError> {
    let mut quotient = add(length, 1)? / 2;
    let mut height = 1;
    while quotient >= 6 {
        quotient /= 6;
        height += 1;
    }
    budget.steps(mul(operations, mul(11, mul(height, key_units.max(1))?)?)?)
}

fn sorted<T>(
    budget: &mut OperationBudget<'_>,
    values: &[T],
    key_units: u64,
) -> Result<(), AssignmentRuleError> {
    let n = count(values.len())?;
    budget.sort_work(values.len())?;
    // Constructor-bounded typed IDs are one logical comparison, matching sort_work.
    // Variable-width rows and composite keys add their actual element comparisons.
    budget.steps(mul(mul(n, levels(n))?, key_units.saturating_sub(1))?)?;
    // Stable slice sorting can allocate a temporary element buffer. Elements are moved,
    // not cloned, so owned string/row contents are not copied into this reservation.
    slots(budget, n, count(size_of::<T>())?)?;
    budget.steps(mul(n, key_units.max(1))?) // subsequent dedup/order traversal
}

fn domain_clone(
    domain: &IntDomain,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    let n = count(domain.inclusive_ranges.len())?;
    // Input clone and IntDomain::new's normalized Vec, in addition to sort scratch.
    slots(budget, mul(2, n)?, 16)?;
    sorted(budget, &domain.inclusive_ranges, 1)?;
    budget.steps(mul(4, n)?) // reversed scan, merge, equality, numeric bounds
}

fn expression_normalization(
    expression: &LinearExpression,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    let n = count(expression.terms.len())?;
    // Our scan plus canonical::canonicalize_expression's identical no-op check.
    budget.steps(mul(4, n)?)?;
    if expression.terms.iter().all(|term| term.coefficient != 0)
        && expression
            .terms
            .windows(2)
            .all(|pair| pair[0].variable < pair[1].variable)
    {
        return Ok(());
    }
    // Cloned terms, combining tree, then collected output. The ID string is cloned
    // only once and subsequently moved through these containers.
    slots(budget, mul(2, n)?, STRING + 8)?;
    tree_slots(budget, n, STRING + 8)?;
    for term in &expression.terms {
        budget.step()?;
        let bytes = count(term.variable.as_str().len())?;
        budget.reserve(0, 0, bytes)?;
        budget.steps(bytes)?;
    }
    tree_work(budget, mul(2, n)?, n, 1)?; // get then insert, even duplicates
    budget.steps(mul(2, n)?) // checked combination and output filtering
}

fn expression_validation(
    expression: &LinearExpression,
    variables: u64,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    let n = count(expression.terms.len())?;
    budget.steps(mul(n, 9)?)?; // strict ordering plus bound arithmetic
    tree_work(budget, n, variables, 1)
}

fn features(
    problem: &PlanningProblem,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    // At most one count-map entry and one required-capability set entry per feature
    // event. This input-derived bound also covers future repetition of a capability.
    let mut events = add(
        count(problem.variables.len())?,
        count(problem.constraints.len())?,
    )?;
    events = add(events, count(problem.projections.len())?)?;
    events = add(events, u64::from(!problem.assumptions.is_empty()))?;
    for level in &problem.objectives.levels {
        budget.step()?;
        events = add(events, count(level.terms.len())?)?;
    }
    // There are 27 distinct Capability variants in the current generic vocabulary.
    let entries = events.min(27);
    tree_slots(budget, entries, 16)?;
    tree_slots(budget, entries, 8)?;
    tree_work(budget, events, entries, 1)?;
    tree_work(budget, entries, entries, 1)?;
    budget.steps(add(events, count(problem.declared_capabilities.len())?)?)
}

/// Precharge one call to `PlanningProblem::canonicalize`, not a whole-model clone.
pub(super) fn precharge_canonicalization(
    problem: &PlanningProblem,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    budget.check()?;
    for variable in &problem.variables {
        budget.step()?;
        match variable {
            Variable::Integer(integer) => domain_clone(&integer.domain, budget)?,
            Variable::Boolean(_) | Variable::Interval(_) => {}
        }
    }
    sorted(budget, &problem.variables, 1)?;
    for record in &problem.constraints {
        budget.step()?;
        sorted(budget, &record.enforcement, 2)?;
        sorted(budget, &record.tags, 1)?;
        match &record.body {
            Constraint::BoolOr { literals }
            | Constraint::BoolAnd { literals }
            | Constraint::AtMostOne { literals }
            | Constraint::ExactlyOne { literals }
            | Constraint::CardinalityRange { literals, .. } => {
                sorted(budget, literals, 2)?;
            }
            Constraint::AllDifferent { variables }
            | Constraint::Min {
                inputs: variables, ..
            }
            | Constraint::Max {
                inputs: variables, ..
            } => sorted(budget, variables, 1)?,
            Constraint::AllowedTable { rows, .. } | Constraint::ForbiddenTable { rows, .. } => {
                let mut width = 0;
                // Record::canonicalize sorts rows without checking arity first.
                for row in rows {
                    budget.step()?;
                    width = width.max(count(row.len())?);
                }
                sorted(budget, rows, width.max(1))?;
            }
            Constraint::NoOverlap { intervals } => sorted(budget, intervals, 1)?,
            Constraint::Cumulative {
                intervals, demands, ..
            } => {
                let n = count(intervals.len().min(demands.len()))?;
                // drain/zip drops unmatched tails on malformed typed input; all original
                // elements are visited/dropped even though only min(len) pairs are sorted.
                budget.steps(add(count(intervals.len())?, count(demands.len())?)?)?;
                slots(budget, n, STRING + 8)?; // pairs
                slots(budget, n, STRING + 8)?; // unzip outputs
                // Sorting the longer original ID slice is a safe borrowed upper bound.
                sorted(budget, intervals, 1)?;
                slots(budget, n, 8)?; // pair sort scratch has an extra demand per slot
            }
            Constraint::LinearComparison(comparison)
            | Constraint::ReifiedLinearComparison { comparison, .. } => {
                expression_normalization(&comparison.expression, budget)?;
            }
            Constraint::Implication { .. }
            | Constraint::Equivalence { .. }
            | Constraint::Element { .. }
            | Constraint::Equality { .. }
            | Constraint::AbsDifference { .. } => {}
        }
    }
    sorted(budget, &problem.constraints, 1)?;
    for level in &problem.objectives.levels {
        budget.step()?;
        for term in &level.terms {
            budget.step()?;
            expression_normalization(&term.expression, budget)?;
        }
        sorted(budget, &level.terms, 1)?;
    }
    for assumption in &problem.assumptions {
        budget.step()?;
        sorted(budget, &assumption.required_rules, 1)?;
    }
    sorted(budget, &problem.assumptions, 1)?;
    sorted(budget, &problem.projections, 1)?;
    for projection in &problem.projections {
        budget.step()?;
        match &projection.expression {
            ProjectionExpression::Linear(expression) => {
                expression_normalization(expression, budget)?;
            }
            ProjectionExpression::Boolean(_)
            | ProjectionExpression::Integer(_)
            | ProjectionExpression::Interval(_)
            | ProjectionExpression::Constant(_) => {}
        }
    }
    sorted(budget, &problem.provenance, 1)?;
    for record in &problem.provenance {
        budget.step()?;
        sorted(budget, &record.entity_refs, 2)?;
    }
    features(problem, budget)?;
    budget.check()
}

// Precharge component analysis's node clones without constructing its node vector.
fn precharge_constraint_nodes(
    body: &Constraint,
    budget: &mut OperationBudget<'_>,
) -> Result<u64, AssignmentRuleError> {
    let mut references = 0;
    let mut visit = |_id: &str| -> Result<(), AssignmentRuleError> {
        slots(budget, 1, 2 * NODE)?;
        budget.steps(2)?; // sizing visit and subsequent borrowed-node construction
        references = add(references, 1)?;
        Ok(())
    };
    match body {
        Constraint::BoolOr { literals }
        | Constraint::BoolAnd { literals }
        | Constraint::AtMostOne { literals }
        | Constraint::ExactlyOne { literals }
        | Constraint::CardinalityRange { literals, .. } => {
            for literal in literals {
                visit(literal.variable.as_str())?;
            }
        }
        Constraint::Implication {
            antecedent,
            consequent,
        } => {
            visit(antecedent.variable.as_str())?;
            visit(consequent.variable.as_str())?;
        }
        Constraint::Equivalence { left, right } => {
            visit(left.variable.as_str())?;
            visit(right.variable.as_str())?;
        }
        Constraint::LinearComparison(comparison)
        | Constraint::ReifiedLinearComparison { comparison, .. } => {
            for term in &comparison.expression.terms {
                visit(term.variable.as_str())?;
            }
            if let Constraint::ReifiedLinearComparison { literal, .. } = body {
                visit(literal.variable.as_str())?;
            }
        }
        Constraint::AllDifferent { variables }
        | Constraint::AllowedTable { variables, .. }
        | Constraint::ForbiddenTable { variables, .. } => {
            for variable in variables {
                visit(variable.as_str())?;
            }
        }
        Constraint::Min { target, inputs } | Constraint::Max { target, inputs } => {
            visit(target.as_str())?;
            for input in inputs {
                visit(input.as_str())?;
            }
        }
        Constraint::Element { index, target, .. } => {
            visit(index.as_str())?;
            visit(target.as_str())?;
        }
        Constraint::Equality { left, right } => {
            visit(left.as_str())?;
            visit(right.as_str())?;
        }
        Constraint::AbsDifference {
            target,
            left,
            right,
        } => {
            visit(target.as_str())?;
            visit(left.as_str())?;
            visit(right.as_str())?;
        }
        Constraint::NoOverlap { intervals } | Constraint::Cumulative { intervals, .. } => {
            for interval in intervals {
                visit(interval.as_str())?;
            }
        }
    }
    Ok(references)
}

fn projection_references(expression: &ProjectionExpression) -> Result<u64, AssignmentRuleError> {
    match expression {
        ProjectionExpression::Boolean(_)
        | ProjectionExpression::Integer(_)
        | ProjectionExpression::Interval(_) => Ok(1),
        ProjectionExpression::Linear(expression) => count(expression.terms.len()),
        ProjectionExpression::Constant(value) => {
            match value {
                AssignmentValue::Boolean(_)
                | AssignmentValue::Integer(_)
                | AssignmentValue::Interval(_)
                | AssignmentValue::Absent => {}
            }
            Ok(0)
        }
    }
}

fn borrowed_nodes(budget: &mut OperationBudget<'_>, n: u64) -> Result<(), AssignmentRuleError> {
    // Vec growth and extension can replace the slot buffer; IDs remain borrowed.
    slots(budget, n, 2 * NODE)?;
    budget.steps(n)
}

fn edge(
    budget: &mut OperationBudget<'_>,
    n: u64,
    variables: u64,
) -> Result<u64, AssignmentRuleError> {
    tree_slots(budget, n, WORD)?; // union_edge's borrowed uniqueness set
    tree_work(budget, n, n, 1)?;
    tree_work(budget, n, variables, 1)?;
    // Two finds per union; duplicates and singleton edges only reduce this bound.
    mul(2, n)
}

fn path_allocations(
    budget: &mut OperationBudget<'_>,
    number: u64,
) -> Result<(), AssignmentRuleError> {
    slots(budget, number, PATH_BYTES)?;
    budget.steps(mul(number, PATH_BYTES)?)
}

fn parameter(
    value: &ProvenanceParameter,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    budget.step()?;
    match value {
        ProvenanceParameter::Boolean(_) | ProvenanceParameter::Integer(_) => Ok(()),
        ProvenanceParameter::Text(text) => budget.steps(count(text.len())?),
        ProvenanceParameter::Entity(entity) => entity_text(entity, budget),
    }
}

fn entity_text(
    entity: &DomainEntityRef,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    budget.steps(add(
        count(entity.kind.as_str().len())?,
        count(entity.id.as_str().len())?,
    )?)
}

/// Precharge one generic `validate` call, including its feature/component analysis.
/// This is also the validation performed inside generic `project_candidate`.
pub(super) fn precharge_validation(
    problem: &PlanningProblem,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    budget.check()?;
    let variables = count(problem.variables.len())?;
    let mut finds = variables; // final component grouping performs one find per node
    let provenance = count(problem.provenance.len())?;
    let constraints = count(problem.constraints.len())?;
    let assumptions = count(problem.assumptions.len())?;
    let projections = count(problem.projections.len())?;
    let objective_levels = count(problem.objectives.levels.len())?;
    let roots = add(
        add(add(variables, constraints)?, add(assumptions, projections)?)?,
        provenance,
    )?;
    budget.steps(roots)?; // strict root ordering
    path_allocations(budget, 1)?; // first returned ValidationError, on any failure path

    // VariableReferences plus its cross-kind ID set; constraint/level/assignment IDs.
    tree_slots(budget, mul(2, variables)?, 2 * WORD)?;
    tree_slots(budget, provenance, WORD)?;
    tree_work(budget, mul(2, variables)?, variables, 1)?;
    tree_work(budget, provenance, provenance, 1)?;
    tree_work(budget, variables, provenance, 1)?;
    tree_slots(
        budget,
        add(add(constraints, objective_levels)?, projections)?,
        WORD,
    )?;
    tree_work(budget, constraints, constraints, 1)?;
    tree_work(budget, objective_levels, objective_levels, 1)?;
    tree_work(budget, projections, projections, 1)?;

    let mut max_ranges = 0;
    let mut interval_count = 0;
    for (index, variable) in problem.variables.iter().enumerate() {
        budget.step()?;
        let path_bytes = add(
            count("variables[].domain".len())?,
            u64::from(index.max(1).ilog10() + 1),
        )?;
        slots(budget, 2, path_bytes)?; // variable domain and provenance paths
        budget.steps(mul(2, path_bytes)?)?;
        match variable {
            Variable::Integer(integer) => {
                max_ranges = max_ranges.max(count(integer.domain.inclusive_ranges.len())?);
                domain_clone(&integer.domain, budget)?;
            }
            Variable::Interval(_) => interval_count = add(interval_count, 1)?,
            Variable::Boolean(_) => {}
        }
    }
    // The generic interval equation does a start-range × duration-range product,
    // partition_point on end ranges per pair, and two duration scans. Using the
    // largest borrowed domain covers unresolved/duplicate/noncanonical structures too.
    budget.steps(mul(
        interval_count,
        add(
            mul(2, max_ranges)?,
            mul(mul(max_ranges, max_ranges)?, add(levels(max_ranges), 8)?)?,
        )?,
    )?)?;
    tree_work(budget, mul(4, interval_count)?, variables, 1)?;
    path_allocations(budget, mul(5, interval_count)?)?;

    finds = add(
        finds,
        precharge_constraints(problem, variables, provenance, budget)?,
    )?;
    let (relation_finds, term_count) = precharge_relations(problem, variables, provenance, budget)?;
    finds = add(finds, relation_finds)?;
    let used_roots = add(
        add(
            add(variables, constraints)?,
            add(objective_levels, term_count)?,
        )?,
        add(assumptions, projections)?,
    )?;
    precharge_provenance(problem, used_roots, budget)?;
    precharge_components(problem, variables, finds, budget)?;
    budget.check()
}

fn precharge_constraints(
    problem: &PlanningProblem,
    variables: u64,
    provenance: u64,
    budget: &mut OperationBudget<'_>,
) -> Result<u64, AssignmentRuleError> {
    let mut finds = 0;
    for (index, record) in problem.constraints.iter().enumerate() {
        budget.step()?;
        let refs = precharge_constraint_nodes(&record.body, budget)?;
        let enforcement = count(record.enforcement.len())?;
        // validate_constraints allocates one bare path, two each for enforcement,
        // tags and body, plus one enforcement path per literal. Error copies have
        // their separate reservation; an early error constructs fewer paths.
        let path_bytes = add(
            count("constraints[]".len())?,
            u64::from(index.max(1).ilog10() + 1),
        )?;
        let paths = add(7, enforcement)?;
        let suffix_bytes = add(
            mul(add(2, enforcement)?, count(".enforcement".len())?)?,
            mul(2, count(".tags".len() + ".body".len())?)?,
        )?;
        let bytes = add(mul(paths, path_bytes)?, suffix_bytes)?;
        budget.reserve(0, paths, bytes)?;
        budget.steps(bytes)?;
        tree_work(budget, 1, provenance, 1)?;
        budget.steps(mul(
            add(add(refs, enforcement)?, count(record.tags.len())?)?,
            2,
        )?)?;
        tree_work(budget, add(refs, enforcement)?, variables, 1)?;
        match &record.body {
            Constraint::LinearComparison(comparison)
            | Constraint::ReifiedLinearComparison { comparison, .. } => {
                expression_validation(&comparison.expression, variables, budget)?;
            }
            Constraint::AllowedTable { rows, .. } | Constraint::ForbiddenTable { rows, .. } => {
                // strict(rows) precedes arity rejection. Actual row lengths, NOT declared
                // arity, bound comparisons on malicious ragged rows. Each cell participates
                // in at most two neighboring row comparisons and one numeric scan.
                for row in rows {
                    budget.step()?;
                    budget.steps(mul(3, count(row.len())?)?)?;
                }
            }
            Constraint::Element { values, .. } => budget.steps(count(values.len())?)?,
            Constraint::Cumulative { demands, .. } => {
                budget.steps(mul(3, count(demands.len())?)?)?;
            }
            Constraint::BoolOr { .. }
            | Constraint::BoolAnd { .. }
            | Constraint::Implication { .. }
            | Constraint::Equivalence { .. }
            | Constraint::AtMostOne { .. }
            | Constraint::ExactlyOne { .. }
            | Constraint::CardinalityRange { .. }
            | Constraint::AllDifferent { .. }
            | Constraint::Min { .. }
            | Constraint::Max { .. }
            | Constraint::Equality { .. }
            | Constraint::AbsDifference { .. }
            | Constraint::NoOverlap { .. } => {}
        }
        // Node vectors and enforcement extension belong only to component analysis;
        // generic validation counts references without allocating them.
        let edge_refs = add(refs, enforcement)?;
        borrowed_nodes(budget, enforcement)?;
        finds = add(finds, edge(budget, edge_refs, variables)?)?;
    }
    Ok(finds)
}

fn precharge_relations(
    problem: &PlanningProblem,
    variables: u64,
    provenance: u64,
    budget: &mut OperationBudget<'_>,
) -> Result<(u64, u64), AssignmentRuleError> {
    let mut finds = 0;
    let assumptions = count(problem.assumptions.len())?;
    let mut term_count = 0;
    for level in &problem.objectives.levels {
        budget.step()?;
        path_allocations(budget, 2)?;
        tree_work(budget, 1, provenance, 1)?;
        budget.steps(mul(count(level.terms.len())?, 5)?)?;
        let mut edge_refs = 0;
        for term in &level.terms {
            budget.step()?;
            term_count = add(term_count, 1)?;
            tree_work(budget, 1, provenance, 1)?;
            expression_validation(&term.expression, variables, budget)?;
            let refs = count(term.expression.terms.len())?;
            edge_refs = add(edge_refs, refs)?;
            borrowed_nodes(budget, refs)?; // each flat_map expression_nodes temporary
        }
        // flat_map then collects a second slot array of borrowed nodes.
        slots(budget, edge_refs, 2 * NODE)?;
        finds = add(finds, edge(budget, edge_refs, variables)?)?;
    }

    if assumptions != 0 {
        tree_slots(budget, provenance, 2 * WORD)?;
        tree_work(budget, provenance, provenance, 1)?;
    }
    tree_slots(budget, assumptions, WORD)?;
    tree_work(budget, assumptions, assumptions, 1)?;
    let mut rules = 0;
    for assumption in &problem.assumptions {
        budget.step()?;
        let n = count(assumption.required_rules.len())?;
        rules = add(rules, n)?;
        path_allocations(budget, 7)?;
        tree_work(budget, 2, provenance, 1)?;
        tree_work(budget, 1, variables, 1)?;
        budget.steps(n)?;
        borrowed_nodes(budget, 1)?;
        finds = add(finds, edge(budget, 1, variables)?)?;
    }
    tree_slots(budget, rules, WORD)?;
    tree_work(budget, rules, rules, 1)?;

    for projection in &problem.projections {
        budget.step()?;
        tree_work(budget, 1, provenance, 1)?;
        let refs = projection_references(&projection.expression)?;
        match &projection.expression {
            ProjectionExpression::Linear(expression) => {
                expression_validation(expression, variables, budget)?;
            }
            ProjectionExpression::Boolean(_)
            | ProjectionExpression::Integer(_)
            | ProjectionExpression::Interval(_) => tree_work(budget, 1, variables, 1)?,
            ProjectionExpression::Constant(_) => budget.step()?,
        }
        // Only component analysis allocates projection nodes; validation counts them.
        borrowed_nodes(budget, refs)?;
        finds = add(finds, edge(budget, refs, variables)?)?;
    }
    Ok((finds, term_count))
}

fn precharge_provenance(
    problem: &PlanningProblem,
    used_roots: u64,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    let provenance = count(problem.provenance.len())?;
    // Ancestry and coverage borrow the canonical roots through binary lookup.
    let depth = provenance.min(PlanningIrLimitsV1::DEFAULT.max_provenance_depth);
    slots(budget, provenance, count(size_of::<Option<u64>>())?)?;
    slots(budget, depth, WORD)?;
    // Each uncached record enters a path once; each outer record can encounter
    // one cached tail, and an invalid graph adds at most one failing terminal.
    // Path comparisons precede depth/missing-reference checks, including on failure.
    let iterations = add(mul(2, provenance)?, 1)?;
    let per_iteration = add(depth, add(levels(provenance), 9)?)?;
    // Initialization and successful unwind/cache writes are linear. The remaining
    // bound covers binary comparisons, path scans and checked traversal bookkeeping.
    budget.steps(add(mul(4, provenance)?, mul(iterations, per_iteration)?)?)?;
    for record in &problem.provenance {
        budget.step()?;
        budget.steps(mul(count(record.entity_refs.len())?, 2)?)?;
        budget.steps(add(
            count(record.source_id.len())?,
            count(record.message_key.len())?,
        )?)?;
        for (key, value) in &record.parameters {
            budget.steps(add(1, count(key.len())?)?)?;
            parameter(value, budget)?;
        }
    }
    let used = add(used_roots, provenance)?;
    tree_slots(budget, used, WORD)?;
    slots(budget, used, 2 * WORD)?; // pending growth
    tree_work(budget, used, used, 1)?;
    budget.steps(mul(used, add(levels(provenance), 1)?)?)?;
    budget.steps(used)?;

    budget.steps(count(problem.metadata.compiler_version.len())?)?;
    for (key, value) in &problem.metadata.compile_metadata {
        budget.steps(add(1, count(key.as_str().len())?)?)?;
        parameter(value, budget)?;
    }
    for (key, value) in &problem.metadata.display_text {
        budget.steps(add(1, add(count(key.len())?, count(value.len())?)?)?)?;
    }
    if let Some(authorization) = &problem.split_authorization {
        budget.steps(add(
            count(authorization.component_hash.len())?,
            count(authorization.domain_merge_contract.len())?,
        )?)?;
    }
    features(problem, budget)?;
    budget.check()
}

fn precharge_components(
    problem: &PlanningProblem,
    variables: u64,
    mut finds: u64,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    // Conservative container/order bounds for declared-node sorting and its index.
    tree_slots(budget, variables, NODE)?;
    slots(budget, variables, NODE)?;
    tree_slots(budget, variables, NODE + WORD)?;
    tree_work(budget, mul(2, variables)?, variables, 1)?;
    slots(budget, variables, 2 * WORD)?; // union-find parent and component size
    budget.steps(mul(2, variables)?)?; // initialize both arrays
    let mut encoded_bytes = 0;
    for variable in &problem.variables {
        budget.step()?;
        let id = match variable {
            Variable::Boolean(value) => value.id.as_str(),
            Variable::Integer(value) => value.id.as_str(),
            Variable::Interval(value) => value.id.as_str(),
        };
        let bytes = count(id.len())?;
        budget.steps(2)?; // declaration and index node copies, without ID payload copies
        encoded_bytes = add(encoded_bytes, add(bytes, 9)?)?; // longest prefix: interval:
        match variable {
            Variable::Interval(interval) => {
                let refs = add(4, u64::from(interval.presence.is_some()))?;
                borrowed_nodes(budget, refs)?;
                finds = add(finds, edge(budget, refs, variables)?)?;
            }
            Variable::Boolean(_) | Variable::Integer(_) => {}
        }
    }
    // Union by size at least doubles a component whenever a node gains depth.
    // Thus depth <= floor(log2(V)); path halving visits at most ceil(depth / 2)
    // nodes per find. Raw declarations/references overcount deduplicated nodes
    // and actual finds, so this also covers duplicates, unknowns and V=0/1.
    let halving_visits = u64::from(variables.max(1).ilog2()).div_ceil(2);
    // One terminal check per find, plus six comparison/add/update steps per
    // union (two finds). Grouping's extra finds conservatively pay those too.
    budget.steps(mul(finds, add(halving_visits, 4)?)?)?;
    tree_slots(budget, variables, WORD + 3 * WORD)?; // grouped map, worst case singleton groups
    tree_work(budget, variables, variables, 1)?;
    slots(budget, variables, 2 * STRING)?; // group Vec growth
    budget.reserve(0, 0, encoded_bytes)?;
    slots(budget, variables, 3 * WORD)?; // raw_groups Vec
    slots(budget, variables, 3 * WORD)?; // group Vec sort scratch
    slots(budget, variables, STRING)?; // inner group string sort scratch
    // Sum of group n log n <= V log V. Each outer comparison examines only the
    // first encoded ID: disjoint groups have different first IDs, even if the input
    // contains duplicate declarations (declared_nodes deduplicates them).
    budget.sort_work(problem.variables.len())?;
    budget.sort_work(problem.variables.len())?;
    slots(budget, variables, STRING + 3 * WORD + 32)?; // components + component.c<usize>
    budget.reserve(0, 0, 64)?; // component hash hex String
    budget.steps(add(add(encoded_bytes, mul(variables, 32)?)?, 64)?)?; // encoding, ID checks, hash
    budget.check()
}
