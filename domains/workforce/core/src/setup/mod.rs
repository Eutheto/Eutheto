//! Read-only Workforce setup contracts and projections.

mod commands;
pub mod contracts;
mod entities;
mod inspection;
mod overview;
mod paging;
mod people;
mod rules;
mod work;

use contracts::{
    DETAIL_DATA_BYTES, ORDINARY_DATA_BYTES, PREVIEW_DATA_BYTES, QUERY_BYTES, SETUP_OUTPUT_LIMITS,
    WorkforcePositionV1, WorkforceSetupQueryV1, WorkforceSetupResultV1, WorkforceSetupViewDataV1,
};
use eutheto_domain_api::{
    ContractJsonLimits, DomainBatchCommand, DomainCatalog, DomainPack, DomainPackError,
    DomainSetupQueryV1, DomainView, DomainViewInput, DomainViewOutput, SetupViewContext,
    bounded_json_size, validate_contract_value,
};
use eutheto_types::{OperationControl, ScenarioDocument};
use paging::{ProjectionBudget, Result, continuation_position, invalid};

pub(crate) fn build_view(
    input: DomainViewInput<'_>,
    control: &OperationControl,
) -> Result<DomainViewOutput> {
    control.check()?;
    let (document, query, context) = setup_subject(input)?;
    if document.domain_pack.id.as_str()
        != crate::generated_workforce_pack_contract::WORKFORCE_PACK_ID
    {
        return Err(invalid(
            "/domainPack",
            "setup subject belongs to another pack",
        ));
    }
    if document.domain_pack.schema_version != 1 {
        return Err(DomainPackError::UnsupportedVersion(
            document.domain_pack.schema_version,
        ));
    }
    if query.schema_version != 1 {
        return Err(DomainPackError::UnsupportedVersion(query.schema_version));
    }
    let catalog = crate::WorkforcePack.catalog()?;
    let descriptor = catalog
        .setup_queries
        .iter()
        .find(|descriptor| descriptor.id == query.view_id)
        .ok_or_else(|| invalid("/query/viewId", "unknown Workforce setup view"))?;
    descriptor.validate_subject(
        if matches!(input, DomainViewInput::StoredSetup { .. }) {
            eutheto_domain_api::SetupQuerySource::Stored
        } else {
            eutheto_domain_api::SetupQuerySource::CommandPreview
        },
        query.continuation.is_some(),
    )?;
    descriptor.validate_parameters(
        &query.parameters,
        ContractJsonLimits {
            max_serialized_bytes: QUERY_BYTES,
            ..ContractJsonLimits::DEFAULT
        },
    )?;
    let decoded = WorkforceSetupQueryV1::decode(query).map_err(|_| {
        invalid(
            "/query/parameters",
            "invalid typed Workforce setup parameters",
        )
    })?;
    let position = continuation_position(query, document.scenario_id, context)?;
    bounded_json_size(query, QUERY_BYTES)?;
    let mut budget = ProjectionBudget::new(control);
    let byte_limit = match &decoded {
        WorkforceSetupQueryV1::EntityDetail(_)
        | WorkforceSetupQueryV1::RuleDetail(_)
        | WorkforceSetupQueryV1::WorkDetail(_) => DETAIL_DATA_BYTES,
        WorkforceSetupQueryV1::GenerationReview(_) | WorkforceSetupQueryV1::CommandChanges(_) => {
            PREVIEW_DATA_BYTES
        }
        _ => ORDINARY_DATA_BYTES,
    };
    let (result, reconciliation) = project(
        input,
        document,
        context,
        &catalog,
        decoded,
        position,
        &mut budget,
    )?;
    control.check()?;
    let result = WorkforceSetupResultV1 {
        schema_version: 1,
        result,
    };
    bounded_json_size(&result, byte_limit).map_err(|_| DomainPackError::ResourceLimitExceeded)?;
    let data = serde_json::to_value(result)
        .map_err(|_| invalid("/view", "setup output cannot be serialized"))?;
    validate_contract_value(
        &descriptor.result_schema,
        &data,
        ContractJsonLimits {
            max_serialized_bytes: byte_limit,
            ..SETUP_OUTPUT_LIMITS
        },
    )?;
    control.check()?;
    Ok(DomainViewOutput {
        view: DomainView {
            view_id: query.view_id.clone(),
            data,
        },
        reconciliation,
    })
}

fn setup_subject(
    input: DomainViewInput<'_>,
) -> Result<(&ScenarioDocument, &DomainSetupQueryV1, SetupViewContext)> {
    match input {
        DomainViewInput::StoredSetup {
            document,
            query,
            context,
        } => Ok((document, query, context)),
        DomainViewInput::CommandPreviewSetup {
            original,
            prospective,
            query,
            context,
            ..
        } => {
            if original.scenario_id != prospective.scenario_id
                || original.domain_pack != prospective.domain_pack
            {
                return Err(invalid(
                    "/source",
                    "preview does not preserve its scenario authority",
                ));
            }
            Ok((prospective, query, context))
        }
        DomainViewInput::AcceptedSolution { .. } => Err(invalid(
            "/source",
            "setup projections require a setup subject",
        )),
    }
}

fn project(
    input: DomainViewInput<'_>,
    document: &ScenarioDocument,
    context: SetupViewContext,
    catalog: &DomainCatalog,
    query: WorkforceSetupQueryV1,
    position: Option<WorkforcePositionV1>,
    budget: &mut ProjectionBudget<'_>,
) -> Result<(WorkforceSetupViewDataV1, Option<DomainBatchCommand>)> {
    let result = match query {
        WorkforceSetupQueryV1::Overview(_) => overview::facts(document, position, budget),
        WorkforceSetupQueryV1::SettingsPreparation(parameters) => {
            commands::settings_preparation(parameters, position, budget)
        }
        WorkforceSetupQueryV1::RuleCatalog(_) => rules::catalog(catalog, position, budget),
        WorkforceSetupQueryV1::RulePage(parameters) => {
            rules::page(catalog, document, &parameters, position, context, budget)
        }
        WorkforceSetupQueryV1::RuleDetail(parameters) => {
            rules::detail(document, &parameters, position, budget)
        }
        WorkforceSetupQueryV1::WorkWindow(parameters) => {
            work::window(document, &parameters, position, context, budget)
        }
        WorkforceSetupQueryV1::WorkDetail(parameters) => {
            work::detail(document, &parameters, position, budget)
        }
        WorkforceSetupQueryV1::GenerationReview(parameters) => {
            let original = if let DomainViewInput::CommandPreviewSetup { original, .. } = input {
                original
            } else {
                document
            };
            return work::generation_review(
                original,
                document,
                &parameters,
                position,
                context,
                budget,
            );
        }
        WorkforceSetupQueryV1::EntityPage(parameters) => {
            entities::page(document, &parameters, position, context, budget)
        }
        WorkforceSetupQueryV1::EntityDetail(parameters) => {
            entities::detail(document, &parameters, position, budget)
        }
        WorkforceSetupQueryV1::CommandChanges(parameters) => {
            let DomainViewInput::CommandPreviewSetup { changes, .. } = input else {
                return Err(invalid(
                    "/source",
                    "command changes require a command preview source",
                ));
            };
            commands::command_changes(
                changes,
                &parameters,
                position,
                document.scenario_id,
                context,
                budget,
            )
        }
        WorkforceSetupQueryV1::EligibilityMatrix(parameters) => {
            people::eligibility_matrix(document, &parameters, position, budget)
        }
        WorkforceSetupQueryV1::AvailabilityWindow(parameters) => {
            people::availability_window(document, &parameters, position, context, budget)
        }
        WorkforceSetupQueryV1::RuleScope(parameters) => {
            inspection::rule_scope(document, &parameters, position, context, budget)
        }
        WorkforceSetupQueryV1::AssignmentInspection(parameters) => {
            inspection::assignment_inspection(document, &parameters, position, context, budget)
        }
    }?;
    Ok((result, None))
}
