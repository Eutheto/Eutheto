use super::contracts::{
    ORDINARY_DATA_BYTES, RuleCatalogEntryV1, RuleCatalogV1, RuleClassV1, RuleDetailParametersV1,
    RulePageParametersV1, RuleRecordV1, RuleReferenceV1, RuleSummaryV1, RuleSupportV1,
    WorkforcePositionV1, WorkforceSetupViewDataV1,
};
use super::paging::{PageBuilder, ProjectionBudget, Result, invalid};
use crate::model::{PreferencePriority, WorkforcePreference, WorkforceRule};
use crate::validation::{MAX_WEIGHT, WorkforceSchemas};
use eutheto_domain_api::{ContractJsonLimits, DomainCatalog, KindDescriptor, SetupViewContext};
use eutheto_types::{RuleId, ScenarioDocument};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;

// Wire-kind/catalog identity mapping. Support follows analysis::supported_rule and
// compiler_model::require_supported, not installed backends or stored activation.
const REQUIRED: &[(&str, &str, RuleSupportV1)] = &[
    (
        "eligibility",
        "official.workforce.rule.eligibility",
        RuleSupportV1::Implemented,
    ),
    (
        "availability",
        "official.workforce.rule.availability",
        RuleSupportV1::Implemented,
    ),
    (
        "coverage",
        "official.workforce.rule.coverage",
        RuleSupportV1::Implemented,
    ),
    (
        "noOverlap",
        "official.workforce.rule.no-overlap",
        RuleSupportV1::Implemented,
    ),
    (
        "minimumRest",
        "official.workforce.rule.minimum-rest",
        RuleSupportV1::Implemented,
    ),
    (
        "maximumHours",
        "official.workforce.rule.maximum-hours",
        RuleSupportV1::NotImplemented,
    ),
    (
        "maximumConsecutive",
        "official.workforce.rule.maximum-consecutive",
        RuleSupportV1::NotImplemented,
    ),
    (
        "maximumAssignmentCount",
        "official.workforce.rule.maximum-assignment-count",
        RuleSupportV1::NotImplemented,
    ),
    (
        "requiredSkillMix",
        "official.workforce.rule.required-skill-mix",
        RuleSupportV1::NotImplemented,
    ),
    (
        "fixedAssignment",
        "official.workforce.rule.fixed-assignment",
        RuleSupportV1::NotImplemented,
    ),
    (
        "mutualAssignmentRestriction",
        "official.workforce.rule.mutual-assignment-restriction",
        RuleSupportV1::NotImplemented,
    ),
    (
        "transitionTime",
        "official.workforce.rule.transition-time",
        RuleSupportV1::NotImplemented,
    ),
];
const PREFERENCES: &[(&str, &str, RuleSupportV1)] = &[
    (
        "time",
        "official.workforce.preference.time",
        RuleSupportV1::NotImplemented,
    ),
    (
        "assignmentType",
        "official.workforce.preference.assignment-type",
        RuleSupportV1::NotImplemented,
    ),
    (
        "location",
        "official.workforce.preference.location",
        RuleSupportV1::NotImplemented,
    ),
    (
        "requestedTimeOff",
        "official.workforce.preference.requested-time-off",
        RuleSupportV1::NotImplemented,
    ),
    (
        "assignmentTarget",
        "official.workforce.preference.assignment-target",
        RuleSupportV1::NotImplemented,
    ),
    (
        "workloadBalance",
        "official.workforce.preference.workload-balance",
        RuleSupportV1::NotImplemented,
    ),
    (
        "consecutiveWork",
        "official.workforce.preference.consecutive-work",
        RuleSupportV1::NotImplemented,
    ),
    (
        "adjacency",
        "official.workforce.preference.adjacency",
        RuleSupportV1::NotImplemented,
    ),
    (
        "baseStability",
        "official.workforce.preference.base-stability",
        RuleSupportV1::NotImplemented,
    ),
    (
        "togetherSeparate",
        "official.workforce.preference.together-separate",
        RuleSupportV1::NotImplemented,
    ),
    (
        "preferredCoverage",
        "official.workforce.preference.preferred-coverage",
        RuleSupportV1::NotImplemented,
    ),
];

fn kinds(class: RuleClassV1) -> &'static [(&'static str, &'static str, RuleSupportV1)] {
    match class {
        RuleClassV1::Required => REQUIRED,
        RuleClassV1::Preference => PREFERENCES,
    }
}

fn records(document: &ScenarioDocument, class: RuleClassV1) -> &BTreeMap<RuleId, Value> {
    match class {
        RuleClassV1::Required => &document.domain.rules,
        RuleClassV1::Preference => &document.domain.preferences,
    }
}

pub(super) fn catalog(
    catalog: &DomainCatalog,
    position: Option<WorkforcePositionV1>,
    budget: &mut ProjectionBudget<'_>,
) -> Result<WorkforceSetupViewDataV1> {
    if position.is_some() {
        return Err(invalid(
            "/query/continuation",
            "rule catalog does not accept continuation",
        ));
    }
    budget.visit()?;
    let required = catalog_entries(&catalog.ui.rule_kinds, RuleClassV1::Required, budget)?;
    let preferences = catalog_entries(&catalog.ui.goal_kinds, RuleClassV1::Preference, budget)?;
    Ok(WorkforceSetupViewDataV1::RuleCatalog(RuleCatalogV1 {
        required,
        preferences,
    }))
}

fn catalog_entries(
    descriptors: &[KindDescriptor],
    class: RuleClassV1,
    budget: &mut ProjectionBudget<'_>,
) -> Result<Vec<RuleCatalogEntryV1>> {
    let mut entries = Vec::new();
    for descriptor in descriptors {
        budget.visit()?;
        let (_, _, support) = kinds(class)
            .iter()
            .find(|(_, id, _)| *id == descriptor.id)
            .ok_or_else(|| {
                invalid(
                    "/catalog/ui",
                    "rule catalog kind has no implementation support declaration",
                )
            })?;
        entries.push(RuleCatalogEntryV1 {
            descriptor: descriptor.clone(),
            support: *support,
        });
    }
    Ok(entries)
}

pub(super) fn page(
    catalog: &DomainCatalog,
    document: &ScenarioDocument,
    parameters: &RulePageParametersV1,
    position: Option<WorkforcePositionV1>,
    context: SetupViewContext,
    budget: &mut ProjectionBudget<'_>,
) -> Result<WorkforceSetupViewDataV1> {
    let previous = match position {
        None => None,
        Some(WorkforcePositionV1::Rule { rule_id }) => Some(rule_id),
        Some(_) => {
            return Err(invalid(
                "/query/continuation/position",
                "rule page requires a rule position",
            ));
        }
    };
    let mut page = PageBuilder::new(parameters.limit, ORDINARY_DATA_BYTES)?;
    if let Some(kind_id) = &parameters.kind_id {
        validate_kind_filter(catalog, parameters.class, kind_id, budget)?;
    }
    let records = records(document, parameters.class);
    if let Some(id) = previous {
        budget.visit()?;
        let record = records.get(&id).ok_or_else(|| {
            invalid(
                "/query/continuation/position",
                "continued rule is absent from the selected class",
            )
        })?;
        let header = header(id, parameters.class, record)?;
        if parameters
            .kind_id
            .as_deref()
            .is_some_and(|kind| kind != header.kind_id)
        {
            return Err(invalid(
                "/query/continuation/position",
                "continued rule does not match the kind filter",
            ));
        }
    }
    // ScenarioDomain uses BTreeMap<RuleId, Value>: no collection copy or sorting is needed.
    for (&rule_id, record) in records {
        budget.visit()?;
        let header = header(rule_id, parameters.class, record)?;
        if parameters
            .kind_id
            .as_deref()
            .is_some_and(|kind| kind != header.kind_id)
        {
            continue;
        }
        page.observe(previous.is_none_or(|id| rule_id > id), || {
            Ok((
                RuleSummaryV1 {
                    rule: RuleReferenceV1 {
                        class: parameters.class,
                        rule_id,
                    },
                    kind_id: header.kind_id.to_owned(),
                    active: header.active,
                    priority: header.priority,
                    weight: header.weight,
                },
                WorkforcePositionV1::Rule { rule_id },
            ))
        })?;
    }
    page.finish(
        document.scenario_id,
        context,
        WorkforceSetupViewDataV1::RulePage,
    )
}

fn validate_kind_filter(
    catalog: &DomainCatalog,
    class: RuleClassV1,
    kind_id: &str,
    budget: &mut ProjectionBudget<'_>,
) -> Result<()> {
    budget.visit()?;
    let descriptors = match class {
        RuleClassV1::Required => &catalog.ui.rule_kinds,
        RuleClassV1::Preference => &catalog.ui.goal_kinds,
    };
    for descriptor in descriptors {
        budget.visit()?;
        if descriptor.id == kind_id {
            return Ok(());
        }
    }
    Err(invalid(
        "/query/parameters/kindId",
        "kind does not exist in the selected rule catalog class",
    ))
}

pub(super) fn detail(
    document: &ScenarioDocument,
    parameters: &RuleDetailParametersV1,
    position: Option<WorkforcePositionV1>,
    budget: &mut ProjectionBudget<'_>,
) -> Result<WorkforceSetupViewDataV1> {
    if position.is_some() {
        return Err(invalid(
            "/query/continuation",
            "rule detail does not accept continuation",
        ));
    }
    budget.visit()?;
    let reference = parameters.rule;
    let record = records(document, reference.class)
        .get(&reference.rule_id)
        .ok_or_else(|| {
            invalid(
                "/query/parameters/rule",
                "rule is absent from the selected class",
            )
        })?;
    header(reference.rule_id, reference.class, record)?;
    let schemas = WorkforceSchemas::load()?;
    budget.visit()?;
    let result = match reference.class {
        RuleClassV1::Required => {
            schemas
                .rules
                .validate(record, ContractJsonLimits::DEFAULT)?;
            RuleRecordV1::Required(
                WorkforceRule::deserialize(record)
                    .map_err(|_| invalid("/domain/rules", "rule has an invalid typed record"))?,
            )
        }
        RuleClassV1::Preference => {
            schemas
                .preferences
                .validate(record, ContractJsonLimits::DEFAULT)?;
            RuleRecordV1::Preference(WorkforcePreference::deserialize(record).map_err(|_| {
                invalid(
                    "/domain/preferences",
                    "preference has an invalid typed record",
                )
            })?)
        }
    };
    budget.visit()?;
    Ok(WorkforceSetupViewDataV1::RuleDetail(Box::new(result)))
}

pub(super) struct RuleHeader {
    pub kind_id: &'static str,
    pub active: bool,
    pub priority: Option<PreferencePriority>,
    pub weight: Option<u32>,
}

/// Stored structural facts only; scopes and rule effects are not evaluated here.
pub(super) fn header(id: RuleId, class: RuleClassV1, record: &Value) -> Result<RuleHeader> {
    let path = match class {
        RuleClassV1::Required => "/domain/rules",
        RuleClassV1::Preference => "/domain/preferences",
    };
    let actual_id = RuleId::deserialize(
        record
            .get("id")
            .ok_or_else(|| invalid(path, "rule identity is absent"))?,
    )
    .map_err(|_| invalid(path, "rule identity is invalid"))?;
    if actual_id != id {
        return Err(invalid(path, "rule identity does not match its map key"));
    }
    let kind = record
        .get("kind")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid(path, "rule kind is invalid"))?;
    let (_, kind_id, _) = kinds(class)
        .iter()
        .find(|(wire, _, _)| *wire == kind)
        .ok_or_else(|| invalid(path, "rule kind does not belong to its class"))?;
    let active = record
        .get("active")
        .and_then(Value::as_bool)
        .ok_or_else(|| invalid(path, "rule activation is invalid"))?;
    let (priority, weight) = match class {
        RuleClassV1::Required => {
            if record.get("strength").and_then(Value::as_str) != Some("required") {
                return Err(invalid(path, "required rule strength is invalid"));
            }
            (None, None)
        }
        RuleClassV1::Preference => {
            let priority = PreferencePriority::deserialize(
                record
                    .get("priority")
                    .ok_or_else(|| invalid(path, "preference priority is absent"))?,
            )
            .map_err(|_| invalid(path, "preference priority is invalid"))?;
            let weight = record
                .get("weight")
                .and_then(Value::as_u64)
                .and_then(|weight| u32::try_from(weight).ok())
                .filter(|weight| (1..=MAX_WEIGHT).contains(weight))
                .ok_or_else(|| invalid(path, "preference weight is invalid"))?;
            (Some(priority), Some(weight))
        }
    };
    Ok(RuleHeader {
        kind_id,
        active,
        priority,
        weight,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{fixture, id};
    use eutheto_domain_api::DomainPack;
    use eutheto_types::{CancellationToken, OperationControl, Revision};
    use serde_json::json;

    #[test]
    fn filtered_rule_pages_keep_full_totals_and_reject_nonmatching_cursors()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let mut document = fixture()?;
        document.domain.rules.clear();
        for (number, kind, active) in [
            (23, "eligibility", true),
            (21, "eligibility", false),
            (22, "coverage", true),
            (20, "eligibility", true),
        ] {
            document.domain.rules.insert(
                id(number).parse()?,
                json!({
                    "kind": kind, "id": id(number), "active": active, "strength": "required",
                    "scope": { "people": { "kind": "all" } }
                }),
            );
        }
        let parameters = RulePageParametersV1 {
            class: RuleClassV1::Required,
            kind_id: Some("official.workforce.rule.eligibility".to_owned()),
            limit: 2,
        };
        let context = SetupViewContext {
            revision: Revision::INITIAL,
            query_fingerprint: [7; 32],
        };
        let control = OperationControl::Cancellation(CancellationToken::new());
        let catalog = crate::WorkforcePack.catalog()?;
        let first = page(
            &catalog,
            &document,
            &parameters,
            None,
            context,
            &mut ProjectionBudget::new(&control),
        )?;
        let WorkforceSetupViewDataV1::RulePage(first) = first else {
            return Err("expected rule page".into());
        };
        assert_eq!(first.total_items, 3);
        assert_eq!(
            first
                .items
                .iter()
                .map(|row| (row.rule.rule_id.to_string(), row.active))
                .collect::<Vec<_>>(),
            vec![(id(20), true), (id(21), false)],
        );
        assert!(
            first
                .items
                .iter()
                .all(|row| row.priority.is_none() && row.weight.is_none())
        );
        let continuation = first.continuation.ok_or("expected continuation")?;
        let position = serde_json::from_value(continuation.position)?;
        let second = page(
            &catalog,
            &document,
            &parameters,
            Some(position),
            context,
            &mut ProjectionBudget::new(&control),
        )?;
        let WorkforceSetupViewDataV1::RulePage(second) = second else {
            return Err("expected rule page".into());
        };
        assert_eq!(second.total_items, 3);
        assert_eq!(
            second
                .items
                .iter()
                .map(|row| row.rule.rule_id.to_string())
                .collect::<Vec<_>>(),
            vec![id(23)]
        );
        assert!(second.continuation.is_none());
        assert!(
            page(
                &catalog,
                &document,
                &parameters,
                Some(WorkforcePositionV1::Rule {
                    rule_id: id(22).parse()?
                }),
                context,
                &mut ProjectionBudget::new(&control),
            )
            .is_err()
        );
        Ok(())
    }
}
