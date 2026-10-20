use crate::ortools_descriptor;
use eutheto_planning_ir::PlanningProblemSummary;
use eutheto_solver_api::{
    CapabilityMatrix, CompatibilityLevel, CompatibilityReport, PreflightError, SupportFeatureId,
    UnsupportedFeature, compatibility_for,
};
use eutheto_types::SolveOptions;

/// Produces the OR-Tools compatibility report from the immutable planning summary,
/// solve options, and authoritative production support matrix.
///
/// Descriptor-to-matrix agreement is checked first so callers cannot receive a
/// report from stale or mismatched capability metadata.
///
/// # Errors
///
/// Returns an error when the reviewed descriptor is invalid, its production
/// matrix column is missing or mismatched, or an internal feature identifier
/// cannot be represented by the shared solver API.
pub fn ortools_compatibility(
    matrix: &CapabilityMatrix,
    summary: &PlanningProblemSummary,
    options: &SolveOptions,
) -> Result<CompatibilityReport, PreflightError> {
    let descriptor = ortools_descriptor()
        .map_err(|error| PreflightError::InvalidDescriptor(error.to_string()))?;
    matrix
        .validate_descriptor(&descriptor)
        .map_err(|error| PreflightError::Matrix(error.to_string()))?;
    let mut report = compatibility_for(matrix, &descriptor.id, summary, options)?;
    if summary.assumption_count > 0 {
        if summary.objective_level_count > 0 {
            report.unsupported_features.push(UnsupportedFeature {
                feature_id: support_feature("ir.assumptions")?,
                usage_count: summary.assumption_count,
                path: "planningProblem.objectives".to_owned(),
                reason: "The sufficient-assumption diagnostic profile is available only for satisfaction models; an objective level remains objective-bearing even when it has no terms.".to_owned(),
                remediation: "Remove all objective levels or choose a backend that supports assumption diagnostics with objectives.".to_owned(),
            });
        }
        if !matches!(
            options.worker_threads,
            eutheto_types::WorkerThreadPolicy::Exact(1)
        ) {
            report.unsupported_features.push(UnsupportedFeature {
                feature_id: support_feature("solve.infeasibility-evidence")?,
                usage_count: 1,
                path: "solveOptions.workerThreads".to_owned(),
                reason: "The sufficient-assumption diagnostic profile requires exactly one worker thread; automatic worker selection is not admitted.".to_owned(),
                remediation: "Set workerThreads to exact count 1 or choose a backend with multi-worker assumption diagnostics.".to_owned(),
            });
        }
    }
    if options.memory_limit_bytes.is_some() || options.solution_limit.is_some() {
        let feature_id = support_feature("solve.resource-limits")?;
        report
            .warnings
            .retain(|warning| warning.feature_id != feature_id);
        report.unsupported_features.push(UnsupportedFeature {
            feature_id,
            usage_count: 1,
            path: if options.memory_limit_bytes.is_some() {
                "solveOptions.memoryLimitBytes"
            } else {
                "solveOptions.solutionLimit"
            }
            .to_owned(),
            reason: "The Phase 03 adapter does not enforce requested process-memory or candidate-count limits."
                .to_owned(),
            remediation: "Remove the unsupported limit or choose a backend that enforces it."
                .to_owned(),
        });
    }
    if options.random_seed > i32::MAX as u64 {
        report.unsupported_features.push(UnsupportedFeature {
            feature_id: support_feature("solve.deterministic-mode")?,
            usage_count: 1,
            path: "solveOptions.randomSeed".to_owned(),
            reason: "OR-Tools accepts only signed 32-bit random seeds.".to_owned(),
            remediation: "Choose a random seed no greater than 2147483647.".to_owned(),
        });
    }
    if matches!(
        options.worker_threads,
        eutheto_types::WorkerThreadPolicy::Exact(count)
            if u32::from(count) > eutheto_protocol::MAX_ORTOOLS_WORKER_THREADS
    ) {
        report.unsupported_features.push(UnsupportedFeature {
            feature_id: support_feature("solve.resource-limits")?,
            usage_count: 1,
            path: "solveOptions.workerThreads".to_owned(),
            reason: "The requested worker count exceeds the pinned worker protocol limit."
                .to_owned(),
            remediation: "Choose at most 10000 worker threads.".to_owned(),
        });
    }
    if matches!(
        options.reproducibility,
        eutheto_types::ReproducibilityMode::Deterministic
    ) && (options.random_seed != 1
        || !matches!(
            options.worker_threads,
            eutheto_types::WorkerThreadPolicy::Exact(1)
        ))
    {
        report.unsupported_features.push(UnsupportedFeature {
            feature_id: support_feature("solve.deterministic-mode")?,
            usage_count: 1,
            path: "solveOptions.reproducibility".to_owned(),
            reason: "The pinned deterministic worker profile requires random seed 1 and exactly one worker thread."
                .to_owned(),
            remediation: "Set randomSeed to 1 and workerThreads to exact count 1.".to_owned(),
        });
    }
    if !report.unsupported_features.is_empty() {
        report.unsupported_features.sort_by(|left, right| {
            left.feature_id
                .cmp(&right.feature_id)
                .then_with(|| left.path.cmp(&right.path))
        });
        report.level = CompatibilityLevel::Unsupported;
    }
    Ok(report)
}

fn support_feature(id: &str) -> Result<SupportFeatureId, PreflightError> {
    SupportFeatureId::new(id).map_err(|error| PreflightError::InternalFeature(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use eutheto_planning_ir::{
        Capability, ComponentAnalysis, FeatureUsageManifest, LexicographicStrategy,
        PLANNING_IR_SCHEMA_VERSION,
    };
    use eutheto_solver_api::{
        BackendSupportColumn, CompatibilityLevel, SUPPORT_FEATURES,
        SUPPORT_MATRIX_IR_SCHEMA_VERSION, SUPPORT_MATRIX_SCHEMA_VERSION, SupportCell,
        SupportFeature, SupportFeatureCategory, SupportFeatureGate, SupportFeatureId,
        SupportMatrixError,
    };
    use eutheto_types::{
        BackendSelection, DurationMillis, ExplanationMode, PreservationPolicy, ReproducibilityMode,
        ResourceLimits, SolveMode, WorkerThreadPolicy,
    };
    use std::collections::BTreeSet;
    use std::error::Error;

    type TestResult = Result<(), Box<dyn Error>>;

    fn matrix() -> Result<CapabilityMatrix, Box<dyn Error>> {
        matrix_with_backend_version(crate::ORTOOLS_VERSION)
    }

    fn matrix_with_backend_version(
        backend_version: &str,
    ) -> Result<CapabilityMatrix, Box<dyn Error>> {
        let descriptor = ortools_descriptor()?;
        let features = SUPPORT_FEATURES
            .iter()
            .map(|(id, category, gate)| {
                let category = match *category {
                    "primitive" => SupportFeatureCategory::Primitive,
                    "objective" => SupportFeatureCategory::Objective,
                    "projection" => SupportFeatureCategory::Projection,
                    "solve" => SupportFeatureCategory::Solve,
                    other => {
                        return Err(SupportMatrixError::UnknownGeneratedCategory(
                            other.to_owned(),
                        ));
                    }
                };
                Ok(SupportFeature {
                    id: SupportFeatureId::new(*id)?,
                    category,
                    gate: if *gate == "unconditional" {
                        SupportFeatureGate::Unconditional
                    } else {
                        SupportFeatureGate::Enabled((*gate).to_owned())
                    },
                })
            })
            .collect::<Result<Vec<_>, SupportMatrixError>>()?;
        let cells = features
            .iter()
            .map(|feature| {
                let cell = if descriptor.capabilities.supported.contains(&feature.id) {
                    SupportCell::Supported {
                        fixture_id: "ortools.compatibility.supported".to_owned(),
                    }
                } else if descriptor.capabilities.degraded.contains(&feature.id) {
                    SupportCell::Degraded {
                        restriction_id: "single-objective-level-bound".to_owned(),
                        reason: "Only one objective level has exact bound evidence.".to_owned(),
                        remediation: "Use one objective level.".to_owned(),
                        fixture_id: "ortools.compatibility.degraded".to_owned(),
                    }
                } else {
                    SupportCell::Unsupported {
                        reason: "This feature is outside the Phase 03 OR-Tools subset.".to_owned(),
                        remediation: "Use only the documented Phase 03 subset.".to_owned(),
                        fixture_id: "ortools.compatibility.unsupported".to_owned(),
                    }
                };
                (feature.id.clone(), cell)
            })
            .collect();
        Ok(CapabilityMatrix::new(
            SUPPORT_MATRIX_SCHEMA_VERSION,
            SUPPORT_MATRIX_IR_SCHEMA_VERSION,
            features,
            vec![BackendSupportColumn {
                backend_id: descriptor.id,
                backend_version: backend_version.to_owned(),
                adapter_version: descriptor.adapter_version,
                cells,
            }],
            Vec::new(),
        )?)
    }

    fn summary() -> PlanningProblemSummary {
        PlanningProblemSummary {
            schema_version: PLANNING_IR_SCHEMA_VERSION,
            variable_count: 0,
            bool_variable_count: 0,
            int_variable_count: 0,
            interval_variable_count: 0,
            constraint_count: 0,
            assumption_count: 0,
            objective_level_count: 0,
            objective_term_count: 0,
            lexicographic_strategy: LexicographicStrategy::ExactScalarization {
                weights: Vec::new(),
            },
            projection_count: 0,
            provenance_count: 0,
            domain_range_count: 0,
            total_reference_count: 0,
            min_coefficient: None,
            max_coefficient: None,
            manifest: FeatureUsageManifest::default(),
            components: ComponentAnalysis {
                components: Vec::new(),
                edge_count: 0,
                component_hash: "fixture-component-hash".to_owned(),
            },
            canonical_ir_hash: "fixture-ir-hash".to_owned(),
        }
    }

    fn options() -> Result<SolveOptions, Box<dyn Error>> {
        Ok(SolveOptions {
            backend: BackendSelection::Specific(ortools_descriptor()?.id),
            mode: SolveMode::Balanced,
            time_limit_milliseconds: DurationMillis::new(1_000)?,
            memory_limit_bytes: None,
            worker_threads: WorkerThreadPolicy::Exact(1),
            random_seed: 1,
            solution_limit: None,
            stop_after_first_feasible: false,
            collect_intermediate_solutions: false,
            explanation_mode: ExplanationMode::None,
            preserve_existing: PreservationPolicy::None,
            reproducibility: ReproducibilityMode::Deterministic,
            resource_limits: ResourceLimits {
                max_entities: 100,
                max_rules: 100,
                max_variables: 100,
                max_constraints: 100,
            },
        })
    }

    #[test]
    fn report_rejects_unsupported_features_and_admits_bounded_intermediate_solutions() -> TestResult
    {
        let matrix = matrix()?;
        let baseline = ortools_compatibility(&matrix, &summary(), &options()?)?;
        assert_eq!(baseline.level, CompatibilityLevel::Degraded);
        assert!(baseline.unsupported_features.is_empty());
        assert_eq!(baseline.warnings.len(), 1);
        assert_eq!(
            baseline.warnings[0].feature_id.as_str(),
            "solve.resource-limits"
        );
        let mut objective_summary = summary();
        objective_summary.objective_level_count = 1;
        objective_summary.objective_term_count = 1;
        objective_summary.lexicographic_strategy =
            LexicographicStrategy::ExactScalarization { weights: vec![1] };
        objective_summary
            .manifest
            .capability_counts
            .insert(Capability::ObjectivePenalty, 1);
        let objective = ortools_compatibility(&matrix, &objective_summary, &options()?)?;
        assert_eq!(objective.level, CompatibilityLevel::Degraded);
        assert_eq!(
            objective
                .warnings
                .iter()
                .map(|warning| warning.feature_id.as_str())
                .collect::<BTreeSet<_>>(),
            BTreeSet::from(["solve.proof-and-bounds", "solve.resource-limits"])
        );

        objective_summary.objective_level_count = 2;
        objective_summary.lexicographic_strategy = LexicographicStrategy::ExactScalarization {
            weights: vec![2, 1],
        };
        let multi_level = ortools_compatibility(&matrix, &objective_summary, &options()?)?;
        assert_eq!(multi_level.level, CompatibilityLevel::Degraded);
        assert_eq!(
            multi_level
                .warnings
                .iter()
                .map(|warning| warning.feature_id.as_str())
                .collect::<BTreeSet<_>>(),
            BTreeSet::from(["solve.proof-and-bounds", "solve.resource-limits"])
        );
        assert!(multi_level.unsupported_features.is_empty());

        objective_summary.objective_level_count = 1;
        objective_summary.lexicographic_strategy = LexicographicStrategy::Multipass;
        let multipass = ortools_compatibility(&matrix, &objective_summary, &options()?)?;
        assert_eq!(multipass.level, CompatibilityLevel::Unsupported);
        assert_eq!(multipass.unsupported_features.len(), 1);
        assert_eq!(
            multipass.unsupported_features[0].feature_id.as_str(),
            "ir.multipass-objectives"
        );

        let mut unsupported_summary = summary();
        unsupported_summary
            .manifest
            .capability_counts
            .insert(Capability::IntervalProjection, 2);
        unsupported_summary.projection_count = 2;
        let unsupported = ortools_compatibility(&matrix, &unsupported_summary, &options()?)?;
        assert_eq!(unsupported.level, CompatibilityLevel::Unsupported);
        assert_eq!(unsupported.unsupported_features.len(), 1);
        assert_eq!(
            unsupported.unsupported_features[0].feature_id.as_str(),
            "projection.interval"
        );
        assert_eq!(unsupported.unsupported_features[0].usage_count, 2);

        let mut optional_features = options()?;
        optional_features.collect_intermediate_solutions = true;
        let production_matrix = CapabilityMatrix::generated()?;
        let intermediate =
            ortools_compatibility(&production_matrix, &summary(), &optional_features)?;
        assert_eq!(intermediate.level, CompatibilityLevel::Degraded);
        assert!(intermediate.unsupported_features.is_empty());
        assert!(intermediate.warnings.iter().any(|warning| {
            warning.feature_id.as_str() == "solve.intermediate-candidates"
                && warning.restriction_id == "first-and-final-only"
        }));
        optional_features.explanation_mode = ExplanationMode::Standard;
        let ordinary_explanation =
            ortools_compatibility(&production_matrix, &summary(), &optional_features)?;
        assert!(
            !ordinary_explanation
                .unsupported_features
                .iter()
                .any(|feature| feature.feature_id.as_str() == "solve.infeasibility-evidence")
        );
        let mut unsupported_memory = options()?;
        unsupported_memory.memory_limit_bytes = Some(1024);
        let unsupported = ortools_compatibility(&matrix, &summary(), &unsupported_memory)?;
        assert_eq!(unsupported.level, CompatibilityLevel::Unsupported);
        assert!(unsupported.warnings.is_empty());
        assert_eq!(unsupported.unsupported_features.len(), 1);
        assert_eq!(
            unsupported.unsupported_features[0].feature_id.as_str(),
            "solve.resource-limits"
        );
        Ok(())
    }

    #[test]
    fn assumption_profile_rejects_objectives_and_non_single_worker_requests() -> TestResult {
        let matrix = matrix()?;
        let mut assumptions = summary();
        assumptions.assumption_count = 1;
        assumptions
            .manifest
            .capability_counts
            .insert(Capability::Assumptions, 1);
        let mut objective = assumptions.clone();
        objective.objective_level_count = 1;
        objective.lexicographic_strategy =
            LexicographicStrategy::ExactScalarization { weights: vec![1] };
        let report = ortools_compatibility(&matrix, &objective, &options()?)?;
        assert!(report.unsupported_features.iter().any(|feature| {
            feature.feature_id.as_str() == "ir.assumptions"
                && feature.path == "planningProblem.objectives"
        }));

        let mut auto_options = options()?;
        auto_options.worker_threads = WorkerThreadPolicy::Auto;
        let report = ortools_compatibility(&matrix, &assumptions, &auto_options)?;
        assert!(report.unsupported_features.iter().any(|feature| {
            feature.feature_id.as_str() == "solve.infeasibility-evidence"
                && feature.path == "solveOptions.workerThreads"
        }));

        let report = ortools_compatibility(&matrix, &assumptions, &options()?)?;
        assert!(!report.unsupported_features.iter().any(|feature| {
            feature.path == "planningProblem.objectives"
                || feature.path == "solveOptions.workerThreads"
        }));
        Ok(())
    }

    #[test]
    fn report_rejects_worker_parameters_outside_pinned_contract() -> TestResult {
        let matrix = matrix()?;

        let mut seed = options()?;
        seed.random_seed = i32::MAX as u64 + 1;
        let report = ortools_compatibility(&matrix, &summary(), &seed)?;
        assert_eq!(report.level, CompatibilityLevel::Unsupported);
        assert!(report.unsupported_features.iter().any(|feature| {
            feature.feature_id.as_str() == "solve.deterministic-mode"
                && feature.path == "solveOptions.randomSeed"
        }));

        let mut threads = options()?;
        threads.reproducibility = ReproducibilityMode::Performance;
        threads.worker_threads = WorkerThreadPolicy::Exact(10_001);
        let report = ortools_compatibility(&matrix, &summary(), &threads)?;
        assert_eq!(report.level, CompatibilityLevel::Unsupported);
        assert!(report.unsupported_features.iter().any(|feature| {
            feature.feature_id.as_str() == "solve.resource-limits"
                && feature.path == "solveOptions.workerThreads"
        }));

        let mut deterministic = options()?;
        deterministic.random_seed = 7;
        let report = ortools_compatibility(&matrix, &summary(), &deterministic)?;
        assert_eq!(report.level, CompatibilityLevel::Unsupported);
        assert!(report.unsupported_features.iter().any(|feature| {
            feature.feature_id.as_str() == "solve.deterministic-mode"
                && feature.path == "solveOptions.reproducibility"
        }));
        Ok(())
    }

    #[test]
    fn report_rejects_matrix_descriptor_drift() -> TestResult {
        let mismatched = matrix_with_backend_version("9.15.9999")?;
        assert!(matches!(
            ortools_compatibility(&mismatched, &summary(), &options()?),
            Err(PreflightError::Matrix(_))
        ));
        Ok(())
    }
}
