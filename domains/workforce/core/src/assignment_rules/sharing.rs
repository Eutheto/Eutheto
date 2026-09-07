use super::{
    authority::{contract, operation_error, revalidate_workforce_accepted},
    boundary::preflight,
    budget::{OperationBudget, count},
    identity::{IdentityKind, PlanningIdentities},
    projection::decode_workforce_assignment,
};
use crate::model::AssignmentPair;
use eutheto_domain_api::{
    ContractJsonLimits, DomainPackError, DomainShareResult, ShareResultOptions,
    ValidatedContractSchema,
};
use eutheto_domain_ir::{
    AcceptedResult, DomainAssignment, DomainAssignmentId, DomainEvidenceId, ScoreVector,
};
use eutheto_types::{PersonId, ScenarioDocument, ScenarioId, SolutionId};
use serde::Serialize;

const SHARE_SCHEMA: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../schemas/generated/workforce.share-result.schema.json"
));

#[derive(Serialize)]
struct ShareAssignment<'a> {
    #[serde(rename = "assignmentId")]
    assignment: &'a DomainAssignmentId,
    #[serde(rename = "personId")]
    person: PersonId,
    #[serde(rename = "shiftId")]
    shift: crate::ids::ShiftId,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ShareProvenance<'a> {
    source_document_hash: &'a str,
    verification_report_checksum: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SharePayload<'a> {
    schema_version: u32,
    scenario_id: ScenarioId,
    scenario_revision: u64,
    solution_id: SolutionId,
    accepted_result_checksum: &'a str,
    status: &'static str,
    provenance: ShareProvenance<'a>,
    privacy: ShareResultOptions,
    assignments: Vec<ShareAssignment<'a>>,
    score: &'a ScoreVector,
    #[serde(skip_serializing_if = "Option::is_none")]
    evidence_references: Option<Vec<DomainEvidenceId>>,
}

/// Builds a closed identity-only share after fresh complete original-domain verification.
///
/// UUIDs and evidence hashes are pseudonymous identifiers, not a claim of anonymization.
/// The caller selects the explicit historical/current document; this hook cannot infer head revision.
///
/// # Errors
/// Rejects unaccepted, stale-content, forged, foreign-reference or oversized data atomically.
pub fn build_workforce_share_result(
    document: &ScenarioDocument,
    accepted: &AcceptedResult,
    options: ShareResultOptions,
) -> Result<DomainShareResult, DomainPackError> {
    let mut budget = OperationBudget::evaluation(None);
    revalidate_workforce_accepted(document, accepted, &mut budget)?;
    let mut identities = PlanningIdentities::default();
    let mut assignments = Vec::new();
    let mut evidence_references = options.include_evidence_references.then(Vec::new);
    for assignment in &accepted.solution.assignments {
        budget.step().map_err(|error| operation_error(&error))?;
        let (pair, selected) = decode_workforce_assignment(assignment)?;
        let evidence = validate_source_provenance(assignment, pair, &mut identities, &mut budget)?;
        if !selected {
            continue;
        }
        budget
            .reserve(1, 3, 64)
            .map_err(|error| operation_error(&error))?;
        assignments.push(ShareAssignment {
            assignment: &assignment.id,
            person: pair.person_id,
            shift: pair.shift_id,
        });
        if let Some(references) = &mut evidence_references {
            budget
                .reserve(0, 1, 24)
                .map_err(|error| operation_error(&error))?;
            references.push(evidence);
        }
    }
    if let Some(references) = &mut evidence_references {
        budget
            .sort_work(references.len())
            .map_err(|error| operation_error(&error))?;
        references.sort_unstable();
        references.dedup();
    }
    let payload = SharePayload {
        schema_version: 1,
        scenario_id: document.scenario_id,
        scenario_revision: accepted.solution.scenario_revision,
        solution_id: accepted.solution.solution_id,
        accepted_result_checksum: &accepted.checksum,
        status: "verifiedFeasible",
        provenance: ShareProvenance {
            source_document_hash: &accepted.verification.document_hash,
            verification_report_checksum: &accepted.verification.checksum,
        },
        privacy: options,
        assignments,
        score: &accepted.verification.score,
        evidence_references,
    };
    let measured = preflight(&payload, &mut budget, ContractJsonLimits::DEFAULT)
        .map_err(|error| operation_error(&error))?;
    // The schema safety pass retains a second scrubbed JSON tree.
    measured
        .reserve_json(&mut budget, 2)
        .map_err(|error| operation_error(&error))?;
    let payload = serde_json::to_value(payload).map_err(|_| contract("share_payload"))?;
    // The generated schema is trusted, but parsing its immutable tree still consumes scratch.
    let schema_bytes = count(SHARE_SCHEMA.len()).map_err(|error| operation_error(&error))?;
    budget
        .steps(schema_bytes)
        .map_err(|error| operation_error(&error))?;
    budget
        .reserve(
            0,
            0,
            schema_bytes
                .checked_mul(128)
                .ok_or_else(|| contract("arithmetic"))?,
        )
        .map_err(|error| operation_error(&error))?;
    let schema = serde_json::from_str(SHARE_SCHEMA).map_err(|_| contract("share_schema"))?;
    ValidatedContractSchema::new(schema)?.validate(&payload, ContractJsonLimits::DEFAULT)?;
    budget.check().map_err(|error| operation_error(&error))?;
    Ok(DomainShareResult {
        pack_id: document.domain_pack.id.clone(),
        schema_version: 1,
        payload,
    })
}

pub(super) fn validate_source_provenance(
    assignment: &DomainAssignment,
    pair: AssignmentPair,
    identities: &mut PlanningIdentities,
    budget: &mut OperationBudget<'_>,
) -> Result<DomainEvidenceId, DomainPackError> {
    let expected = DomainEvidenceId::new(
        identities
            .derive(
                IdentityKind::Provenance,
                &("assignment", pair.person_id, pair.shift_id),
                budget,
            )
            .map_err(|error| operation_error(&error))?,
    )
    .map_err(|_| contract("evidence_identity"))?;
    if assignment.evidence.len() != 1 || assignment.evidence[0] != expected {
        return Err(contract("source_provenance"));
    }
    Ok(expected)
}
