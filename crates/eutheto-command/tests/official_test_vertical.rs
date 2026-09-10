use eutheto_command::{
    CommandError, OFFICIAL_TEST_PACK_ID, OfficialTestPack, apply_command_with_registry,
};
use eutheto_domain_api::{
    CompileContext, CounterfactualCompileContext, DomainBatchCommand, DomainCatalog,
    DomainMutation, DomainPack, DomainPackDescriptor, DomainPackError, DomainPackRegistry,
    DomainShareResult, DomainValidationReport, MAX_DOMAIN_MUTATION_CHANGES, PortableImportContext,
    ShareResultOptions,
};
use eutheto_domain_ir::{
    AcceptedResult, AssignmentValue, CounterfactualConditionV1, DomainAssignmentId, DomainEntityId,
    DomainEntityKindId, DomainEntityRef, DomainEvidenceId, EvidenceRenderRequestV1,
    EvidenceRenderResultV1, ExplanationCapability, NormalizedSolution, OptimizationDirection,
    RequiredRuleBinding, RuleEvaluation, ScoreCategoryId, ScoreLevelId, ScoreLevelValue,
    ScoreVector, VerificationContextV1, VerificationFactId, VerificationReport, VerificationScope,
    VerificationValue, blake3_hex,
};
use eutheto_planning_ir::{
    BoolVariable, BoolVariableId, CandidateValues, Capability, ComparisonOp, CompilerId,
    Constraint, ConstraintRecord, ConstraintTag, InclusiveRange, IntDomain, IntVariable,
    IntVariableId, IntervalVariable, IntervalVariableId, LinearComparison, LinearExpression,
    LinearTerm, Literal, MetadataKey, ObjectiveLevel, ObjectiveLevelId, ObjectivePlan,
    ObjectiveTerm, ObjectiveTermId, ObjectiveTermKind, PLANNING_IR_SCHEMA_VERSION,
    PROJECTION_SCHEMA_VERSION, PlanningConstraintId, PlanningIrLimitsV1, PlanningMetadata,
    PlanningProblem, ProjectionExpression, ProjectionId, ProvenanceId, ProvenanceParameter,
    ProvenanceRecord, ProvenanceSourceKind, SolutionProjection, Variable, canonical_ir_hash,
    feature_usage, summarize, validate,
};
use eutheto_types::{
    ActorRef, CancellationToken, CommandEnvelope, CommandSource, DomainCommandEnvelope,
    PortableDomainDocument, Revision, RuleId, ScenarioDocument, SolutionId,
};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::str::FromStr;

const SCENARIO_ID: &str = "0195a5e4-7c00-7000-8000-000000000011";
const SOLUTION_ID: &str = "0195a5e4-7c00-7000-8000-000000000012";
const FIXTURE_ENTITY_KIND: &str = "official.test.fixture.option";
const FIXTURE_SCORE_LEVEL: &str = "official.test.fixture.score.selected";
const FIXTURE_SCORE_CATEGORY: &str = "official.test.fixture.score.selected-total";
const MAX_FIXTURE_OPTIONS: u32 = 8;
const FIXTURE_RULE_EARLY: &str = "0195a5e4-7c00-7000-8000-000000000101";
const FIXTURE_RULE_OVERLAP: &str = "0195a5e4-7c00-7000-8000-000000000102";
const FIXTURE_RULE_INELIGIBLE: &str = "0195a5e4-7c00-7000-8000-000000000103";
const FIXTURE_RULE_NO_OVERLAP: &str = "0195a5e4-7c00-7000-8000-000000000104";
const FIXTURE_RULE_CUMULATIVE: &str = "0195a5e4-7c00-7000-8000-000000000105";

#[derive(Clone, Copy, Debug)]
struct FixtureOption {
    key: &'static str,
    start: i64,
    duration: i64,
    eligible: bool,
}

impl FixtureOption {
    fn end(self) -> i64 {
        self.start + self.duration
    }
}

const OPTIONS: [FixtureOption; 3] = [
    FixtureOption {
        key: "early",
        start: 0,
        duration: 2,
        eligible: true,
    },
    FixtureOption {
        key: "overlap",
        start: 1,
        duration: 2,
        eligible: true,
    },
    FixtureOption {
        key: "ineligible",
        start: 3,
        duration: 1,
        eligible: false,
    },
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ModelEstimate {
    variables: u32,
    constraints: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PruningSummary {
    candidates_before: u32,
    candidates_after: u32,
    model_before: ModelEstimate,
    model_after: ModelEstimate,
}

impl PruningSummary {
    fn new(before: &[FixtureOption], after: &[FixtureOption]) -> Result<Self, DomainPackError> {
        let candidates_before = bounded_count(before.len())?;
        let candidates_after = bounded_count(after.len())?;
        if candidates_before == 0 || candidates_after == 0 || candidates_after > candidates_before {
            return Err(contract("invalid synthetic pruning counts"));
        }
        Ok(Self {
            candidates_before,
            candidates_after,
            model_before: estimate(before)?,
            model_after: estimate(after)?,
        })
    }

    fn metadata(self) -> Result<BTreeMap<MetadataKey, ProvenanceParameter>, DomainPackError> {
        let entries = [
            (
                "official_test.pruning.candidates_before",
                self.candidates_before,
            ),
            (
                "official_test.pruning.candidates_after",
                self.candidates_after,
            ),
            (
                "official_test.pruning.model_variables_before",
                self.model_before.variables,
            ),
            (
                "official_test.pruning.model_variables_after",
                self.model_after.variables,
            ),
            (
                "official_test.pruning.model_constraints_before",
                self.model_before.constraints,
            ),
            (
                "official_test.pruning.model_constraints_after",
                self.model_after.constraints,
            ),
        ];
        entries
            .into_iter()
            .map(|(key, value)| {
                Ok((
                    MetadataKey::new(key).map_err(contract)?,
                    ProvenanceParameter::Integer(i64::from(value)),
                ))
            })
            .collect()
    }

    fn read(problem: &PlanningProblem) -> Result<Self, DomainPackError> {
        Ok(Self {
            candidates_before: metadata_count(problem, "official_test.pruning.candidates_before")?,
            candidates_after: metadata_count(problem, "official_test.pruning.candidates_after")?,
            model_before: ModelEstimate {
                variables: metadata_count(problem, "official_test.pruning.model_variables_before")?,
                constraints: metadata_count(
                    problem,
                    "official_test.pruning.model_constraints_before",
                )?,
            },
            model_after: ModelEstimate {
                variables: metadata_count(problem, "official_test.pruning.model_variables_after")?,
                constraints: metadata_count(
                    problem,
                    "official_test.pruning.model_constraints_after",
                )?,
            },
        })
    }
}

fn bounded_count(count: usize) -> Result<u32, DomainPackError> {
    let count = u32::try_from(count).map_err(contract)?;
    if count > MAX_FIXTURE_OPTIONS {
        return Err(contract("synthetic option corpus exceeds its fixed bound"));
    }
    Ok(count)
}

fn estimate(options: &[FixtureOption]) -> Result<ModelEstimate, DomainPackError> {
    let count = bounded_count(options.len())?;
    let ineligible = bounded_count(options.iter().filter(|option| !option.eligible).count())?;
    Ok(ModelEstimate {
        variables: count
            .checked_mul(6)
            .ok_or_else(|| contract("synthetic variable estimate overflow"))?,
        constraints: count
            .checked_mul(5)
            .and_then(|value| value.checked_add(2))
            .and_then(|value| value.checked_add(ineligible))
            .ok_or_else(|| contract("synthetic constraint estimate overflow"))?,
    })
}

fn metadata_count(problem: &PlanningProblem, key: &str) -> Result<u32, DomainPackError> {
    let key = MetadataKey::new(key).map_err(contract)?;
    let Some(ProvenanceParameter::Integer(value)) = problem.metadata.compile_metadata.get(&key)
    else {
        return Err(contract("missing typed pruning metadata"));
    };
    u32::try_from(*value).map_err(contract)
}

type MutationOverride = fn(&mut DomainMutation, &CancellationToken) -> Result<(), DomainPackError>;

#[derive(Clone, Copy)]
struct IntervalFixturePack {
    prune: bool,
    mutation_override: Option<MutationOverride>,
    large_values: bool,
}

impl IntervalFixturePack {
    fn with_pruning(prune: bool) -> Self {
        Self {
            prune,
            mutation_override: None,
            large_values: false,
        }
    }
}

impl DomainPack for IntervalFixturePack {
    fn descriptor(&self) -> Result<DomainPackDescriptor, DomainPackError> {
        let mut descriptor = OfficialTestPack.descriptor()?;
        descriptor.explanation_capabilities.clear();
        Ok(descriptor)
    }

    fn catalog(&self) -> Result<DomainCatalog, DomainPackError> {
        let mut catalog = OfficialTestPack.catalog()?;
        if self.large_values {
            let notes = json!({"type": "array", "items": {"type": "string"}});
            catalog.commands[0].payload_schema["properties"]["notes"] = notes.clone();
            catalog.commands[0].result_schema["properties"]["notes"] = notes;
            for tool in &mut catalog.ai_tools {
                if tool.command_id == catalog.commands[0].id {
                    tool.input_schema
                        .clone_from(&catalog.commands[0].payload_schema);
                }
            }
        }
        Ok(catalog)
    }

    fn new_document(&self, shell: ScenarioDocument) -> Result<ScenarioDocument, DomainPackError> {
        OfficialTestPack.new_document(shell)
    }

    fn migrate_document(
        &self,
        document: ScenarioDocument,
    ) -> Result<ScenarioDocument, DomainPackError> {
        OfficialTestPack.migrate_document(document)
    }

    fn validate_fast(&self, document: &ScenarioDocument) -> DomainValidationReport {
        OfficialTestPack.validate_fast(document)
    }

    fn validate_full(
        &self,
        document: &ScenarioDocument,
        control: &eutheto_types::OperationControl,
    ) -> Result<DomainValidationReport, DomainPackError> {
        OfficialTestPack.validate_full(document, control)
    }

    fn reconcile_settings(
        &self,
        document: &ScenarioDocument,
        settings: &eutheto_types::ScenarioSettings,
        restoration: Option<&serde_json::Value>,
        control: &eutheto_types::OperationControl,
    ) -> Result<eutheto_domain_api::DomainSettingsMutation, DomainPackError> {
        OfficialTestPack.reconcile_settings(document, settings, restoration, control)
    }

    fn apply_batch(
        &self,
        document: &ScenarioDocument,
        batch: &DomainBatchCommand,
        cancellation: &CancellationToken,
    ) -> Result<DomainMutation, DomainPackError> {
        batch.validate_bounds()?;
        let Some(mutate) = self.mutation_override else {
            return OfficialTestPack.apply_batch(document, batch, cancellation);
        };
        // Fault injection ignores input so host validation is independently exercised.
        // Large-value fixtures instead preserve semantics, stripping only auxiliary notes.
        let mut canonical = batch.clone();
        for command in &mut canonical.commands {
            if self.large_values {
                command
                    .payload
                    .as_object_mut()
                    .ok_or_else(|| contract("payload"))?
                    .remove("notes");
            } else {
                "official.test.configure_entity".clone_into(&mut command.command_type);
                command.payload = json!({
                    "entityId": MUTATION_ENTITY_ID, "enabled": true, "target": 3,
                });
            }
        }
        let mut mutation = OfficialTestPack.apply_batch(document, &canonical, cancellation)?;
        mutate(&mut mutation, cancellation)?;
        Ok(mutation)
    }

    fn compile(
        &self,
        document: &ScenarioDocument,
        context: &CompileContext,
    ) -> Result<PlanningProblem, DomainPackError> {
        build_interval_problem(document, context, self.prune)
    }

    fn project(
        &self,
        problem: &PlanningProblem,
        candidate: &CandidateValues,
        solution_id: SolutionId,
        control: &eutheto_types::OperationControl,
    ) -> Result<NormalizedSolution, DomainPackError> {
        control.check()?;
        OfficialTestPack.project(problem, candidate, solution_id, control)
    }

    fn verification_scope(
        &self,
        document: &ScenarioDocument,
        scenario_revision: u64,
        control: &eutheto_types::OperationControl,
    ) -> Result<VerificationScope, DomainPackError> {
        control.check()?;
        if document.domain_pack.id.as_str() != OFFICIAL_TEST_PACK_ID {
            return Err(contract("optional fixture scenario pack mismatch"));
        }
        VerificationScope::new(
            document.scenario_id,
            scenario_revision,
            fixture_rule_bindings()?,
        )
        .map_err(contract)
    }

    fn verify(
        &self,
        document: &ScenarioDocument,
        solution: &NormalizedSolution,
        context: &VerificationContextV1,
        authoritative_score: &ScoreVector,
        control: &eutheto_types::OperationControl,
    ) -> Result<VerificationReport, DomainPackError> {
        control.check()?;
        validate_interval_verification_context(*self, document, solution, context)?;
        VerificationReport::new(
            context,
            evaluate_interval_rules(document, solution)?,
            authoritative_score.clone(),
            Vec::new(),
            BTreeMap::new(),
        )
        .map_err(contract)
    }

    fn score(
        &self,
        document: &ScenarioDocument,
        solution: &NormalizedSolution,
        control: &eutheto_types::OperationControl,
    ) -> Result<ScoreVector, DomainPackError> {
        control.check()?;
        authoritative_interval_score(document, solution)
    }

    fn export_portable(
        &self,
        document: &ScenarioDocument,
    ) -> Result<PortableDomainDocument, DomainPackError> {
        OfficialTestPack.export_portable(document)
    }

    fn migrate_portable_step(
        &self,
        document: PortableDomainDocument,
    ) -> Result<PortableDomainDocument, DomainPackError> {
        OfficialTestPack.migrate_portable_step(document)
    }

    fn import_portable(
        &self,
        document: &PortableDomainDocument,
        context: &PortableImportContext,
    ) -> Result<ScenarioDocument, DomainPackError> {
        OfficialTestPack.import_portable(document, context)
    }

    fn build_share_result(
        &self,
        document: &ScenarioDocument,
        accepted: &AcceptedResult,
        options: ShareResultOptions,
    ) -> Result<DomainShareResult, DomainPackError> {
        OfficialTestPack.build_share_result(document, accepted, options)
    }

    fn build_view(
        &self,
        input: eutheto_domain_api::DomainViewInput<'_>,
        control: &eutheto_types::OperationControl,
    ) -> Result<eutheto_domain_api::DomainViewOutput, DomainPackError> {
        OfficialTestPack.build_view(input, control)
    }

    fn render_evidence(
        &self,
        _document: &ScenarioDocument,
        request: &EvidenceRenderRequestV1,
    ) -> Result<EvidenceRenderResultV1, DomainPackError> {
        Err(DomainPackError::UnsupportedExplanationCapability(
            explanation_capability(request.kind),
        ))
    }

    fn compile_counterfactual(
        &self,
        _document: &ScenarioDocument,
        _condition: &CounterfactualConditionV1,
        _context: &CounterfactualCompileContext<'_>,
    ) -> Result<PlanningProblem, DomainPackError> {
        Err(DomainPackError::UnsupportedExplanationCapability(
            ExplanationCapability::Counterfactual,
        ))
    }
}

// This synthetic IR fixture is intentionally kept in one place so the variable,
// constraint, projection, and provenance correspondence remains auditable.
#[allow(clippy::too_many_lines)]
fn build_interval_problem(
    document: &ScenarioDocument,
    context: &CompileContext,
    prune: bool,
) -> Result<PlanningProblem, DomainPackError> {
    context.control.check()?;
    if document.domain_pack.id.as_str() != OFFICIAL_TEST_PACK_ID
        || document.domain_pack.schema_version != 1
    {
        return Err(contract("optional fixture document pack mismatch"));
    }

    let included: Vec<_> = OPTIONS
        .iter()
        .copied()
        .filter(|option| !prune || option.eligible)
        .collect();
    let pruning = PruningSummary::new(&OPTIONS, &included)?;
    let global_rule = ProvenanceId::new("official_test.fixture.rule.global").map_err(contract)?;
    let preference =
        ProvenanceId::new("official_test.fixture.preference.selected").map_err(contract)?;
    let mut variables = Vec::with_capacity(included.len() * 6);
    let mut constraints = Vec::with_capacity(pruning.model_after.constraints as usize);
    let mut objective_terms = Vec::with_capacity(included.len());
    let mut projections = Vec::with_capacity(OPTIONS.len());
    let mut provenance = Vec::with_capacity(OPTIONS.len() * 3 + 2);
    let mut interval_ids = Vec::with_capacity(included.len());

    for option in OPTIONS {
        let entity = fixture_entity(option)?;
        let fact = fixture_provenance("fact", option)?;
        let eligibility_rule = fixture_provenance("eligibility", option)?;
        let projection = fixture_provenance("projection", option)?;
        provenance.extend([
            provenance_record(
                fact.clone(),
                ProvenanceSourceKind::Fact,
                format!("official.test.fixture.option.{}", option.key),
                entity.clone(),
                None,
            ),
            provenance_record(
                eligibility_rule.clone(),
                ProvenanceSourceKind::RequiredRule,
                format!("official.test.fixture.eligibility.{}", option.key),
                entity.clone(),
                Some(fact.clone()),
            ),
            provenance_record(
                projection.clone(),
                ProvenanceSourceKind::Projection,
                format!("official.test.fixture.projection.{}", option.key),
                entity.clone(),
                Some(eligibility_rule.clone()),
            ),
        ]);

        if !included.iter().any(|included| included.key == option.key) {
            projections.push(SolutionProjection {
                id: fixture_projection_id(option)?,
                assignment_id: fixture_assignment_id(option)?,
                entity,
                required: true,
                expression: ProjectionExpression::Constant(AssignmentValue::Absent),
                provenance: projection,
            });
            continue;
        }

        let present = fixture_bool_id(option)?;
        let start = fixture_int_id(option, "start")?;
        let duration = fixture_int_id(option, "duration")?;
        let end = fixture_int_id(option, "end")?;
        let cost = fixture_int_id(option, "cost")?;
        let interval = fixture_interval_id(option)?;
        variables.extend([
            Variable::Boolean(BoolVariable {
                id: present.clone(),
                provenance: fact.clone(),
            }),
            Variable::Integer(IntVariable {
                id: start.clone(),
                domain: IntDomain::new(vec![InclusiveRange { start: 0, end: 4 }])
                    .map_err(contract)?,
                provenance: fact.clone(),
            }),
            Variable::Integer(IntVariable {
                id: duration.clone(),
                domain: IntDomain::new(vec![InclusiveRange { start: 1, end: 2 }])
                    .map_err(contract)?,
                provenance: fact.clone(),
            }),
            Variable::Integer(IntVariable {
                id: end.clone(),
                domain: IntDomain::new(vec![InclusiveRange { start: 1, end: 5 }])
                    .map_err(contract)?,
                provenance: fact.clone(),
            }),
            Variable::Integer(IntVariable {
                id: cost.clone(),
                domain: IntDomain::new(vec![InclusiveRange { start: 0, end: 1 }])
                    .map_err(contract)?,
                provenance: fact.clone(),
            }),
            Variable::Interval(IntervalVariable {
                id: interval.clone(),
                start: start.clone(),
                duration: duration.clone(),
                end: end.clone(),
                presence: Some(Literal::positive(present.clone())),
                provenance: fact.clone(),
            }),
        ]);
        interval_ids.push(interval.clone());
        constraints.extend([
            fixed_integer_constraint(option, "start", start, option.start, &present, true, &fact)?,
            fixed_integer_constraint(
                option,
                "duration",
                duration,
                option.duration,
                &present,
                true,
                &fact,
            )?,
            fixed_integer_constraint(option, "end", end, option.end(), &present, true, &fact)?,
            fixed_integer_constraint(
                option,
                "selected_cost",
                cost.clone(),
                1,
                &present,
                true,
                &fact,
            )?,
            fixed_integer_constraint(
                option,
                "absent_cost",
                cost.clone(),
                0,
                &present,
                false,
                &fact,
            )?,
        ]);
        if !option.eligible {
            constraints.push(ConstraintRecord {
                id: PlanningConstraintId::new(format!(
                    "official_test.fixture.required_eligibility.{}",
                    option.key
                ))
                .map_err(contract)?,
                body: Constraint::bool_and(vec![Literal::negative(present.clone())]),
                enforcement: Vec::new(),
                provenance: eligibility_rule,
                tags: vec![ConstraintTag::new("official_test.fixture.required").map_err(contract)?],
            });
        }
        objective_terms.push(ObjectiveTerm {
            id: ObjectiveTermId::new(format!(
                "official_test.fixture.objective.selected.{}",
                option.key
            ))
            .map_err(contract)?,
            expression: LinearExpression::new(
                vec![LinearTerm {
                    variable: cost,
                    coefficient: 1,
                }],
                0,
            )
            .map_err(contract)?,
            kind: ObjectiveTermKind::Penalty,
            category: ScoreCategoryId::new(FIXTURE_SCORE_CATEGORY).map_err(contract)?,
            provenance: preference.clone(),
        });
        projections.push(SolutionProjection {
            id: fixture_projection_id(option)?,
            assignment_id: fixture_assignment_id(option)?,
            entity,
            required: true,
            expression: ProjectionExpression::Interval(interval),
            provenance: projection,
        });
    }

    provenance.extend([
        ProvenanceRecord {
            id: global_rule.clone(),
            source_kind: ProvenanceSourceKind::RequiredRule,
            source_id: "official.test.fixture.interval-capacity".to_owned(),
            entity_refs: OPTIONS
                .into_iter()
                .map(fixture_entity)
                .collect::<Result<Vec<_>, _>>()?,
            message_key: "official.test.fixture.provenance.interval_capacity".to_owned(),
            parameters: BTreeMap::new(),
            parent: None,
        },
        ProvenanceRecord {
            id: preference.clone(),
            source_kind: ProvenanceSourceKind::Preference,
            source_id: "official.test.fixture.preference.selected".to_owned(),
            entity_refs: Vec::new(),
            message_key: "official.test.fixture.provenance.selected_preference".to_owned(),
            parameters: BTreeMap::new(),
            parent: None,
        },
    ]);
    constraints.extend([
        ConstraintRecord {
            id: PlanningConstraintId::new("official_test.fixture.no_overlap").map_err(contract)?,
            body: Constraint::no_overlap(interval_ids.clone()),
            enforcement: Vec::new(),
            provenance: global_rule.clone(),
            tags: vec![ConstraintTag::new("official_test.fixture.required").map_err(contract)?],
        },
        ConstraintRecord {
            id: PlanningConstraintId::new("official_test.fixture.cumulative").map_err(contract)?,
            body: Constraint::cumulative(interval_ids.clone(), vec![1; interval_ids.len()], 1)
                .map_err(contract)?,
            enforcement: Vec::new(),
            provenance: global_rule,
            tags: vec![ConstraintTag::new("official_test.fixture.required").map_err(contract)?],
        },
    ]);

    let mut problem = PlanningProblem {
        schema_version: PLANNING_IR_SCHEMA_VERSION,
        variables,
        constraints,
        objectives: ObjectivePlan {
            levels: vec![ObjectiveLevel {
                id: ObjectiveLevelId::new(FIXTURE_SCORE_LEVEL).map_err(contract)?,
                direction: OptimizationDirection::Minimize,
                lower_bound: 0,
                upper_bound: i64::from(pruning.candidates_after),
                terms: objective_terms,
                provenance: preference,
            }],
        },
        assumptions: Vec::new(),
        projections,
        provenance,
        metadata: PlanningMetadata {
            pack_id: document.domain_pack.id.clone(),
            scenario_id: document.scenario_id,
            scenario_revision: context.scenario_revision,
            projection_version: PROJECTION_SCHEMA_VERSION,
            compiler_id: CompilerId::new("official_test.fixture.interval_compiler")
                .map_err(contract)?,
            compiler_version: "1.0.0".to_owned(),
            compile_metadata: pruning.metadata()?,
            display_text: BTreeMap::new(),
        },
        declared_capabilities: BTreeSet::new(),
        split_authorization: None,
    };
    problem.declared_capabilities = feature_usage(&problem).required_capabilities();
    problem.canonicalize().map_err(contract)?;
    validate(&problem, context.planning_limits).map_err(contract)?;
    Ok(problem)
}

fn fixed_integer_constraint(
    option: FixtureOption,
    label: &str,
    variable: IntVariableId,
    value: i64,
    present: &BoolVariableId,
    when_present: bool,
    provenance: &ProvenanceId,
) -> Result<ConstraintRecord, DomainPackError> {
    Ok(ConstraintRecord {
        id: PlanningConstraintId::new(format!(
            "official_test.fixture.fixed_{label}.{}",
            option.key
        ))
        .map_err(contract)?,
        body: Constraint::LinearComparison(LinearComparison {
            expression: LinearExpression::new(
                vec![LinearTerm {
                    variable,
                    coefficient: 1,
                }],
                0,
            )
            .map_err(contract)?,
            op: ComparisonOp::Equal,
            rhs: value,
        }),
        enforcement: vec![if when_present {
            Literal::positive(present.clone())
        } else {
            Literal::negative(present.clone())
        }],
        provenance: provenance.clone(),
        tags: vec![ConstraintTag::new("official_test.fixture.required").map_err(contract)?],
    })
}

fn fixture_rule_bindings() -> Result<Vec<RequiredRuleBinding>, DomainPackError> {
    let mut bindings = OPTIONS
        .into_iter()
        .map(|option| {
            Ok(RequiredRuleBinding {
                rule_id: fixture_rule_id(option)?,
                semantic_hash: blake3_hex(
                    format!(
                        "official.test.fixture.option/v1;key={};start={};duration={};eligible={}",
                        option.key, option.start, option.duration, option.eligible
                    )
                    .as_bytes(),
                ),
            })
        })
        .collect::<Result<Vec<_>, DomainPackError>>()?;
    bindings.extend([
        RequiredRuleBinding {
            rule_id: RuleId::from_str(FIXTURE_RULE_NO_OVERLAP).map_err(contract)?,
            semantic_hash: blake3_hex(
                b"official.test.fixture.no-overlap/v1;capacity=1;options=early,ineligible,overlap",
            ),
        },
        RequiredRuleBinding {
            rule_id: RuleId::from_str(FIXTURE_RULE_CUMULATIVE).map_err(contract)?,
            semantic_hash: blake3_hex(
                b"official.test.fixture.cumulative/v1;capacity=1;horizon=0..5;options=early,ineligible,overlap",
            ),
        },
    ]);
    Ok(bindings)
}

fn fixture_rule_id(option: FixtureOption) -> Result<RuleId, DomainPackError> {
    RuleId::from_str(match option.key {
        "early" => FIXTURE_RULE_EARLY,
        "overlap" => FIXTURE_RULE_OVERLAP,
        "ineligible" => FIXTURE_RULE_INELIGIBLE,
        _ => return Err(contract("unknown optional fixture rule")),
    })
    .map_err(contract)
}

fn validate_interval_verification_context(
    pack: IntervalFixturePack,
    document: &ScenarioDocument,
    solution: &NormalizedSolution,
    context: &VerificationContextV1,
) -> Result<(), DomainPackError> {
    context.validate().map_err(contract)?;
    let document_hash =
        blake3_hex(&serde_json::to_vec(document).map_err(|error| contract(error.to_string()))?);
    let scope = pack.verification_scope(
        document,
        solution.scenario_revision,
        &eutheto_types::OperationControl::Cancellation(CancellationToken::new()),
    )?;
    if context.scenario_id != document.scenario_id
        || context.evaluated_revision != solution.scenario_revision
        || context.document_hash != document_hash
        || context.normalized_solution_hash != solution.canonical_hash().map_err(contract)?
        || context.verification_scope_checksum != scope.checksum
    {
        return Err(contract(
            "optional fixture verification context binding mismatch",
        ));
    }
    Ok(())
}

// Keep the independent rule evaluator contiguous so its facts and aggregate capacity checks remain
// auditable against the fixture's domain semantics.
#[allow(clippy::too_many_lines)]
fn evaluate_interval_rules(
    document: &ScenarioDocument,
    solution: &NormalizedSolution,
) -> Result<Vec<RuleEvaluation>, DomainPackError> {
    if document.domain_pack.id.as_str() != OFFICIAL_TEST_PACK_ID
        || solution.pack_id != document.domain_pack.id
        || solution.scenario_id != document.scenario_id
    {
        return Err(contract("optional fixture solution scenario mismatch"));
    }
    solution.validate().map_err(contract)?;
    let expected_ids: BTreeSet<_> = OPTIONS
        .into_iter()
        .map(fixture_assignment_id)
        .collect::<Result<_, _>>()?;
    if solution.assignments.len() != expected_ids.len()
        || solution
            .assignments
            .iter()
            .any(|assignment| !expected_ids.contains(&assignment.id))
    {
        return Err(contract("optional fixture assignment set mismatch"));
    }

    let assignments: BTreeMap<_, _> = solution
        .assignments
        .iter()
        .map(|assignment| (assignment.id.clone(), assignment))
        .collect();
    let eligible_fact =
        VerificationFactId::new("official.test.fixture.fact.eligible").map_err(contract)?;
    let present_fact =
        VerificationFactId::new("official.test.fixture.fact.present").map_err(contract)?;
    let start_fact =
        VerificationFactId::new("official.test.fixture.fact.start").map_err(contract)?;
    let duration_fact =
        VerificationFactId::new("official.test.fixture.fact.duration").map_err(contract)?;
    let end_fact = VerificationFactId::new("official.test.fixture.fact.end").map_err(contract)?;
    let assignment_kind_fact =
        VerificationFactId::new("official.test.fixture.fact.assignment-kind").map_err(contract)?;
    let mut selected = Vec::new();
    let mut evaluations = Vec::with_capacity(OPTIONS.len() + 2);
    for option in OPTIONS {
        let assignment = assignments
            .get(&fixture_assignment_id(option)?)
            .ok_or_else(|| contract("optional fixture assignment is missing"))?;
        let entity = fixture_entity(option)?;
        if assignment.entity != entity {
            return Err(contract("optional fixture assignment entity mismatch"));
        }
        let mut observed = BTreeMap::from([(
            present_fact.clone(),
            VerificationValue::Boolean(!matches!(assignment.value, AssignmentValue::Absent)),
        )]);
        let satisfied = match &assignment.value {
            AssignmentValue::Absent => true,
            AssignmentValue::Interval(interval) => {
                observed.extend([
                    (
                        start_fact.clone(),
                        VerificationValue::Integer(interval.start),
                    ),
                    (
                        duration_fact.clone(),
                        VerificationValue::Integer(interval.duration),
                    ),
                    (end_fact.clone(), VerificationValue::Integer(interval.end)),
                ]);
                let exact = interval.start == option.start
                    && interval.duration == option.duration
                    && interval.end == option.end();
                if exact && option.eligible {
                    selected.push((option, *interval));
                }
                exact && option.eligible
            }
            AssignmentValue::Boolean(_) => {
                observed.insert(
                    assignment_kind_fact.clone(),
                    VerificationValue::Text("boolean".to_owned()),
                );
                false
            }
            AssignmentValue::Integer(_) => {
                observed.insert(
                    assignment_kind_fact.clone(),
                    VerificationValue::Text("integer".to_owned()),
                );
                false
            }
        };
        evaluations.push(RuleEvaluation {
            rule_id: fixture_rule_id(option)?,
            satisfied,
            affected_entities: vec![entity],
            message_key: "official.test.fixture.verify.option".to_owned(),
            expected: BTreeMap::from([
                (
                    eligible_fact.clone(),
                    VerificationValue::Boolean(option.eligible),
                ),
                (start_fact.clone(), VerificationValue::Integer(option.start)),
                (
                    duration_fact.clone(),
                    VerificationValue::Integer(option.duration),
                ),
                (end_fact.clone(), VerificationValue::Integer(option.end())),
            ]),
            observed,
            evidence: vec![
                DomainEvidenceId::new(fixture_provenance("eligibility", option)?.as_str())
                    .map_err(contract)?,
            ],
        });
    }

    let no_overlap_fact =
        VerificationFactId::new("official.test.fixture.fact.no-overlap").map_err(contract)?;
    let has_overlap = selected.iter().enumerate().any(|(left_index, (_, left))| {
        selected
            .iter()
            .skip(left_index + 1)
            .any(|(_, right)| left.start < right.end && right.start < left.end)
    });
    let affected_entities = fixture_selected_entities(&selected)?;
    evaluations.push(RuleEvaluation {
        rule_id: RuleId::from_str(FIXTURE_RULE_NO_OVERLAP).map_err(contract)?,
        satisfied: !has_overlap,
        affected_entities: affected_entities.clone(),
        message_key: "official.test.fixture.verify.no_overlap".to_owned(),
        expected: BTreeMap::from([(no_overlap_fact.clone(), VerificationValue::Boolean(true))]),
        observed: BTreeMap::from([(no_overlap_fact, VerificationValue::Boolean(!has_overlap))]),
        evidence: vec![
            DomainEvidenceId::new("official_test.fixture.rule.global").map_err(contract)?,
        ],
    });

    let max_demand = (0..5)
        .map(|instant| {
            selected
                .iter()
                .filter(|(_, interval)| interval.start <= instant && instant < interval.end)
                .count()
        })
        .max()
        .unwrap_or(0);
    let capacity_fact =
        VerificationFactId::new("official.test.fixture.fact.capacity").map_err(contract)?;
    let demand_fact =
        VerificationFactId::new("official.test.fixture.fact.maximum-demand").map_err(contract)?;
    evaluations.push(RuleEvaluation {
        rule_id: RuleId::from_str(FIXTURE_RULE_CUMULATIVE).map_err(contract)?,
        satisfied: max_demand <= 1,
        affected_entities,
        message_key: "official.test.fixture.verify.cumulative".to_owned(),
        expected: BTreeMap::from([(capacity_fact, VerificationValue::Integer(1))]),
        observed: BTreeMap::from([(
            demand_fact,
            VerificationValue::Integer(i64::try_from(max_demand).map_err(contract)?),
        )]),
        evidence: vec![
            DomainEvidenceId::new("official_test.fixture.rule.global").map_err(contract)?,
        ],
    });
    Ok(evaluations)
}

fn fixture_selected_entities(
    selected: &[(FixtureOption, eutheto_domain_ir::AssignedInterval)],
) -> Result<Vec<DomainEntityRef>, DomainPackError> {
    let entities = selected
        .iter()
        .map(|(option, _)| fixture_entity(*option))
        .collect::<Result<BTreeSet<_>, _>>()?;
    Ok(entities.into_iter().collect())
}

fn authoritative_interval_score(
    document: &ScenarioDocument,
    solution: &NormalizedSolution,
) -> Result<ScoreVector, DomainPackError> {
    if document.domain_pack.id.as_str() != OFFICIAL_TEST_PACK_ID
        || solution.pack_id != document.domain_pack.id
        || solution.scenario_id != document.scenario_id
    {
        return Err(contract("optional fixture solution scenario mismatch"));
    }
    solution.validate().map_err(contract)?;
    let expected_ids = OPTIONS
        .into_iter()
        .map(fixture_assignment_id)
        .collect::<Result<BTreeSet<_>, _>>()?;
    if solution.assignments.len() != expected_ids.len()
        || solution
            .assignments
            .iter()
            .any(|assignment| !expected_ids.contains(&assignment.id))
    {
        return Err(contract("optional fixture assignment set mismatch"));
    }

    let assignments = solution
        .assignments
        .iter()
        .map(|assignment| (&assignment.id, assignment))
        .collect::<BTreeMap<_, _>>();
    let mut selected = Vec::new();
    let mut selected_count = 0_i64;
    let mut feasibility = 0_i64;
    for option in OPTIONS {
        let assignment = assignments
            .get(&fixture_assignment_id(option)?)
            .ok_or_else(|| contract("optional fixture assignment is missing"))?;
        if assignment.entity != fixture_entity(option)? {
            return Err(contract("optional fixture assignment entity mismatch"));
        }
        match &assignment.value {
            AssignmentValue::Absent => {}
            AssignmentValue::Interval(interval)
                if interval.start == option.start
                    && interval.duration == option.duration
                    && interval.end == option.end() =>
            {
                selected_count = selected_count
                    .checked_add(1)
                    .ok_or_else(|| contract("optional fixture selected count overflow"))?;
                if option.eligible {
                    selected.push(*interval);
                } else {
                    feasibility = feasibility
                        .checked_add(1)
                        .ok_or_else(|| contract("optional fixture feasibility overflow"))?;
                }
            }
            AssignmentValue::Boolean(_)
            | AssignmentValue::Integer(_)
            | AssignmentValue::Interval(_) => {
                feasibility = feasibility
                    .checked_add(1)
                    .ok_or_else(|| contract("optional fixture feasibility overflow"))?;
            }
        }
    }

    let has_overlap = selected.iter().enumerate().any(|(left_index, left)| {
        selected
            .iter()
            .skip(left_index + 1)
            .any(|right| left.start < right.end && right.start < left.end)
    });
    if has_overlap {
        feasibility = feasibility
            .checked_add(1)
            .ok_or_else(|| contract("optional fixture feasibility overflow"))?;
    }
    let max_demand = (0..5)
        .map(|instant| {
            selected
                .iter()
                .filter(|interval| interval.start <= instant && instant < interval.end)
                .count()
        })
        .max()
        .unwrap_or(0);
    if max_demand > 1 {
        feasibility = feasibility
            .checked_add(1)
            .ok_or_else(|| contract("optional fixture feasibility overflow"))?;
    }

    Ok(ScoreVector {
        feasibility,
        levels: vec![ScoreLevelValue {
            level_id: ScoreLevelId::new(FIXTURE_SCORE_LEVEL).map_err(contract)?,
            value: selected_count,
            direction: OptimizationDirection::Minimize,
            category_breakdown: [(
                ScoreCategoryId::new(FIXTURE_SCORE_CATEGORY).map_err(contract)?,
                selected_count,
            )]
            .into_iter()
            .collect(),
        }],
    })
}

fn fixture_entity(option: FixtureOption) -> Result<DomainEntityRef, DomainPackError> {
    Ok(DomainEntityRef {
        kind: DomainEntityKindId::new(FIXTURE_ENTITY_KIND).map_err(contract)?,
        id: DomainEntityId::new(format!("official.test.fixture.option.{}", option.key))
            .map_err(contract)?,
    })
}

fn fixture_provenance(kind: &str, option: FixtureOption) -> Result<ProvenanceId, DomainPackError> {
    ProvenanceId::new(format!("official_test.fixture.{kind}.{}", option.key)).map_err(contract)
}

fn fixture_bool_id(option: FixtureOption) -> Result<BoolVariableId, DomainPackError> {
    BoolVariableId::new(format!("official_test.fixture.present.{}", option.key)).map_err(contract)
}

fn fixture_int_id(
    option: FixtureOption,
    component: &str,
) -> Result<IntVariableId, DomainPackError> {
    IntVariableId::new(format!("official_test.fixture.{component}.{}", option.key))
        .map_err(contract)
}

fn fixture_interval_id(option: FixtureOption) -> Result<IntervalVariableId, DomainPackError> {
    IntervalVariableId::new(format!("official_test.fixture.interval.{}", option.key))
        .map_err(contract)
}

fn fixture_projection_id(option: FixtureOption) -> Result<ProjectionId, DomainPackError> {
    ProjectionId::new(format!("official_test.fixture.project.{}", option.key)).map_err(contract)
}

fn fixture_assignment_id(option: FixtureOption) -> Result<DomainAssignmentId, DomainPackError> {
    DomainAssignmentId::new(format!("official.test.fixture.assignment.{}", option.key))
        .map_err(contract)
}

fn provenance_record(
    id: ProvenanceId,
    source_kind: ProvenanceSourceKind,
    source_id: String,
    entity: DomainEntityRef,
    parent: Option<ProvenanceId>,
) -> ProvenanceRecord {
    ProvenanceRecord {
        id,
        source_kind,
        source_id,
        entity_refs: vec![entity],
        message_key: "official.test.fixture.provenance.option".to_owned(),
        parameters: BTreeMap::new(),
        parent,
    }
}

fn fixture_document() -> Result<ScenarioDocument, serde_json::Error> {
    serde_json::from_value(json!({
        "format": "eutheto/scenario",
        "formatVersion": 1,
        "scenarioId": SCENARIO_ID,
        "domainPack": { "id": OFFICIAL_TEST_PACK_ID, "schemaVersion": 1 },
        "metadata": {
            "title": "Optional interval contract fixture",
            "description": "",
            "createdAt": "2026-08-29T12:00:00Z",
            "updatedAt": "2026-08-29T12:00:00Z"
        },
        "settings": {
            "timeZone": "UTC",
            "locale": "en-US",
            "units": "metric",
            "horizon": {
                "start": "2026-08-29T12:00:00Z",
                "end": "2026-09-05T12:00:00Z"
            },
            "gapPolicy": "reject",
            "overlapPolicy": "earlier"
        },
        "domain": {
            "entities": {},
            "rules": {},
            "preferences": {},
            "lockedAssignments": {}
        },
        "extensions": {}
    }))
}

fn fixture_context() -> CompileContext {
    CompileContext {
        scenario_revision: 11,
        semantic_metadata: BTreeMap::new(),
        control: eutheto_types::OperationControl::Cancellation(CancellationToken::new()),
        planning_limits: PlanningIrLimitsV1::DEFAULT,
    }
}
fn fixture_verification_context(
    pack: &dyn DomainPack,
    document: &ScenarioDocument,
    problem: &PlanningProblem,
    solution: &NormalizedSolution,
) -> Result<VerificationContextV1, Box<dyn Error>> {
    let scope = pack.verification_scope(
        document,
        solution.scenario_revision,
        &eutheto_types::OperationControl::Cancellation(CancellationToken::new()),
    )?;
    Ok(VerificationContextV1::new(
        document.scenario_id,
        solution.scenario_revision,
        blake3_hex(&serde_json::to_vec(document)?),
        canonical_ir_hash(problem, PlanningIrLimitsV1::DEFAULT)?,
        solution.canonical_hash()?,
        scope.checksum,
    )?)
}

fn candidate_for(
    problem: &PlanningProblem,
    selected_mask: usize,
) -> Result<CandidateValues, DomainPackError> {
    let bool_ids: BTreeSet<_> = problem
        .variables
        .iter()
        .filter_map(|variable| match variable {
            Variable::Boolean(value) => Some(value.id.clone()),
            Variable::Integer(_) | Variable::Interval(_) => None,
        })
        .collect();
    let int_ids: BTreeSet<_> = problem
        .variables
        .iter()
        .filter_map(|variable| match variable {
            Variable::Integer(value) => Some(value.id.clone()),
            Variable::Boolean(_) | Variable::Interval(_) => None,
        })
        .collect();
    let mut candidate = CandidateValues::default();
    for (index, option) in OPTIONS.into_iter().enumerate() {
        let selected = selected_mask & (1 << index) != 0;
        let present = fixture_bool_id(option)?;
        if bool_ids.contains(&present) {
            candidate.booleans.insert(present, selected);
        }
        for (component, value) in [
            ("start", option.start),
            ("duration", option.duration),
            ("end", option.end()),
            ("cost", i64::from(selected)),
        ] {
            let id = fixture_int_id(option, component)?;
            if int_ids.contains(&id) {
                candidate.integers.insert(id, value);
            }
        }
    }
    Ok(candidate)
}

#[derive(Debug, Eq, PartialEq)]
struct FixtureTruth {
    accepted: bool,
    feasibility: i64,
    objective: i64,
    assignments: BTreeMap<DomainAssignmentId, (DomainEntityRef, AssignmentValue)>,
    rule_satisfaction: BTreeMap<RuleId, bool>,
}

struct FixtureObservation {
    compiler_accepts: bool,
    compiler_objective: i64,
    solution: NormalizedSolution,
    score: ScoreVector,
    report: VerificationReport,
}

// This is intentionally a truth table over the domain fixture constants, not a restatement of
// either the planning primitives or the verifier/scorer implementation.
fn fixture_truth(selected_mask: usize) -> Result<FixtureTruth, DomainPackError> {
    let mut assignments = BTreeMap::new();
    let mut rule_satisfaction = BTreeMap::new();
    let mut selected = Vec::new();
    let mut feasibility = 0_i64;
    let mut objective = 0_i64;
    for (index, option) in OPTIONS.into_iter().enumerate() {
        let is_selected = selected_mask & (1 << index) != 0;
        let value = if is_selected {
            objective = objective
                .checked_add(1)
                .ok_or_else(|| contract("fixture oracle objective overflow"))?;
            let interval = eutheto_domain_ir::AssignedInterval::new(
                option.start,
                option.duration,
                option.end(),
            )
            .map_err(contract)?;
            if option.eligible {
                selected.push(interval);
            } else {
                feasibility = feasibility
                    .checked_add(1)
                    .ok_or_else(|| contract("fixture oracle feasibility overflow"))?;
            }
            AssignmentValue::Interval(interval)
        } else {
            AssignmentValue::Absent
        };
        assignments.insert(
            fixture_assignment_id(option)?,
            (fixture_entity(option)?, value),
        );
        rule_satisfaction.insert(fixture_rule_id(option)?, !is_selected || option.eligible);
    }

    let has_overlap = selected.iter().enumerate().any(|(left_index, left)| {
        selected
            .iter()
            .skip(left_index + 1)
            .any(|right| left.start < right.end && right.start < left.end)
    });
    let horizon_end = OPTIONS
        .into_iter()
        .map(FixtureOption::end)
        .max()
        .unwrap_or(0);
    let exceeds_capacity = (0..horizon_end).any(|instant| {
        selected
            .iter()
            .filter(|interval| interval.start <= instant && instant < interval.end)
            .count()
            > 1
    });
    rule_satisfaction.insert(
        RuleId::from_str(FIXTURE_RULE_NO_OVERLAP).map_err(contract)?,
        !has_overlap,
    );
    rule_satisfaction.insert(
        RuleId::from_str(FIXTURE_RULE_CUMULATIVE).map_err(contract)?,
        !exceeds_capacity,
    );
    feasibility = feasibility
        .checked_add(i64::from(has_overlap))
        .and_then(|value| value.checked_add(i64::from(exceeds_capacity)))
        .ok_or_else(|| contract("fixture oracle feasibility overflow"))?;

    Ok(FixtureTruth {
        accepted: feasibility == 0,
        feasibility,
        objective,
        assignments,
        rule_satisfaction,
    })
}

fn fixture_expected_score(truth: &FixtureTruth) -> Result<ScoreVector, DomainPackError> {
    Ok(ScoreVector {
        feasibility: truth.feasibility,
        levels: vec![ScoreLevelValue {
            level_id: ScoreLevelId::new(FIXTURE_SCORE_LEVEL).map_err(contract)?,
            value: truth.objective,
            direction: OptimizationDirection::Minimize,
            category_breakdown: [(
                ScoreCategoryId::new(FIXTURE_SCORE_CATEGORY).map_err(contract)?,
                truth.objective,
            )]
            .into_iter()
            .collect(),
        }],
    })
}

fn fixture_literal_value(
    literal: &Literal,
    candidate: &CandidateValues,
) -> Result<bool, DomainPackError> {
    candidate
        .booleans
        .get(&literal.variable)
        .copied()
        .map(|value| value == literal.positive)
        .ok_or_else(|| contract(format!("missing compiler Boolean {}", literal.variable)))
}

fn fixture_linear_value(
    expression: &LinearExpression,
    candidate: &CandidateValues,
) -> Result<i64, DomainPackError> {
    let mut value = expression.constant;
    for term in &expression.terms {
        let assigned = candidate
            .integers
            .get(&term.variable)
            .copied()
            .ok_or_else(|| contract(format!("missing compiler integer {}", term.variable)))?;
        value = value
            .checked_add(
                term.coefficient
                    .checked_mul(assigned)
                    .ok_or_else(|| contract("compiler expression overflow"))?,
            )
            .ok_or_else(|| contract("compiler expression overflow"))?;
    }
    Ok(value)
}

fn fixture_present_interval(
    problem: &PlanningProblem,
    candidate: &CandidateValues,
    id: &IntervalVariableId,
) -> Result<Option<(i64, i64)>, DomainPackError> {
    let interval = problem
        .variables
        .iter()
        .find_map(|variable| match variable {
            Variable::Interval(interval) if interval.id == *id => Some(interval),
            Variable::Boolean(_) | Variable::Integer(_) | Variable::Interval(_) => None,
        })
        .ok_or_else(|| contract(format!("unknown compiler interval {id}")))?;
    if let Some(presence) = &interval.presence
        && !fixture_literal_value(presence, candidate)?
    {
        return Ok(None);
    }
    let start = candidate
        .integers
        .get(&interval.start)
        .copied()
        .ok_or_else(|| contract(format!("missing compiler integer {}", interval.start)))?;
    let duration = candidate
        .integers
        .get(&interval.duration)
        .copied()
        .ok_or_else(|| contract(format!("missing compiler integer {}", interval.duration)))?;
    let end = candidate
        .integers
        .get(&interval.end)
        .copied()
        .ok_or_else(|| contract(format!("missing compiler integer {}", interval.end)))?;
    if duration < 0 || start.checked_add(duration) != Some(end) {
        return Err(contract(format!("incoherent compiler interval {id}")));
    }
    Ok(Some((start, end)))
}

fn fixture_constraint_value(
    problem: &PlanningProblem,
    candidate: &CandidateValues,
    constraint: &Constraint,
) -> Result<bool, DomainPackError> {
    match constraint {
        Constraint::BoolAnd { literals } => {
            for literal in literals {
                if !fixture_literal_value(literal, candidate)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        Constraint::LinearComparison(comparison) => {
            let left = fixture_linear_value(&comparison.expression, candidate)?;
            Ok(match comparison.op {
                ComparisonOp::Equal => left == comparison.rhs,
                ComparisonOp::LessOrEqual => left <= comparison.rhs,
                ComparisonOp::GreaterOrEqual => left >= comparison.rhs,
            })
        }
        Constraint::NoOverlap { intervals } => {
            let mut present = Vec::new();
            for interval in intervals {
                if let Some(value) = fixture_present_interval(problem, candidate, interval)? {
                    present.push(value);
                }
            }
            Ok(!present.iter().enumerate().any(|(left_index, left)| {
                present
                    .iter()
                    .skip(left_index + 1)
                    .any(|right| left.0 < right.1 && right.0 < left.1)
            }))
        }
        Constraint::Cumulative {
            intervals,
            demands,
            capacity,
        } => {
            let mut present = Vec::new();
            for (interval, demand) in intervals.iter().zip(demands) {
                if let Some((start, end)) = fixture_present_interval(problem, candidate, interval)?
                {
                    present.push((start, end, *demand));
                }
            }
            for instant in present.iter().map(|(start, _, _)| *start) {
                let mut demand = 0_i64;
                for (start, end, value) in &present {
                    if *start <= instant && instant < *end {
                        demand = demand
                            .checked_add(*value)
                            .ok_or_else(|| contract("compiler cumulative overflow"))?;
                    }
                }
                if demand > *capacity {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        unsupported => Err(contract(format!(
            "unsupported primitive in optional fixture: {unsupported:?}"
        ))),
    }
}

fn fixture_compiler_accepts(
    problem: &PlanningProblem,
    candidate: &CandidateValues,
) -> Result<bool, DomainPackError> {
    for variable in &problem.variables {
        match variable {
            Variable::Boolean(variable) => {
                if !candidate.booleans.contains_key(&variable.id) {
                    return Err(contract(format!(
                        "missing compiler Boolean {}",
                        variable.id
                    )));
                }
            }
            Variable::Integer(variable) => {
                let value = candidate
                    .integers
                    .get(&variable.id)
                    .copied()
                    .ok_or_else(|| contract(format!("missing compiler integer {}", variable.id)))?;
                if !variable.domain.contains(value) {
                    return Ok(false);
                }
            }
            Variable::Interval(variable) => {
                fixture_present_interval(problem, candidate, &variable.id)?;
            }
        }
    }
    for record in &problem.constraints {
        let mut enabled = true;
        for literal in &record.enforcement {
            if !fixture_literal_value(literal, candidate)? {
                enabled = false;
                break;
            }
        }
        if enabled && !fixture_constraint_value(problem, candidate, &record.body)? {
            return Ok(false);
        }
    }
    Ok(true)
}

fn fixture_compiler_objective(
    problem: &PlanningProblem,
    candidate: &CandidateValues,
) -> Result<i64, DomainPackError> {
    let [level] = problem.objectives.levels.as_slice() else {
        return Err(contract(
            "optional fixture must have exactly one objective level",
        ));
    };
    let mut objective = 0_i64;
    for term in &level.terms {
        if term.kind != ObjectiveTermKind::Penalty {
            return Err(contract(
                "optional fixture objective must contain penalties",
            ));
        }
        objective = objective
            .checked_add(fixture_linear_value(&term.expression, candidate)?)
            .ok_or_else(|| contract("compiler objective overflow"))?;
    }
    Ok(objective)
}

fn observe_fixture_candidate(
    pack: IntervalFixturePack,
    document: &ScenarioDocument,
    problem: &PlanningProblem,
    selected_mask: usize,
) -> Result<FixtureObservation, Box<dyn Error>> {
    let candidate = candidate_for(problem, selected_mask)?;
    let compiler_accepts = fixture_compiler_accepts(problem, &candidate)?;
    let compiler_objective = fixture_compiler_objective(problem, &candidate)?;
    let solution = pack.project(
        problem,
        &candidate,
        SolutionId::from_str(SOLUTION_ID)?,
        &eutheto_types::OperationControl::Cancellation(CancellationToken::new()),
    )?;
    let score = pack.score(
        document,
        &solution,
        &eutheto_types::OperationControl::Cancellation(CancellationToken::new()),
    )?;
    let verification_context = fixture_verification_context(&pack, document, problem, &solution)?;
    let report = pack.verify(
        document,
        &solution,
        &verification_context,
        &score,
        &eutheto_types::OperationControl::Cancellation(CancellationToken::new()),
    )?;
    Ok(FixtureObservation {
        compiler_accepts,
        compiler_objective,
        solution,
        score,
        report,
    })
}

fn assert_fixture_conformance(
    selected_mask: usize,
    observation: &FixtureObservation,
) -> Result<(), DomainPackError> {
    let truth = fixture_truth(selected_mask)?;
    if observation.compiler_accepts != truth.accepted {
        return Err(contract(format!(
            "compiler feasibility mismatch for mask {selected_mask:#05b}"
        )));
    }
    if observation.compiler_objective != truth.objective {
        return Err(contract(format!(
            "compiler objective mismatch for mask {selected_mask:#05b}"
        )));
    }

    let actual_assignments = observation
        .solution
        .assignments
        .iter()
        .map(|assignment| {
            (
                assignment.id.clone(),
                (assignment.entity.clone(), assignment.value.clone()),
            )
        })
        .collect::<BTreeMap<_, _>>();
    if actual_assignments != truth.assignments {
        return Err(contract(format!(
            "exact projection mismatch for mask {selected_mask:#05b}"
        )));
    }

    let expected_score = fixture_expected_score(&truth)?;
    if observation.score != expected_score {
        return Err(contract(format!(
            "authoritative score mismatch for mask {selected_mask:#05b}"
        )));
    }
    let actual_rules = observation
        .report
        .required_rule_results
        .iter()
        .map(|evaluation| (evaluation.rule_id, evaluation.satisfied))
        .collect::<BTreeMap<_, _>>();
    if actual_rules != truth.rule_satisfaction {
        return Err(contract(format!(
            "verifier rule mismatch for mask {selected_mask:#05b}"
        )));
    }
    if observation.report.accepted != truth.accepted {
        return Err(contract(format!(
            "verifier acceptance mismatch for mask {selected_mask:#05b}"
        )));
    }
    if observation.report.score != expected_score {
        return Err(contract(format!(
            "verifier score mismatch for mask {selected_mask:#05b}"
        )));
    }
    Ok(())
}

fn projected_schedule(solution: &NormalizedSolution) -> Result<String, serde_json::Error> {
    serde_json::to_string(&solution.assignments)
}
fn explanation_capability(kind: eutheto_domain_ir::ExplanationKind) -> ExplanationCapability {
    match kind {
        eutheto_domain_ir::ExplanationKind::Validation => ExplanationCapability::Validation,
        eutheto_domain_ir::ExplanationKind::Infeasibility => ExplanationCapability::Infeasibility,
        eutheto_domain_ir::ExplanationKind::Assignment => ExplanationCapability::Assignment,
        eutheto_domain_ir::ExplanationKind::Counterfactual => ExplanationCapability::Counterfactual,
        eutheto_domain_ir::ExplanationKind::SolutionDifference => {
            ExplanationCapability::SolutionDifference
        }
        eutheto_domain_ir::ExplanationKind::Repair => ExplanationCapability::Repair,
        eutheto_domain_ir::ExplanationKind::OptimalityStatus => {
            ExplanationCapability::OptimalityStatus
        }
    }
}

fn contract(error: impl std::fmt::Display) -> DomainPackError {
    DomainPackError::Contract(error.to_string())
}

fn compile_and_assert_interval_problem(
    pack: IntervalFixturePack,
    document: &ScenarioDocument,
    context: &CompileContext,
) -> Result<PlanningProblem, Box<dyn Error>> {
    let first = pack.compile(document, context)?;
    let second = pack.compile(document, context)?;
    validate(&first, context.planning_limits)?;
    let summary = summarize(&first, context.planning_limits)?;
    assert_eq!(first, second);
    assert_eq!(summary, summarize(&second, context.planning_limits)?);
    assert_eq!(summary.interval_variable_count, 3);
    assert_eq!(summary.manifest.optional_interval_count, 3);
    assert_eq!(summary.manifest.global_interval_constraint_count, 2);
    assert_eq!(
        summary.manifest.required_capabilities(),
        [
            Capability::BoolAnd,
            Capability::LinearComparison,
            Capability::NoOverlap,
            Capability::Cumulative,
            Capability::OptionalIntervals,
            Capability::ObjectivePenalty,
            Capability::IntervalProjection,
        ]
        .into_iter()
        .collect()
    );
    assert_eq!(
        first.declared_capabilities,
        summary.manifest.required_capabilities()
    );
    Ok(first)
}

#[test]
// This test intentionally exercises the complete optional-interval boundary in one sequence.
#[allow(clippy::too_many_lines)]
fn optional_intervals_compile_project_and_verify_boundaries() -> Result<(), Box<dyn Error>> {
    let pack = IntervalFixturePack::with_pruning(false);
    let document = fixture_document()?;
    let context = fixture_context();
    let first = compile_and_assert_interval_problem(pack, &document, &context)?;

    let missing_presence = CandidateValues::default();
    assert!(
        pack.project(
            &first,
            &missing_presence,
            SolutionId::from_str(SOLUTION_ID)?,
            &context.control
        )
        .is_err()
    );

    let absent_candidate = candidate_for(&first, 0)?;
    let absent = pack.project(
        &first,
        &absent_candidate,
        SolutionId::from_str(SOLUTION_ID)?,
        &eutheto_types::OperationControl::Cancellation(CancellationToken::new()),
    )?;
    assert!(
        absent
            .assignments
            .iter()
            .all(|assignment| assignment.value == AssignmentValue::Absent)
    );
    let absent_score = pack.score(
        &document,
        &absent,
        &eutheto_types::OperationControl::Cancellation(CancellationToken::new()),
    )?;
    let absent_context = fixture_verification_context(&pack, &document, &first, &absent)?;
    let absent_report = pack.verify(
        &document,
        &absent,
        &absent_context,
        &absent_score,
        &eutheto_types::OperationControl::Cancellation(CancellationToken::new()),
    )?;
    assert!(absent_report.accepted);
    assert_eq!(absent_report.score.feasibility, 0);
    assert_eq!(absent_report.score.levels[0].value, 0);
    let scope = pack.verification_scope(
        &document,
        context.scenario_revision,
        &eutheto_types::OperationControl::Cancellation(CancellationToken::new()),
    )?;
    assert_eq!(
        scope
            .required_rules
            .iter()
            .map(|binding| binding.rule_id.to_string())
            .collect::<Vec<_>>(),
        vec![
            FIXTURE_RULE_EARLY,
            FIXTURE_RULE_OVERLAP,
            FIXTURE_RULE_INELIGIBLE,
            FIXTURE_RULE_NO_OVERLAP,
            FIXTURE_RULE_CUMULATIVE,
        ]
    );
    assert_eq!(
        absent_report
            .required_rule_results
            .iter()
            .map(|evaluation| evaluation.rule_id)
            .collect::<Vec<_>>(),
        scope
            .required_rules
            .iter()
            .map(|binding| binding.rule_id)
            .collect::<Vec<_>>()
    );
    assert_eq!(absent_report.required_rule_results.len(), OPTIONS.len() + 2);
    assert!(
        absent_report
            .required_rule_results
            .iter()
            .all(|evaluation| evaluation.satisfied)
    );

    let mut ignored_incoherence = absent_candidate;
    ignored_incoherence
        .integers
        .insert(fixture_int_id(OPTIONS[0], "end")?, 3);
    let still_absent = pack.project(
        &first,
        &ignored_incoherence,
        SolutionId::from_str(SOLUTION_ID)?,
        &eutheto_types::OperationControl::Cancellation(CancellationToken::new()),
    )?;
    assert!(
        still_absent
            .assignments
            .iter()
            .all(|assignment| assignment.value == AssignmentValue::Absent)
    );

    let present_candidate = candidate_for(&first, 1)?;
    let present = pack.project(
        &first,
        &present_candidate,
        SolutionId::from_str(SOLUTION_ID)?,
        &eutheto_types::OperationControl::Cancellation(CancellationToken::new()),
    )?;
    let present_score = pack.score(
        &document,
        &present,
        &eutheto_types::OperationControl::Cancellation(CancellationToken::new()),
    )?;
    let present_context = fixture_verification_context(&pack, &document, &first, &present)?;
    let present_report = pack.verify(
        &document,
        &present,
        &present_context,
        &present_score,
        &eutheto_types::OperationControl::Cancellation(CancellationToken::new()),
    )?;
    assert!(present_report.accepted);
    assert_eq!(present_report.score.feasibility, 0);
    assert_eq!(present_report.score, present_score);
    assert!(matches!(
        &present.assignments[0].value,
        AssignmentValue::Interval(interval)
            if interval.start == 0 && interval.duration == 2 && interval.end == 2
    ));

    let mut incoherent = present_candidate;
    incoherent
        .integers
        .insert(fixture_int_id(OPTIONS[0], "end")?, 3);
    let incoherent_error = pack.project(
        &first,
        &incoherent,
        SolutionId::from_str(SOLUTION_ID)?,
        &eutheto_types::OperationControl::Cancellation(CancellationToken::new()),
    );
    let message = match incoherent_error {
        Err(DomainPackError::Contract(message)) => message,
        other => {
            return Err(format!(
                "a present incoherent interval must fail projection with a contract error, got {other:?}"
            )
            .into());
        }
    };
    assert!(message.contains("InvalidInterval"));

    let overlap = pack.project(
        &first,
        &candidate_for(&first, 0b011)?,
        SolutionId::from_str(SOLUTION_ID)?,
        &eutheto_types::OperationControl::Cancellation(CancellationToken::new()),
    )?;
    let overlap_score = pack.score(
        &document,
        &overlap,
        &eutheto_types::OperationControl::Cancellation(CancellationToken::new()),
    )?;
    let overlap_context = fixture_verification_context(&pack, &document, &first, &overlap)?;
    let overlap_report = pack.verify(
        &document,
        &overlap,
        &overlap_context,
        &overlap_score,
        &eutheto_types::OperationControl::Cancellation(CancellationToken::new()),
    )?;
    assert!(!overlap_report.accepted);
    assert_eq!(overlap_report.score.feasibility, 2);
    assert_eq!(
        overlap_report
            .required_rule_results
            .iter()
            .filter(|evaluation| !evaluation.satisfied)
            .map(|evaluation| evaluation.message_key.as_str())
            .collect::<Vec<_>>(),
        vec![
            "official.test.fixture.verify.no_overlap",
            "official.test.fixture.verify.cumulative",
        ]
    );
    Ok(())
}

#[test]
fn deterministic_pruning_preserves_every_feasible_schedule_and_score() -> Result<(), Box<dyn Error>>
{
    let document = fixture_document()?;
    let context = fixture_context();
    let unpruned_pack = IntervalFixturePack::with_pruning(false);
    let pruned_pack = IntervalFixturePack::with_pruning(true);
    let unpruned = unpruned_pack.compile(&document, &context)?;
    let pruned = pruned_pack.compile(&document, &context)?;
    assert_eq!(unpruned, unpruned_pack.compile(&document, &context)?);
    assert_eq!(pruned, pruned_pack.compile(&document, &context)?);
    validate(&unpruned, context.planning_limits)?;
    validate(&pruned, context.planning_limits)?;

    let unpruned_summary = summarize(&unpruned, context.planning_limits)?;
    let pruned_summary = summarize(&pruned, context.planning_limits)?;
    let unpruned_pruning = PruningSummary::read(&unpruned)?;
    let pruned_pruning = PruningSummary::read(&pruned)?;
    assert_eq!(
        unpruned_pruning,
        PruningSummary {
            candidates_before: 3,
            candidates_after: 3,
            model_before: ModelEstimate {
                variables: 18,
                constraints: 18,
            },
            model_after: ModelEstimate {
                variables: 18,
                constraints: 18,
            },
        }
    );
    assert_eq!(
        pruned_pruning,
        PruningSummary {
            candidates_before: 3,
            candidates_after: 2,
            model_before: ModelEstimate {
                variables: 18,
                constraints: 18,
            },
            model_after: ModelEstimate {
                variables: 12,
                constraints: 12,
            },
        }
    );
    assert_eq!(
        pruned_summary.variable_count,
        u64::from(pruned_pruning.model_after.variables)
    );
    assert_eq!(
        pruned_summary.constraint_count,
        u64::from(pruned_pruning.model_after.constraints)
    );
    assert!(pruned.variables.iter().all(|variable| match variable {
        Variable::Boolean(value) => !value.id.as_str().contains("ineligible"),
        Variable::Integer(value) => !value.id.as_str().contains("ineligible"),
        Variable::Interval(value) => !value.id.as_str().contains("ineligible"),
    }));
    assert_ne!(
        unpruned_summary.canonical_ir_hash,
        pruned_summary.canonical_ir_hash
    );
    assert_eq!(
        unpruned
            .projections
            .iter()
            .map(|projection| (&projection.assignment_id, &projection.entity))
            .collect::<Vec<_>>(),
        pruned
            .projections
            .iter()
            .map(|projection| (&projection.assignment_id, &projection.entity))
            .collect::<Vec<_>>()
    );
    assert_eq!(unpruned.provenance, pruned.provenance);

    let mut unpruned_feasible = BTreeMap::new();
    let mut pruned_feasible = BTreeMap::new();
    let pruned_bits = OPTIONS
        .into_iter()
        .enumerate()
        .filter(|(_, option)| !option.eligible)
        .fold(0_usize, |mask, (index, _)| mask | (1 << index));
    for (pack, problem, feasible) in [
        (unpruned_pack, &unpruned, &mut unpruned_feasible),
        (pruned_pack, &pruned, &mut pruned_feasible),
    ] {
        for mask in 0..(1 << OPTIONS.len()) {
            // A pruned option has no decision variable, so only masks representable by that
            // compiled problem are candidates for its independent compiler evaluation.
            if pack.prune && mask & pruned_bits != 0 {
                continue;
            }
            let observation = observe_fixture_candidate(pack, &document, problem, mask)?;
            assert_fixture_conformance(mask, &observation)?;
            let truth = fixture_truth(mask)?;
            if truth.accepted {
                feasible.insert(projected_schedule(&observation.solution)?, truth.objective);
            }
        }
    }
    assert_eq!(unpruned_feasible.len(), 3);
    assert_eq!(unpruned_feasible, pruned_feasible);
    Ok(())
}

#[test]
fn conformance_rejects_one_sided_compiler_constraint_mutation() -> Result<(), Box<dyn Error>> {
    let pack = IntervalFixturePack::with_pruning(false);
    let document = fixture_document()?;
    let mut problem = pack.compile(&document, &fixture_context())?;
    let record = problem
        .constraints
        .iter_mut()
        .find(|record| record.id.as_str() == "official_test.fixture.fixed_start.early")
        .ok_or("missing early start constraint")?;
    let Constraint::LinearComparison(comparison) = &mut record.body else {
        return Err("early start constraint is not a linear comparison".into());
    };
    comparison.rhs = 1;

    let observation = observe_fixture_candidate(pack, &document, &problem, 0b001)?;
    let Err(error) = assert_fixture_conformance(0b001, &observation) else {
        return Err("the oracle accepted a one-sided compiler constraint mutation".into());
    };
    assert!(error.to_string().contains("compiler feasibility mismatch"));
    Ok(())
}

#[test]
fn conformance_rejects_one_sided_verifier_rule_mutation() -> Result<(), Box<dyn Error>> {
    let pack = IntervalFixturePack::with_pruning(false);
    let document = fixture_document()?;
    let problem = pack.compile(&document, &fixture_context())?;
    let mut observation = observe_fixture_candidate(pack, &document, &problem, 0b001)?;
    let early_rule = RuleId::from_str(FIXTURE_RULE_EARLY)?;
    let mut mutated_rules = observation.report.required_rule_results.clone();
    mutated_rules
        .iter_mut()
        .find(|evaluation| evaluation.rule_id == early_rule)
        .ok_or("missing early verifier rule")?
        .satisfied = false;
    let verification_context =
        fixture_verification_context(&pack, &document, &problem, &observation.solution)?;
    observation.report = VerificationReport::new(
        &verification_context,
        mutated_rules,
        observation.report.score.clone(),
        Vec::new(),
        BTreeMap::new(),
    )?;

    let Err(error) = assert_fixture_conformance(0b001, &observation) else {
        return Err("the oracle accepted a one-sided verifier rule mutation".into());
    };
    assert!(error.to_string().contains("verifier rule mismatch"));
    Ok(())
}

#[test]
fn conformance_rejects_one_sided_exact_projection_mutation() -> Result<(), Box<dyn Error>> {
    let pack = IntervalFixturePack::with_pruning(false);
    let document = fixture_document()?;
    let mut problem = pack.compile(&document, &fixture_context())?;
    let projection_id = fixture_projection_id(OPTIONS[0])?;
    let projection = problem
        .projections
        .iter_mut()
        .find(|projection| projection.id == projection_id)
        .ok_or("missing early projection")?;
    projection.expression = ProjectionExpression::Constant(AssignmentValue::Interval(
        eutheto_domain_ir::AssignedInterval::new(0, 1, 1)?,
    ));

    let observation = observe_fixture_candidate(pack, &document, &problem, 0b001)?;
    let Err(error) = assert_fixture_conformance(0b001, &observation) else {
        return Err("the oracle accepted a one-sided exact projection mutation".into());
    };
    assert!(error.to_string().contains("exact projection mismatch"));
    Ok(())
}

#[test]
fn conformance_rejects_one_sided_in_bounds_score_mutation() -> Result<(), Box<dyn Error>> {
    let pack = IntervalFixturePack::with_pruning(false);
    let document = fixture_document()?;
    let problem = pack.compile(&document, &fixture_context())?;
    let mut observation = observe_fixture_candidate(pack, &document, &problem, 0)?;
    let mutated_value = 1_i64;
    assert!(mutated_value <= problem.objectives.levels[0].upper_bound);
    observation.score.levels[0].value = mutated_value;
    observation.score.levels[0]
        .category_breakdown
        .insert(ScoreCategoryId::new(FIXTURE_SCORE_CATEGORY)?, mutated_value);

    let Err(error) = assert_fixture_conformance(0, &observation) else {
        return Err("the oracle accepted a one-sided in-bounds score mutation".into());
    };
    assert!(error.to_string().contains("authoritative score mismatch"));
    Ok(())
}

const MUTATION_ENTITY_ID: &str = "0195a5e4-7c00-7000-8000-000000000013";

fn mutation_fixture() -> Result<(ScenarioDocument, CommandEnvelope), Box<dyn Error>> {
    let mut document = fixture_document()?;
    document.domain.entities.insert(
        MUTATION_ENTITY_ID.parse()?,
        json!({"id": MUTATION_ENTITY_ID, "enabled": true, "target": 1}),
    );
    let envelope = CommandEnvelope {
        command_id: "0195a5e4-7c00-7000-8000-000000000014".parse()?,
        scenario_id: document.scenario_id,
        expected_revision: Revision::INITIAL,
        actor: ActorRef {
            actor_id: None,
            display_name: "Contract test".to_owned(),
        },
        source: CommandSource::System,
        command: eutheto_types::ScenarioCommand::ApplyDomainCommand(DomainCommandEnvelope {
            command_type: "official.test.configure_entity".to_owned(),
            payload: json!({"entityId": MUTATION_ENTITY_ID, "enabled": true, "target": 3}),
        }),
    };
    Ok((document, envelope))
}

#[test]
fn command_boundary_rejects_independent_mutation_contract_faults() -> Result<(), Box<dyn Error>> {
    let faults: &[(&str, MutationOverride)] = &[
        ("result count", |mutation, _| {
            mutation.results.clear();
            Ok(())
        }),
        ("result schema", |mutation, _| {
            mutation.results[0] = json!({});
            Ok(())
        }),
        ("change index", |mutation, _| {
            mutation.changes[0].command_index = 1;
            Ok(())
        }),
        ("change schema", |mutation, _| {
            mutation.changes[0]
                .value
                .as_object_mut()
                .ok_or_else(|| contract("change"))?
                .remove("after");
            Ok(())
        }),
        ("inverse count", |mutation, _| {
            mutation.inverse.commands.clear();
            Ok(())
        }),
        ("inverse version", |mutation, _| {
            mutation.inverse.schema_version = 2;
            Ok(())
        }),
        ("inverse pack", |mutation, _| {
            mutation.inverse.pack_id = "official.other".parse().map_err(contract)?;
            Ok(())
        }),
        ("inverse scenario schema", |mutation, _| {
            mutation.inverse.scenario_schema_version = 2;
            Ok(())
        }),
        ("inverse command", |mutation, _| {
            mutation.inverse.commands[0].command_type = "official.test.unknown".to_owned();
            Ok(())
        }),
        ("inverse payload", |mutation, _| {
            mutation.inverse.commands[0].payload["target"] = json!("invalid");
            Ok(())
        }),
        ("host identity", |mutation, _| {
            mutation.document.scenario_id = MUTATION_ENTITY_ID.parse().map_err(contract)?;
            Ok(())
        }),
        ("host version", |mutation, _| {
            mutation.document.format_version = 2;
            Ok(())
        }),
        ("host pack", |mutation, _| {
            mutation.document.domain_pack.schema_version = 2;
            Ok(())
        }),
        ("host metadata", |mutation, _| {
            mutation.document.metadata.title = "Changed".to_owned();
            Ok(())
        }),
        ("host settings", |mutation, _| {
            mutation.document.settings.locale = "sv-SE".parse().map_err(contract)?;
            Ok(())
        }),
        ("host extensions", |mutation, _| {
            mutation
                .document
                .extensions
                .insert("official.test.marker".to_owned(), json!(true));
            Ok(())
        }),
        ("change count", |mutation, _| {
            mutation.changes = vec![mutation.changes[0].clone(); MAX_DOMAIN_MUTATION_CHANGES + 1];
            Ok(())
        }),
        ("aggregate change bytes", |mutation, _| {
            mutation.changes[0].value["before"] =
                json!({"notes": vec!["public ".repeat(2_000); 640]});
            mutation.changes = vec![mutation.changes[0].clone(); 8];
            Ok(())
        }),
    ];
    assert_mutation_faults_rejected(faults)
}

fn assert_mutation_faults_rejected(
    faults: &[(&str, MutationOverride)],
) -> Result<(), Box<dyn Error>> {
    for &(name, mutate) in faults {
        let (document, envelope) = mutation_fixture()?;
        let original = document.clone();
        let registry = DomainPackRegistry::builder()
            .register(IntervalFixturePack {
                prune: false,
                mutation_override: Some(mutate),
                large_values: false,
            })
            .build()?;
        let result = apply_command_with_registry(
            &document,
            Revision::INITIAL,
            &envelope,
            &registry,
            &CancellationToken::new(),
        );
        assert!(result.is_err(), "{name} was accepted");
        assert_eq!(document, original, "{name} changed the input");
    }
    Ok(())
}

#[test]
fn command_boundary_rejects_inputs_even_when_the_pack_ignores_them() -> Result<(), Box<dyn Error>> {
    let registry = DomainPackRegistry::builder()
        .register(IntervalFixturePack {
            prune: false,
            mutation_override: Some(|_, _| Ok(())),
            large_values: false,
        })
        .build()?;
    let (document, mut envelope) = mutation_fixture()?;
    let eutheto_types::ScenarioCommand::ApplyDomainCommand(command) = &mut envelope.command else {
        return Err("expected domain command".into());
    };
    command.payload["target"] = json!("invalid");
    assert!(matches!(
        apply_command_with_registry(
            &document,
            Revision::INITIAL,
            &envelope,
            &registry,
            &CancellationToken::new(),
        ),
        Err(CommandError::InvalidDomainPayload { .. })
    ));
    let eutheto_types::ScenarioCommand::ApplyDomainCommand(command) = &mut envelope.command else {
        return Err("expected domain command".into());
    };
    command.command_type = "official.test.unknown".to_owned();
    assert!(matches!(
        apply_command_with_registry(
            &document,
            Revision::INITIAL,
            &envelope,
            &registry,
            &CancellationToken::new(),
        ),
        Err(CommandError::Unsupported { .. })
    ));
    Ok(())
}

#[test]
fn command_boundary_observes_cancellation_after_a_pack_returns() -> Result<(), Box<dyn Error>> {
    let registry = DomainPackRegistry::builder()
        .register(IntervalFixturePack {
            prune: false,
            large_values: false,
            mutation_override: Some(|_, cancellation| {
                cancellation.cancel();
                Ok(())
            }),
        })
        .build()?;
    let (document, envelope) = mutation_fixture()?;
    assert_eq!(
        apply_command_with_registry(
            &document,
            Revision::INITIAL,
            &envelope,
            &registry,
            &CancellationToken::new(),
        ),
        Err(CommandError::Cancelled),
    );
    Ok(())
}

fn mutation_batch(
    command: eutheto_types::ScenarioCommand,
    count: usize,
) -> eutheto_types::ScenarioCommand {
    eutheto_types::ScenarioCommand::ApplyBatch(eutheto_types::CommandBatch {
        label: Some("Output budget".to_owned()),
        commands: vec![command; count],
    })
}

#[test]
fn command_boundary_rejects_valid_index_changes_returned_out_of_order() -> Result<(), Box<dyn Error>>
{
    let registry = DomainPackRegistry::builder()
        .register(IntervalFixturePack {
            prune: false,
            large_values: false,
            mutation_override: Some(|mutation, _| {
                mutation.changes.swap(0, 1);
                Ok(())
            }),
        })
        .build()?;
    let (document, mut envelope) = mutation_fixture()?;
    envelope.command = mutation_batch(envelope.command, 2);
    assert!(
        matches!(apply_command_with_registry(&document, Revision::INITIAL, &envelope,
        &registry, &CancellationToken::new()),
        Err(CommandError::Validation { path, .. }) if path == "/domainMutation")
    );
    Ok(())
}

#[test]
fn late_nested_pack_cancellation_discards_earlier_successful_effects() -> Result<(), Box<dyn Error>>
{
    let registry = DomainPackRegistry::builder()
        .register(IntervalFixturePack {
            prune: false,
            large_values: false,
            mutation_override: Some(|mutation, cancellation| {
                if mutation.changes[0].value["before"]["target"] == json!(3) {
                    cancellation.cancel();
                }
                Ok(())
            }),
        })
        .build()?;
    let (document, mut envelope) = mutation_fixture()?;
    let original = document.clone();
    envelope.command = mutation_batch(mutation_batch(envelope.command, 1), 2);
    assert_eq!(
        apply_command_with_registry(
            &document,
            Revision::INITIAL,
            &envelope,
            &registry,
            &CancellationToken::new()
        ),
        Err(CommandError::Cancelled)
    );
    assert_eq!(document, original);
    Ok(())
}

#[test]
fn cumulative_mutation_outputs_are_bounded_across_nested_runs() -> Result<(), Box<dyn Error>> {
    let cases: &[(&str, MutationOverride, usize, bool)] = &[
        (
            "change count",
            |mutation, _| {
                mutation.changes = vec![mutation.changes[0].clone(); 43_000];
                Ok(())
            },
            2,
            false,
        ),
        (
            "change bytes",
            |mutation, _| {
                mutation.changes[0].value["before"] =
                    json!({"notes": vec!["public ".repeat(2_000); 640]});
                mutation.changes = vec![mutation.changes[0].clone(); 4];
                Ok(())
            },
            2,
            false,
        ),
        (
            "result bytes",
            |mutation, _| {
                mutation.results[0]["notes"] = json!(vec!["public ".repeat(2_000); 480]);
                Ok(())
            },
            3,
            true,
        ),
        (
            "generic inverse bytes",
            |mutation, _| {
                mutation.inverse.commands[0].payload["notes"] =
                    json!(vec!["public ".repeat(2_000); 640]);
                mutation.inverse.validate_inverse_bounds()
            },
            8,
            true,
        ),
    ];
    for &(name, mutate, count, large_values) in cases {
        let registry = DomainPackRegistry::builder()
            .register(IntervalFixturePack {
                prune: false,
                mutation_override: Some(mutate),
                large_values,
            })
            .build()?;
        let (document, mut envelope) = mutation_fixture()?;
        let original = document.clone();
        // Prove each individual output is valid; rejection must be cumulative,
        // not a schema or per-call output failure.
        let single = apply_command_with_registry(
            &document,
            Revision::INITIAL,
            &envelope,
            &registry,
            &CancellationToken::new(),
        )?;
        assert_eq!(
            single.document.domain.entities[&MUTATION_ENTITY_ID.parse()?]["target"],
            json!(3)
        );
        envelope.command = mutation_batch(mutation_batch(envelope.command, 1), count);
        assert!(
            apply_command_with_registry(
                &document,
                Revision::INITIAL,
                &envelope,
                &registry,
                &CancellationToken::new()
            )
            .is_err(),
            "{name} was not bounded"
        );
        assert_eq!(document, original);
    }
    Ok(())
}

#[test]
fn semantic_pack_errors_are_not_retried_as_smaller_chunks() -> Result<(), Box<dyn Error>> {
    let registry = DomainPackRegistry::builder()
        .register(IntervalFixturePack {
            prune: false,
            large_values: false,
            mutation_override: Some(|mutation, _| {
                if mutation.results.len() > 1 {
                    return Err(DomainPackError::InvalidPayload {
                        path: "/commands/1".to_owned(),
                        message: "invalid second prefix".to_owned(),
                    });
                }
                Ok(())
            }),
        })
        .build()?;
    let (document, mut envelope) = mutation_fixture()?;
    envelope.command = mutation_batch(envelope.command, 2);
    assert!(
        matches!(apply_command_with_registry(&document, Revision::INITIAL, &envelope,
        &registry, &CancellationToken::new()),
        Err(CommandError::InvalidDomainBatchPayload { path, .. }) if path == "/commands/1")
    );
    Ok(())
}

#[test]
fn typed_inverse_overflow_is_bisected_and_byte_chunked_replay_restores_state()
-> Result<(), Box<dyn Error>> {
    let pack = IntervalFixturePack {
        prune: false,
        large_values: true,
        mutation_override: Some(|mutation, _| {
            for inverse in &mut mutation.inverse.commands {
                inverse.payload["notes"] = json!(vec!["public ".repeat(2_000); 640]);
            }
            // Exercise the real typed byte check, rather than simulating it by call size.
            mutation.inverse.validate_inverse_bounds()
        }),
    };
    let registry = DomainPackRegistry::builder().register(pack).build()?;
    let (document, mut envelope) = mutation_fixture()?;
    let eutheto_types::ScenarioCommand::ApplyDomainCommand(first) = envelope.command else {
        return Err("expected domain command".into());
    };
    let mut second = first.clone();
    second.payload["target"] = json!(4);
    let direct = DomainBatchCommand {
        schema_version: eutheto_domain_api::DOMAIN_BATCH_SCHEMA_VERSION,
        pack_id: document.domain_pack.id.clone(),
        scenario_schema_version: document.domain_pack.schema_version,
        label: None,
        commands: vec![first, second],
    };
    direct.validate_bounds()?;
    assert!(matches!(
        pack.apply_batch(&document, &direct, &CancellationToken::new()),
        Err(DomainPackError::BatchInverseTooLarge)
    ));
    envelope.command = eutheto_types::ScenarioCommand::ApplyBatch(eutheto_types::CommandBatch {
        label: Some("inverse amplification".to_owned()),
        commands: direct
            .commands
            .into_iter()
            .map(eutheto_types::ScenarioCommand::ApplyDomainCommand)
            .collect(),
    });
    let applied = apply_command_with_registry(
        &document,
        Revision::INITIAL,
        &envelope,
        &registry,
        &CancellationToken::new(),
    )?;
    let changes = &applied.result.change_set.changes;
    assert_eq!(changes.len(), 2);
    assert_eq!(
        changes[0].before.as_ref().ok_or("missing before")?["target"],
        json!(1)
    );
    assert_eq!(
        changes[0].after.as_ref().ok_or("missing after")?["target"],
        json!(3)
    );
    assert_eq!(
        changes[1].before.as_ref().ok_or("missing before")?["target"],
        json!(3)
    );
    assert_eq!(
        changes[1].after.as_ref().ok_or("missing after")?["target"],
        json!(4)
    );
    let inverse = applied.result.inverse.ok_or("missing inverse")?;
    assert!(serde_json::to_vec(&inverse)?.len() > 16 * 1024 * 1024);
    let eutheto_types::ScenarioCommand::ApplyBatch(inverse_batch) = &inverse else {
        return Err("expected original batch wrapper".into());
    };
    assert_eq!(
        inverse_batch.label.as_deref(),
        Some("inverse amplification")
    );
    assert_eq!(inverse_batch.commands.len(), 2);
    for (command, target) in inverse_batch.commands.iter().zip([3, 1]) {
        let eutheto_types::ScenarioCommand::ApplyDomainCommand(command) = command else {
            return Err("unexpected extra inverse batch layer".into());
        };
        assert_eq!(command.payload["target"], json!(target));
    }
    // This replay has only ~1300 nodes, so the byte ceiling, not node packing,
    // requires splitting its two large envelopes.
    envelope.command = inverse;
    envelope.expected_revision = applied.result.new_revision;
    let undone = apply_command_with_registry(
        &applied.document,
        applied.result.new_revision,
        &envelope,
        &registry,
        &CancellationToken::new(),
    )?;
    assert_eq!(undone.document, document);
    Ok(())
}

#[test]
fn domain_input_node_limit_includes_header_and_envelope_nodes() -> Result<(), Box<dyn Error>> {
    let registry = DomainPackRegistry::builder()
        .register(IntervalFixturePack {
            prune: false,
            large_values: true,
            mutation_override: Some(|_, _| Ok(())),
        })
        .build()?;
    let (document, mut envelope) = mutation_fixture()?;
    let eutheto_types::ScenarioCommand::ApplyDomainCommand(command) = &mut envelope.command else {
        return Err("expected domain command".into());
    };
    // Six header nodes plus two envelope nodes plus five payload nodes.
    command.payload["notes"] = json!(vec![""; 100_000 - 13]);
    let applied = apply_command_with_registry(
        &document,
        Revision::INITIAL,
        &envelope,
        &registry,
        &CancellationToken::new(),
    )?;
    assert_eq!(
        applied.result.change_set.changes[0]
            .after
            .as_ref()
            .ok_or("missing after")?["target"],
        json!(3)
    );
    let eutheto_types::ScenarioCommand::ApplyDomainCommand(command) = &mut envelope.command else {
        return Err("expected domain command".into());
    };
    command.payload["notes"]
        .as_array_mut()
        .ok_or("expected notes array")?
        .push(json!(""));
    assert!(matches!(
        apply_command_with_registry(
            &document,
            Revision::INITIAL,
            &envelope,
            &registry,
            &CancellationToken::new()
        ),
        Err(CommandError::InvalidDomainPayload { .. })
    ));
    Ok(())
}
