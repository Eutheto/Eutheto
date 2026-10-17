use super::{AssignmentConstructionIssue, AssignmentRuleError, AssignmentRuleLimit};
use crate::temporal::{CalendarStep, ResolutionStep};
use eutheto_planning_ir::PlanningIrLimitsV1;
use eutheto_types::{OperationControl, OperationInterruption};
use serde::Serialize;
use std::io::{self, Write};

pub const MAX_INSPECTED_PAIRS: u64 = 1_000_000;
pub const MAX_WORK_STEPS: u64 = 20_000_000;
// Projection repeats generic model validation before decoding the candidate. The reviewed
// rolling-hours model exceeds the compilation allowance in this distinct, bounded phase.
const MAX_PROJECTION_WORK_STEPS: u64 = 30_000_000;
pub const MAX_EXPANDED_INTERVALS: u64 = 32_768;
pub const MAX_SELECTED_PAIRS: u64 = 100_000;
const MAX_EVALUATION_RECORDS: u64 = 100_000;
const MAX_EVALUATION_ITEMS: u64 = 1_000_000;
const MAX_EVALUATION_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Clone, Default)]
struct Usage {
    records: u64,
    items: u64,
    bytes: u64,
}

#[derive(Clone)]
pub(crate) struct OperationBudget<'a> {
    control: Option<&'a OperationControl>,
    limits: PlanningIrLimitsV1,
    steps: u64,
    max_work_steps: u64,
    expanded_intervals: u64,
    retained: Usage,
    ir: Usage,
    max_records: u64,
    max_items: u64,
    max_bytes: u64,
    #[cfg(test)]
    cancel_at_step: Option<u64>,
}

impl<'a> OperationBudget<'a> {
    pub fn analysis(control: Option<&'a OperationControl>, limits: PlanningIrLimitsV1) -> Self {
        Self {
            control,
            limits,
            steps: 0,
            max_work_steps: MAX_WORK_STEPS,
            expanded_intervals: 0,
            retained: Usage::default(),
            ir: Usage::default(),
            max_records: limits
                .max_provenance_records
                .min(PlanningIrLimitsV1::DEFAULT.max_provenance_records),
            max_items: limits
                .max_total_refs
                .min(PlanningIrLimitsV1::DEFAULT.max_total_refs),
            max_bytes: limits
                .max_ir_bytes
                .min(PlanningIrLimitsV1::DEFAULT.max_ir_bytes),
            #[cfg(test)]
            cancel_at_step: None,
        }
    }

    pub fn projection(control: Option<&'a OperationControl>, limits: PlanningIrLimitsV1) -> Self {
        let mut budget = Self::analysis(control, limits);
        budget.max_work_steps = MAX_PROJECTION_WORK_STEPS;
        budget
    }

    pub fn evaluation(control: Option<&'a OperationControl>) -> Self {
        let mut budget = Self::analysis(control, PlanningIrLimitsV1::DEFAULT);
        budget.max_records = MAX_EVALUATION_RECORDS;
        budget.max_items = MAX_EVALUATION_ITEMS;
        budget.max_bytes = MAX_EVALUATION_BYTES;
        budget
    }

    pub fn control(&self) -> Option<&'a OperationControl> {
        self.control
    }

    pub fn check(&self) -> Result<(), AssignmentRuleError> {
        self.control
            .map_or(Ok(()), OperationControl::check)
            .map_err(interruption)
    }

    pub fn steps(&mut self, amount: u64) -> Result<(), AssignmentRuleError> {
        self.check()?;
        let next = add(self.steps, amount)?;
        #[cfg(test)]
        if self
            .cancel_at_step
            .is_some_and(|threshold| next >= threshold)
        {
            if let Some(OperationControl::Cancellation(token)) = self.control {
                token.cancel();
            }
            self.check()?;
        }
        within(next, self.max_work_steps, AssignmentRuleLimit::WorkSteps)?;
        self.steps = next;
        Ok(())
    }

    pub fn step(&mut self) -> Result<(), AssignmentRuleError> {
        self.steps(1)
    }

    pub fn sort_work(&mut self, length: usize) -> Result<(), AssignmentRuleError> {
        let count = count(length)?;
        let levels = u64::from(u64::BITS - count.leading_zeros());
        self.steps(count.checked_mul(levels).ok_or_else(arithmetic)?)
    }

    /// Deterministic test actor: cancel the real caller token at a later local checkpoint.
    /// Tests arm this only after entering the operation phase they exercise.
    #[cfg(test)]
    pub fn cancel_after_steps(&mut self, additional: u64) -> Result<(), AssignmentRuleError> {
        if !matches!(self.control, Some(OperationControl::Cancellation(_))) {
            return Err(AssignmentRuleError::InvalidConstruction(
                AssignmentConstructionIssue::InvalidRecord,
            ));
        }
        self.cancel_at_step = Some(add(self.steps, additional)?);
        Ok(())
    }

    #[cfg(test)]
    pub fn remaining_output(&self) -> (u64, u64, u64) {
        (
            self.max_records - self.retained.records,
            self.max_items - self.retained.items,
            self.max_bytes - self.retained.bytes,
        )
    }

    #[cfg(test)]
    pub fn reserved_ir_bytes(&self) -> u64 {
        self.ir.bytes
    }

    /// Reserve before allocating owned records or variable-length fields. Counters are cumulative,
    /// conservatively including temporary structural records; releasing a value does not refill them.
    pub fn reserve(
        &mut self,
        records: u64,
        items: u64,
        bytes: u64,
    ) -> Result<(), AssignmentRuleError> {
        self.check()?;
        reserve(
            &mut self.retained,
            records,
            items,
            bytes,
            self.max_records,
            self.max_items,
            self.max_bytes,
        )
    }

    /// IR output has separate byte/reference limits but shares this operation's work counter.
    pub fn reserve_ir(
        &mut self,
        records: u64,
        items: u64,
        bytes: u64,
    ) -> Result<(), AssignmentRuleError> {
        self.check()?;
        let defaults = PlanningIrLimitsV1::DEFAULT;
        let records_limit = add(
            add(self.variable_limit(), self.constraint_limit())?,
            self.provenance_limit(),
        )?;
        reserve(
            &mut self.ir,
            records,
            items,
            bytes,
            records_limit,
            self.limits.max_total_refs.min(defaults.max_total_refs),
            self.limits.max_ir_bytes.min(defaults.max_ir_bytes),
        )
    }

    /// Charge the measured complete model alongside analysis/diagnostics and scratch.
    /// Call once after full IR preflight, before allocating its output graph.
    pub fn reserve_ir_retention(&mut self) -> Result<(), AssignmentRuleError> {
        self.reserve(self.ir.records, self.ir.items, self.ir.bytes)
    }

    pub fn variable_limit(&self) -> u64 {
        self.limits
            .max_variables
            .min(PlanningIrLimitsV1::DEFAULT.max_variables)
    }

    pub fn constraint_limit(&self) -> u64 {
        self.limits
            .max_constraints
            .min(PlanningIrLimitsV1::DEFAULT.max_constraints)
    }

    pub fn provenance_limit(&self) -> u64 {
        self.limits
            .max_provenance_records
            .min(PlanningIrLimitsV1::DEFAULT.max_provenance_records)
    }

    pub fn expanded_interval(&mut self) -> Result<(), AssignmentRuleError> {
        self.step()?;
        let next = add(self.expanded_intervals, 1)?;
        within(
            next,
            MAX_EXPANDED_INTERVALS,
            AssignmentRuleLimit::ExpandedIntervals,
        )?;
        self.expanded_intervals = next;
        Ok(())
    }

    /// Bound borrowed serialization before materializing or retaining a payload.
    pub fn measure<T: Serialize + ?Sized>(&self, value: &T) -> Result<u64, AssignmentRuleError> {
        self.measure_with_limit(value, self.max_bytes - self.retained.bytes)
    }

    pub fn measure_ir<T: Serialize + ?Sized>(&self, value: &T) -> Result<u64, AssignmentRuleError> {
        let maximum = self
            .limits
            .max_ir_bytes
            .min(PlanningIrLimitsV1::DEFAULT.max_ir_bytes);
        self.measure_with_limit(value, maximum - self.ir.bytes)
    }

    fn measure_with_limit<T: Serialize + ?Sized>(
        &self,
        value: &T,
        limit: u64,
    ) -> Result<u64, AssignmentRuleError> {
        self.check()?;
        let mut writer = CountingWriter {
            bytes: 0,
            limit,
            control: self.control,
            failure: None,
        };
        if serde_json::to_writer(&mut writer, value).is_err() {
            return Err(writer
                .failure
                .unwrap_or(AssignmentRuleError::InvalidConstruction(
                    AssignmentConstructionIssue::InvalidRecord,
                )));
        }
        Ok(writer.bytes)
    }

    pub fn resolution_step(&mut self, event: ResolutionStep) -> Result<(), AssignmentRuleError> {
        self.step()?;
        match event {
            ResolutionStep::Inspect => Ok(()),
            ResolutionStep::RetainRecurrenceKey => self.reserve(1, 1, 16),
            ResolutionStep::Sort(length) => self.sort_work(length),
            // Conservative fixed logical-payload bounds, not Rust layout/RSS estimates.
            ResolutionStep::RetainOwner | ResolutionStep::RetainSpec => self.reserve(1, 2, 128),
            ResolutionStep::RetainShift => self.reserve(1, 3, 512),
        }
    }

    pub fn calendar_step(&mut self, event: CalendarStep) -> Result<(), AssignmentRuleError> {
        self.step()?;
        match event {
            CalendarStep::Inspect => Ok(()),
            CalendarStep::RetainWindow => self.reserve(1, 1, 512),
        }
    }
}

/// One cumulative native preparation allowance; neither integrity hashes nor this budget admit data.
pub struct WorkforceAdmissionBudget<'a> {
    inner: OperationBudget<'a>,
}

impl<'a> WorkforceAdmissionBudget<'a> {
    /// Starts the existing bounded evaluation profile, retaining the caller's interruption control.
    #[must_use]
    pub fn new(control: Option<&'a OperationControl>) -> Self {
        Self {
            inner: OperationBudget::evaluation(control),
        }
    }

    /// Checks borrowed JSON shape/size while charging every traversal to this operation.
    /// # Errors
    /// Rejects malformed, oversized or interrupted input and exhausted work.
    pub fn preflight<T: Serialize + ?Sized>(
        &mut self,
        value: &T,
    ) -> Result<(), eutheto_domain_api::DomainPackError> {
        super::boundary::preflight(
            value,
            &mut self.inner,
            eutheto_domain_api::ContractJsonLimits::DEFAULT,
        )
        .map(|_| ())
        .map_err(|error| super::authority::operation_error(&error))
    }

    /// Reserves measured JSON scratch before native cloning/serialization.
    /// # Errors
    /// Rejects bounds, interrupted input or exhausted cumulative retention/work.
    pub fn reserve_json<T: Serialize + ?Sized>(
        &mut self,
        value: &T,
        copies: u64,
    ) -> Result<(), eutheto_domain_api::DomainPackError> {
        let measured = super::boundary::preflight(
            value,
            &mut self.inner,
            eutheto_domain_api::ContractJsonLimits::DEFAULT,
        )
        .map_err(|error| super::authority::operation_error(&error))?;
        measured
            .reserve_json(&mut self.inner, copies)
            .map_err(|error| super::authority::operation_error(&error))
    }

    /// Reserves native records/items/bytes before retaining them; release does not refill usage.
    /// # Errors
    /// Rejects checked arithmetic, exhausted retention or interruption.
    pub fn reserve(
        &mut self,
        records: u64,
        items: u64,
        bytes: u64,
    ) -> Result<(), eutheto_domain_api::DomainPackError> {
        self.inner
            .reserve(records, items, bytes)
            .map_err(|error| super::authority::operation_error(&error))
    }

    /// Charges one native source/command visit.
    /// # Errors
    /// Rejects exhausted cumulative work or interruption.
    pub fn step(&mut self) -> Result<(), eutheto_domain_api::DomainPackError> {
        self.inner
            .step()
            .map_err(|error| super::authority::operation_error(&error))
    }

    /// Charges a bounded native traversal.
    /// # Errors
    /// Rejects exhausted cumulative work or interruption.
    pub fn steps(&mut self, amount: u64) -> Result<(), eutheto_domain_api::DomainPackError> {
        self.inner
            .steps(amount)
            .map_err(|error| super::authority::operation_error(&error))
    }

    /// Charges canonical ordering before native sorting.
    /// # Errors
    /// Rejects checked arithmetic, exhausted cumulative work or interruption.
    pub fn sort_work(&mut self, length: usize) -> Result<(), eutheto_domain_api::DomainPackError> {
        self.inner
            .sort_work(length)
            .map_err(|error| super::authority::operation_error(&error))
    }

    /// Checks the retained caller control without creating another allowance.
    /// # Errors
    /// Rejects cancellation or an expired deadline.
    pub fn check(&self) -> Result<(), eutheto_domain_api::DomainPackError> {
        self.inner
            .check()
            .map_err(|error| super::authority::operation_error(&error))
    }

    /// Hashes complete canonical pair content using the remaining allowance.
    /// # Errors
    /// Rejects duplicates, input/resource bounds or interruption.
    pub fn pair_content_hash(
        &mut self,
        pairs: &[crate::model::AssignmentPair],
    ) -> Result<String, eutheto_domain_api::DomainPackError> {
        super::baseline::pair_content_hash(pairs, &mut self.inner)
    }

    /// Compares effective `BaseSchedule` identity, accepted source and complete pair content.
    /// # Errors
    /// Rejects malformed/multiple bases, input/resource bounds or interruption.
    pub fn effective_base_changed(
        &mut self,
        original: &eutheto_types::ScenarioDocument,
        prospective: &eutheto_types::ScenarioDocument,
    ) -> Result<bool, eutheto_domain_api::DomainPackError> {
        super::baseline::effective_base_changed(original, prospective, &mut self.inner)
    }

    /// Computes integrity only, never a local admission receipt.
    /// # Errors
    /// Rejects input/resource bounds or interruption.
    pub fn manual_selection_checksum(
        &mut self,
        record: &crate::model::ManualSelection,
    ) -> Result<String, eutheto_domain_api::DomainPackError> {
        super::baseline::manual_checksum(record, &mut self.inner)
    }

    /// Resolves current generated, detached and manual work through original-source temporal
    /// authority, charging validation and the complete catalogue to this inherited allowance.
    ///
    /// # Errors
    /// Rejects malformed source, stale occurrence identities, temporal/resource bounds or interruption.
    pub fn resolved_shifts(
        &mut self,
        document: &eutheto_types::ScenarioDocument,
    ) -> Result<Vec<crate::temporal::ResolvedShift>, eutheto_domain_api::DomainPackError> {
        let prepared = super::authority::prepare_source(document, &mut self.inner)?;
        Ok(prepared.shifts)
    }

    /// Evaluates complete Required outcomes; the native owner must reject unsatisfied outcomes.
    /// # Errors
    /// Rejects malformed selections, unsupported/incomplete obligations, bounds or interruption.
    pub fn evaluate_required_selection(
        &mut self,
        document: &eutheto_types::ScenarioDocument,
        pairs: &[crate::model::AssignmentPair],
    ) -> Result<super::AssignmentRuleEvaluation, eutheto_domain_api::DomainPackError> {
        super::authority::evaluate_required_with_budget(document, pairs, &mut self.inner)
    }

    /// Resolves exact bound pairs, borrowing snapshot data or owning one captured decode.
    /// # Errors
    /// Rejects content/source/checksum mismatches, bounds or interruption.
    pub fn baseline_pairs<'d>(
        &mut self,
        domain: &'d crate::model::WorkforceDomainV1,
        scenario_id: eutheto_types::ScenarioId,
        binding: &eutheto_domain_ir::RunBaselineBinding,
    ) -> Result<
        std::borrow::Cow<'d, [crate::model::AssignmentPair]>,
        eutheto_domain_api::DomainPackError,
    > {
        super::baseline::standalone_baseline_pairs(domain, scenario_id, binding, &mut self.inner)
    }
    /// Derives complete source-owned descriptive outcomes under this cumulative allowance.
    /// # Errors
    /// Rejects source/recorded-score disagreement, incomplete census, bounds or interruption.
    pub fn alternative_outcomes(
        &mut self,
        document: &eutheto_types::ScenarioDocument,
        accepted: &eutheto_domain_ir::AcceptedResult,
        objective_context: &eutheto_domain_ir::RunObjectiveContext,
    ) -> Result<super::WorkforceAlternativeOutcomes, eutheto_domain_api::DomainPackError> {
        super::alternative_outcomes::extract(document, accepted, objective_context, &mut self.inner)
    }

    /// Establishes exact local equivalence, never acceptance or imported provenance.
    /// # Errors
    /// Rejects exhausted cumulative work or interruption; never proves partial equality.
    pub fn same_alternative_outcomes(
        &mut self,
        left: &super::WorkforceAlternativeOutcomes,
        right: &super::WorkforceAlternativeOutcomes,
    ) -> Result<bool, eutheto_domain_api::DomainPackError> {
        super::alternative_outcomes::same(left, right, &mut self.inner)
    }
}

/// Complete compilation and projection never expand the supported generic-operation ceilings.
pub(super) fn effective_limits(
    limits: PlanningIrLimitsV1,
) -> Result<PlanningIrLimitsV1, AssignmentRuleError> {
    if limits.max_abs_coefficient < 0 || limits.max_abs_value < 0 {
        return Err(AssignmentRuleError::InvalidConstruction(
            AssignmentConstructionIssue::InvalidRecord,
        ));
    }
    let defaults = PlanningIrLimitsV1::DEFAULT;
    Ok(PlanningIrLimitsV1 {
        max_ir_bytes: limits.max_ir_bytes.min(defaults.max_ir_bytes),
        max_variables: limits.max_variables.min(defaults.max_variables),
        max_constraints: limits.max_constraints.min(defaults.max_constraints),
        max_assumptions: limits.max_assumptions.min(defaults.max_assumptions),
        max_objective_levels: limits
            .max_objective_levels
            .min(defaults.max_objective_levels),
        max_objective_terms: limits.max_objective_terms.min(defaults.max_objective_terms),
        max_provenance_records: limits
            .max_provenance_records
            .min(defaults.max_provenance_records),
        max_provenance_depth: limits
            .max_provenance_depth
            .min(defaults.max_provenance_depth),
        max_parameters_per_record: limits
            .max_parameters_per_record
            .min(defaults.max_parameters_per_record),
        max_parameter_text_bytes: limits
            .max_parameter_text_bytes
            .min(defaults.max_parameter_text_bytes),
        max_entity_refs_per_record: limits
            .max_entity_refs_per_record
            .min(defaults.max_entity_refs_per_record),
        max_projections: limits.max_projections.min(defaults.max_projections),
        max_projection_expression_depth: limits
            .max_projection_expression_depth
            .min(defaults.max_projection_expression_depth),
        max_domain_ranges: limits.max_domain_ranges.min(defaults.max_domain_ranges),
        max_refs_per_node: limits.max_refs_per_node.min(defaults.max_refs_per_node),
        max_total_refs: limits.max_total_refs.min(defaults.max_total_refs),
        max_table_rows: limits.max_table_rows.min(defaults.max_table_rows),
        max_table_arity: limits.max_table_arity.min(defaults.max_table_arity),
        max_table_cells: limits.max_table_cells.min(defaults.max_table_cells),
        max_intervals_per_global: limits
            .max_intervals_per_global
            .min(defaults.max_intervals_per_global),
        max_enforcement_literals: limits
            .max_enforcement_literals
            .min(defaults.max_enforcement_literals),
        max_tags: limits.max_tags.min(defaults.max_tags),
        max_component_nodes: limits.max_component_nodes.min(defaults.max_component_nodes),
        max_component_edges: limits.max_component_edges.min(defaults.max_component_edges),
        max_id_bytes: limits.max_id_bytes.min(defaults.max_id_bytes),
        max_metadata_text_bytes: limits
            .max_metadata_text_bytes
            .min(defaults.max_metadata_text_bytes),
        max_abs_coefficient: limits.max_abs_coefficient.min(defaults.max_abs_coefficient),
        max_abs_value: limits.max_abs_value.min(defaults.max_abs_value),
    })
}

pub(super) fn count(value: usize) -> Result<u64, AssignmentRuleError> {
    u64::try_from(value).map_err(|_| arithmetic())
}

pub(super) fn add(left: u64, right: u64) -> Result<u64, AssignmentRuleError> {
    left.checked_add(right).ok_or_else(arithmetic)
}

pub(super) fn within(
    value: u64,
    limit: u64,
    kind: AssignmentRuleLimit,
) -> Result<(), AssignmentRuleError> {
    if value > limit {
        Err(AssignmentRuleError::LimitExceeded(kind))
    } else {
        Ok(())
    }
}

fn arithmetic() -> AssignmentRuleError {
    AssignmentRuleError::InvalidConstruction(AssignmentConstructionIssue::ArithmeticOverflow)
}

fn reserve(
    usage: &mut Usage,
    records: u64,
    items: u64,
    bytes: u64,
    max_records: u64,
    max_items: u64,
    max_bytes: u64,
) -> Result<(), AssignmentRuleError> {
    let records = add(usage.records, records)?;
    let items = add(usage.items, items)?;
    let bytes = add(usage.bytes, bytes)?;
    within(records, max_records, AssignmentRuleLimit::Records)?;
    within(items, max_items, AssignmentRuleLimit::References)?;
    within(bytes, max_bytes, AssignmentRuleLimit::Bytes)?;
    *usage = Usage {
        records,
        items,
        bytes,
    };
    Ok(())
}

struct CountingWriter<'a> {
    bytes: u64,
    limit: u64,
    control: Option<&'a OperationControl>,
    failure: Option<AssignmentRuleError>,
}

impl Write for CountingWriter<'_> {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let next = self
            .control
            .map_or(Ok(()), OperationControl::check)
            .map_err(interruption)
            .and_then(|()| count(buffer.len()))
            .and_then(|length| add(self.bytes, length))
            .and_then(|bytes| {
                within(bytes, self.limit, AssignmentRuleLimit::Bytes)?;
                Ok(bytes)
            });
        match next {
            Ok(bytes) => {
                self.bytes = bytes;
                Ok(buffer.len())
            }
            Err(error) => {
                self.failure = Some(error);
                // Interrupted would make write_all retry a permanently cancelled operation.
                Err(io::Error::other("bounded Workforce operation stopped"))
            }
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn interruption(reason: OperationInterruption) -> AssignmentRuleError {
    match reason {
        OperationInterruption::Cancelled => AssignmentRuleError::Cancelled,
        OperationInterruption::DeadlineExceeded => AssignmentRuleError::BudgetExpired,
    }
}
