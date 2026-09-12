use super::{
    contracts::{
        CommandChangeV1, PREVIEW_DATA_BYTES, PageParametersV1, SettingsPreparationParametersV1,
        WorkforcePositionV1, WorkforceSetupViewDataV1,
    },
    paging::{PageBuilder, ProjectionBudget, Result, invalid},
};
use eutheto_domain_api::{DomainPackError, SetupViewContext};
use eutheto_types::{Change, Horizon, ScenarioId, ScenarioSettings, resolve_local_midnight};

pub(super) fn settings_preparation(
    parameters: SettingsPreparationParametersV1,
    position: Option<WorkforcePositionV1>,
    budget: &mut ProjectionBudget<'_>,
) -> Result<WorkforceSetupViewDataV1> {
    if position.is_some() {
        return Err(invalid(
            "/query/continuation",
            "settings preparation does not accept continuation",
        ));
    }
    if parameters.dates.start_date >= parameters.dates.end_date_exclusive {
        return Err(invalid(
            "/query/parameters/dates",
            "planning dates must be nonempty and increasing",
        ));
    }
    // These dates define the entire desired horizon, not a 366-day display window.
    let mut endpoint = |date: jiff::civil::Date, path| {
        budget.visit()?;
        resolve_local_midnight(
            date,
            &parameters.time_zone,
            parameters.gap_policy,
            parameters.overlap_policy,
        )
        .map_err(|error| {
            invalid(
                path,
                &format!(
                    "planning boundary requires local-time resolution: {:?}",
                    error.kind
                ),
            )
        })
    };
    let start = endpoint(
        parameters.dates.start_date,
        "/query/parameters/dates/startDate",
    )?;
    let end = endpoint(
        parameters.dates.end_date_exclusive,
        "/query/parameters/dates/endDateExclusive",
    )?;
    let horizon = Horizon::new(start, end).map_err(|_| {
        invalid(
            "/query/parameters/dates",
            "resolved planning horizon must be nonempty and increasing",
        )
    })?;
    let settings = ScenarioSettings {
        time_zone: parameters.time_zone,
        locale: parameters.locale,
        units: parameters.units,
        horizon,
        gap_policy: parameters.gap_policy,
        overlap_policy: parameters.overlap_policy,
    };
    budget.visit()?;
    crate::model::planning_dates(&settings)?;
    Ok(WorkforceSetupViewDataV1::SettingsPreparation(settings))
}

pub(super) fn command_changes(
    changes: &[Change],
    parameters: &PageParametersV1,
    position: Option<WorkforcePositionV1>,
    scenario_id: ScenarioId,
    context: SetupViewContext,
    budget: &mut ProjectionBudget<'_>,
) -> Result<WorkforceSetupViewDataV1> {
    let total = u32::try_from(changes.len()).map_err(|_| DomainPackError::ResourceLimitExceeded)?;
    let next = match position {
        None => 0,
        Some(WorkforcePositionV1::Ordinal { next_ordinal }) if next_ordinal <= total => {
            next_ordinal
        }
        Some(_) => {
            return Err(invalid(
                "/query/continuation/position",
                "command changes require an in-range next ordinal",
            ));
        }
    };
    let mut page = PageBuilder::new(parameters.limit, PREVIEW_DATA_BYTES)?;
    for (index, change) in changes.iter().enumerate() {
        budget.visit()?;
        let ordinal = u32::try_from(index).map_err(|_| DomainPackError::ResourceLimitExceeded)?;
        page.observe(ordinal >= next, || {
            Ok((
                CommandChangeV1 {
                    ordinal,
                    change: change.clone(),
                },
                WorkforcePositionV1::Ordinal {
                    next_ordinal: ordinal
                        .checked_add(1)
                        .ok_or(DomainPackError::ResourceLimitExceeded)?,
                },
            ))
        })?;
    }
    page.finish(
        scenario_id,
        context,
        WorkforceSetupViewDataV1::CommandChanges,
    )
}
