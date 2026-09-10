//! Captured setup facts and bounded, ephemeral full-validation lifecycle authority.

use super::{
    CancelOnDrop, EuthetoApp, FRAME_BYTES, check_cancelled, join_error, resource_limit_error,
    setup_domain_error,
};
use eutheto_domain_api::bounded_json_size;
use eutheto_types::{
    AppError, CancellationToken, OperationControl, OperationId, Revision, ScenarioId,
    ValidationIssue, ValidationReport, ValidationSeverity,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex, MutexGuard},
};

const MAX_READINESS_ENTRIES: usize = 16;
const MAX_FAST_ISSUES: usize = 200;
const MAX_FAST_BYTES: usize = 2 * 1024 * 1024;
const MAX_FULL_ISSUES: usize = 100_000;
const MAX_FULL_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScenarioStructureV1 {
    pub entities: u32,
    pub rules: u32,
    pub preferences: u32,
    pub locked_assignments: u32,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IssueCountsV1 {
    pub errors: u32,
    pub warnings: u32,
    pub information: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FastFindingsV1 {
    pub counts: IssueCountsV1,
    pub issues: Vec<ValidationIssue>,
    pub omitted: u32,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "state",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum FullValidationStateV1 {
    #[default]
    NotRun,
    Running {
        operation_id: OperationId,
        input_revision: Revision,
        stale: bool,
    },
    Completed {
        operation_id: OperationId,
        input_revision: Revision,
        stale: bool,
        counts: IssueCountsV1,
    },
    Failed {
        operation_id: OperationId,
        input_revision: Revision,
        stale: bool,
        code: String,
    },
    Cancelled {
        operation_id: OperationId,
        input_revision: Revision,
        stale: bool,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScenarioSetupStatusV2 {
    pub schema_version: u32,
    pub scenario_id: ScenarioId,
    pub revision: Revision,
    pub structure: ScenarioStructureV1,
    pub fast: FastFindingsV1,
    pub full: FullValidationStateV1,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScenarioSummaryV2 {
    pub schema_version: u32,
    pub scenario_id: ScenarioId,
    pub revision: Revision,
    pub title: String,
    pub structure: ScenarioStructureV1,
    pub fast: FastFindingsV1,
    pub full: FullValidationStateV1,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FullValidationResultV2 {
    pub schema_version: u32,
    pub scenario_id: ScenarioId,
    pub revision: Revision,
    pub report: ValidationReport,
}

#[derive(Default)]
pub(crate) struct ValidationReadiness(Mutex<ReadinessEntries>);

#[derive(Default)]
struct ReadinessEntries {
    sequence: u64,
    entries: VecDeque<ReadinessEntry>,
}

struct ReadinessEntry {
    scenario_id: ScenarioId,
    sequence: u64,
    state: FullValidationStateV1,
}

impl ValidationReadiness {
    fn lock(&self) -> Result<MutexGuard<'_, ReadinessEntries>, AppError> {
        self.0.lock().map_err(|_| {
            super::super::protocol_error(
                "scenario.validation_state_unavailable",
                "Validation lifecycle state is unavailable.",
                false,
            )
        })
    }

    fn begin(
        self: &Arc<Self>,
        scenario_id: ScenarioId,
        operation_id: OperationId,
        revision: Revision,
    ) -> Result<ValidationAttempt, AppError> {
        let mut cache = self.lock()?;
        let sequence = cache
            .sequence
            .checked_add(1)
            .ok_or_else(resource_limit_error)?;
        cache.sequence = sequence;
        cache
            .entries
            .retain(|entry| entry.scenario_id != scenario_id);
        if cache.entries.len() == MAX_READINESS_ENTRIES {
            cache.entries.pop_front();
        }
        cache.entries.push_back(ReadinessEntry {
            scenario_id,
            sequence,
            state: FullValidationStateV1::Running {
                operation_id,
                input_revision: revision,
                stale: false,
            },
        });
        Ok(ValidationAttempt {
            readiness: Arc::clone(self),
            sequence,
            operation_id,
            revision,
            finished: false,
        })
    }

    fn replace(&self, sequence: u64, state: FullValidationStateV1) -> Result<(), AppError> {
        if let Some(entry) = self
            .lock()?
            .entries
            .iter_mut()
            .find(|entry| entry.sequence == sequence)
        {
            entry.state = state;
        }
        Ok(())
    }

    fn snapshot(
        &self,
        scenario_id: ScenarioId,
        revision: Revision,
    ) -> Result<FullValidationStateV1, AppError> {
        let mut state = self
            .lock()?
            .entries
            .iter()
            .find(|entry| entry.scenario_id == scenario_id)
            .map(|entry| entry.state.clone())
            .unwrap_or_default();
        match &mut state {
            FullValidationStateV1::NotRun => {}
            FullValidationStateV1::Running {
                input_revision,
                stale,
                ..
            }
            | FullValidationStateV1::Completed {
                input_revision,
                stale,
                ..
            }
            | FullValidationStateV1::Failed {
                input_revision,
                stale,
                ..
            }
            | FullValidationStateV1::Cancelled {
                input_revision,
                stale,
                ..
            } => *stale = *input_revision != revision,
        }
        Ok(state)
    }
}

struct ValidationAttempt {
    readiness: Arc<ValidationReadiness>,
    sequence: u64,
    operation_id: OperationId,
    revision: Revision,
    finished: bool,
}

impl ValidationAttempt {
    fn cancelled(&self) -> FullValidationStateV1 {
        FullValidationStateV1::Cancelled {
            operation_id: self.operation_id,
            input_revision: self.revision,
            stale: false,
        }
    }

    fn finish(
        &mut self,
        result: &Result<(FullValidationResultV2, IssueCountsV1), AppError>,
    ) -> Result<(), AppError> {
        let state = match result {
            Ok((_, counts)) => FullValidationStateV1::Completed {
                operation_id: self.operation_id,
                input_revision: self.revision,
                stale: false,
                counts: *counts,
            },
            Err(AppError::Protocol(failure)) if failure.code == "operation.cancelled" => {
                self.cancelled()
            }
            Err(_) => FullValidationStateV1::Failed {
                operation_id: self.operation_id,
                input_revision: self.revision,
                stale: false,
                // The invoke returns the detailed typed failure; readiness retains no report.
                code: "scenario.full_validation_failed".to_owned(),
            },
        };
        self.readiness.replace(self.sequence, state)?;
        self.finished = true;
        Ok(())
    }
}

impl Drop for ValidationAttempt {
    fn drop(&mut self) {
        if !self.finished {
            let _ = self.readiness.replace(self.sequence, self.cancelled());
        }
    }
}

impl EuthetoApp {
    /// Returns captured structural facts, bounded fast feedback and revision-bound full state.
    /// Native callers hold admission through capture, blocking work and response publication.
    ///
    /// # Errors
    /// Rejects missing, unsupported, stale or cancelled sources and response resource limits.
    pub async fn setup_summary(
        &self,
        scenario_id: ScenarioId,
        expected_revision: Option<Revision>,
        cancellation: CancellationToken,
    ) -> Result<ScenarioSummaryV2, AppError> {
        let _cancel_on_drop = CancelOnDrop(cancellation.clone());
        let project = self
            .capture_setup_project(scenario_id, expected_revision, &cancellation)
            .await?;
        let registry = Arc::clone(&self.pack_registry);
        let readiness = Arc::clone(&self.validation_readiness);
        tokio::task::spawn_blocking(move || {
            check_cancelled(&cancellation)?;
            let domain = &project.document.domain;
            let structure = ScenarioStructureV1 {
                entities: count(domain.entities.len())?,
                rules: count(domain.rules.len())?,
                preferences: count(domain.preferences.len())?,
                locked_assignments: count(domain.locked_assignments.len())?,
            };
            let fast = bounded_fast_findings(
                super::super::validate(&project.document, &registry),
                &cancellation,
            )?;
            let result = ScenarioSummaryV2 {
                schema_version: 2,
                scenario_id,
                revision: project.summary.revision,
                title: project.summary.title,
                structure,
                fast,
                full: readiness.snapshot(scenario_id, project.summary.revision)?,
            };
            bounded_json_size(&result, MAX_FAST_BYTES + FRAME_BYTES)
                .map_err(|_| resource_limit_error())?;
            check_cancelled(&cancellation)?;
            Ok(result)
        })
        .await
        .map_err(join_error)?
    }

    /// Runs complete pack validation against one immutable revision outside scenario locks.
    /// Fast findings never substitute for this explicit attempt or its complete result.
    ///
    /// # Errors
    /// Returns exact-revision, cancellation, pack or resource failures; no partial successful report.
    pub async fn full_validate_setup(
        &self,
        scenario_id: ScenarioId,
        expected_revision: Revision,
        operation_id: OperationId,
        cancellation: CancellationToken,
    ) -> Result<FullValidationResultV2, AppError> {
        let _cancel_on_drop = CancelOnDrop(cancellation.clone());
        check_cancelled(&cancellation)?;
        let mut attempt =
            self.validation_readiness
                .begin(scenario_id, operation_id, expected_revision)?;
        let result = async {
            let project = self
                .capture_setup_project(scenario_id, Some(expected_revision), &cancellation)
                .await?;
            let registry = Arc::clone(&self.pack_registry);
            tokio::task::spawn_blocking(move || {
                check_cancelled(&cancellation)?;
                let pack = registry
                    .require(&project.document.domain_pack.id)
                    .map_err(|error| setup_domain_error(&error))?;
                let validated = pack
                    .validate_full(
                        &project.document,
                        &OperationControl::Cancellation(cancellation.clone()),
                    )
                    .map_err(|error| setup_domain_error(&error))?;
                if validated.issues.len() > MAX_FULL_ISSUES {
                    return Err(resource_limit_error());
                }
                let report = ValidationReport {
                    issues: validated.issues,
                };
                bounded_json_size(&report, MAX_FULL_BYTES).map_err(|_| resource_limit_error())?;
                let counts = issue_counts(&report.issues, &cancellation)?;
                let result = FullValidationResultV2 {
                    schema_version: 2,
                    scenario_id,
                    revision: project.summary.revision,
                    report,
                };
                bounded_json_size(&result, MAX_FULL_BYTES + FRAME_BYTES)
                    .map_err(|_| resource_limit_error())?;
                check_cancelled(&cancellation)?;
                Ok((result, counts))
            })
            .await
            .map_err(join_error)?
        }
        .await;
        attempt.finish(&result)?;
        result.map(|(report, _)| report)
    }
}

fn count(value: usize) -> Result<u32, AppError> {
    u32::try_from(value).map_err(|_| resource_limit_error())
}

fn issue_counts(
    issues: &[ValidationIssue],
    cancellation: &CancellationToken,
) -> Result<IssueCountsV1, AppError> {
    let mut counts = IssueCountsV1::default();
    for issue in issues {
        check_cancelled(cancellation)?;
        let counter = match issue.severity {
            ValidationSeverity::Error => &mut counts.errors,
            ValidationSeverity::Warning => &mut counts.warnings,
            ValidationSeverity::Info => &mut counts.information,
        };
        *counter = counter.checked_add(1).ok_or_else(resource_limit_error)?;
    }
    Ok(counts)
}

fn bounded_fast_findings(
    report: ValidationReport,
    cancellation: &CancellationToken,
) -> Result<FastFindingsV1, AppError> {
    let counts = issue_counts(&report.issues, cancellation)?;
    let mut result = FastFindingsV1 {
        counts,
        issues: Vec::new(),
        omitted: count(report.issues.len())?,
    };
    // Reserving the original omitted count's width remains safe as that count decreases.
    let mut bytes =
        bounded_json_size(&result, MAX_FAST_BYTES).map_err(|_| resource_limit_error())?;
    for issue in report.issues.into_iter().take(MAX_FAST_ISSUES) {
        check_cancelled(cancellation)?;
        let separator = usize::from(!result.issues.is_empty());
        let Some(remaining) = MAX_FAST_BYTES.checked_sub(bytes + separator) else {
            break;
        };
        let Ok(size) = bounded_json_size(&issue, remaining) else {
            break;
        };
        bytes += separator + size;
        result.issues.push(issue);
        result.omitted -= 1;
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use eutheto_types::SystemIdGenerator;

    fn finding(severity: ValidationSeverity, message: String) -> ValidationIssue {
        ValidationIssue {
            code: "fixture.finding".to_owned(),
            severity,
            message,
            field_path: None,
            resource: None,
        }
    }

    #[test]
    fn fast_feedback_counts_errors_beyond_the_retained_issue_prefix() -> Result<(), AppError> {
        let mut issues = vec![finding(ValidationSeverity::Warning, "Warning".to_owned()); 200];
        issues.push(finding(
            ValidationSeverity::Error,
            "Omitted error".to_owned(),
        ));
        let result = bounded_fast_findings(ValidationReport { issues }, &CancellationToken::new())?;
        assert_eq!(result.omitted, 1);
        assert_eq!(result.counts.errors, 1);
        assert_eq!(result.counts.warnings, 200);
        assert!(
            result
                .issues
                .iter()
                .all(|issue| issue.severity == ValidationSeverity::Warning)
        );
        Ok(())
    }

    #[test]
    fn fast_feedback_obeys_byte_budget_without_losing_omitted_counts()
    -> Result<(), Box<dyn std::error::Error>> {
        let report = ValidationReport {
            issues: vec![
                finding(ValidationSeverity::Warning, "x".repeat(1024 * 1024)),
                finding(ValidationSeverity::Error, "y".repeat(1024 * 1024)),
                finding(ValidationSeverity::Info, "Small later finding".to_owned()),
            ],
        };
        let result = bounded_fast_findings(report, &CancellationToken::new())
            .map_err(|error| std::io::Error::other(format!("{error:?}")))?;
        assert_eq!(result.omitted, 2);
        assert_eq!(result.issues.len(), 1);
        assert_eq!(
            result.counts,
            IssueCountsV1 {
                errors: 1,
                warnings: 1,
                information: 1
            }
        );
        assert!(serde_json::to_vec(&result)?.len() <= MAX_FAST_BYTES);
        Ok(())
    }

    #[test]
    fn evicted_validation_cannot_resurrect_readiness_on_late_completion()
    -> Result<(), Box<dyn std::error::Error>> {
        let cache = Arc::new(ValidationReadiness::default());
        let scenario_id = ScenarioId::new(&SystemIdGenerator)?;
        let operation_id = OperationId::new(&SystemIdGenerator)?;
        let boxed = |error: AppError| std::io::Error::other(format!("{error:?}"));
        let mut first = cache
            .begin(scenario_id, operation_id, Revision::INITIAL)
            .map_err(boxed)?;
        let mut retained = Vec::new();
        for _ in 0..MAX_READINESS_ENTRIES {
            retained.push(
                cache
                    .begin(
                        ScenarioId::new(&SystemIdGenerator)?,
                        OperationId::new(&SystemIdGenerator)?,
                        Revision::INITIAL,
                    )
                    .map_err(boxed)?,
            );
        }
        assert_eq!(
            cache
                .snapshot(scenario_id, Revision::INITIAL)
                .map_err(boxed)?,
            FullValidationStateV1::NotRun
        );
        first
            .finish(&Ok((
                FullValidationResultV2 {
                    schema_version: 2,
                    scenario_id,
                    revision: Revision::INITIAL,
                    report: ValidationReport::default(),
                },
                IssueCountsV1::default(),
            )))
            .map_err(boxed)?;
        assert_eq!(
            cache
                .snapshot(scenario_id, Revision::INITIAL)
                .map_err(boxed)?,
            FullValidationStateV1::NotRun
        );
        drop(retained);
        Ok(())
    }
}
