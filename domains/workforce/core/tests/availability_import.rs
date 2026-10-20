mod support;

use eutheto_types::{
    CancellationToken, GapPolicy, OperationControl, OverlapPolicy, Revision, ScenarioDocument,
};
use eutheto_workforce::{
    assignment_rules::{
        compile_workforce, evaluate_assignment_rules, project_workforce_candidate,
        score_workforce_solution,
    },
    availability_import::*,
    commands,
    model::AssignmentPair,
    people_csv::{
        BlankPolicy, CsvDialect, CsvError, CsvErrorCode, MAX_CSV_REVIEW_BYTES,
        MAX_CSV_SOURCE_BYTES, PeopleImportDisposition,
    },
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    io::{Cursor, Read},
};
use support::{fixture, id};

type TestResult<T = ()> = Result<T, Box<dyn Error>>;

fn mapping(fields: &[AvailabilityCsvField]) -> AvailabilityCsvMapping {
    let columns = fields
        .iter()
        .zip(1..)
        .map(|(field, index)| AvailabilityCsvColumn {
            index,
            field: *field,
            blank: BlankPolicy::Preserve,
        })
        .collect::<Vec<_>>();
    AvailabilityCsvMapping {
        dialect: CsvDialect::Tab,
        has_header: true,
        expected_columns: columns.last().map_or(1, |column| column.index + 1),
        identity_column: Some(0),
        columns,
        person_mappings: BTreeMap::new(),
        assignment_type_mappings: BTreeMap::new(),
        location_mappings: BTreeMap::new(),
    }
}
fn complete_mapping() -> AvailabilityCsvMapping {
    mapping(&[
        AvailabilityCsvField::PersonId,
        AvailabilityCsvField::AvailabilityKind,
        AvailabilityCsvField::StartsAt,
        AvailabilityCsvField::EndsAt,
        AvailabilityCsvField::EffectiveStartDate,
        AvailabilityCsvField::EffectiveEndDateExclusive,
    ])
}
fn source(rows: &[Vec<String>]) -> String {
    let width = rows.first().map_or(1, Vec::len);
    let mut text = (0..width)
        .map(|index| format!("column{index}"))
        .collect::<Vec<_>>()
        .join("\t");
    text.push('\n');
    for row in rows {
        for (index, cell) in row.iter().enumerate() {
            if index > 0 {
                text.push('\t');
            }
            if cell.contains(['"', '\t', '\r', '\n']) {
                text.push('\"');
                text.push_str(&cell.replace('\"', "\"\""));
                text.push('\"');
            } else {
                text.push_str(cell);
            }
        }
        text.push('\n');
    }
    text
}
fn complete_row(target: String) -> Vec<String> {
    vec![
        target,
        id(1),
        "approvedTimeOff".to_owned(),
        "2026-11-01T08:00".to_owned(),
        "2026-11-01T10:00".to_owned(),
        "2026-11-01".to_owned(),
        "2026-12-01".to_owned(),
    ]
}
fn decision(record: u32, add: bool, target: u32) -> TestResult<AvailabilityCsvRowDecision> {
    Ok(AvailabilityCsvRowDecision {
        record,
        decision: if add {
            AvailabilityCsvDecision::Add {
                availability_id: id(target).parse()?,
            }
        } else {
            AvailabilityCsvDecision::Update {
                availability_id: id(target).parse()?,
            }
        },
    })
}
fn preview(
    document: &ScenarioDocument,
    source: &str,
    mapping: &AvailabilityCsvMapping,
    decisions: &[AvailabilityCsvRowDecision],
) -> Result<AvailabilityImportPreview, CsvError> {
    preview_availability_csv(
        document,
        Revision::INITIAL,
        &mut Cursor::new(source.as_bytes()),
        mapping,
        decisions,
        &BTreeSet::new(),
        &CancellationToken::default(),
    )
}
fn base() -> TestResult<ScenarioDocument> {
    let mut document = fixture()?;
    document.domain.entities.remove(&id(6).parse()?);
    document.domain.locked_assignments.clear();
    document.settings.time_zone = "UTC".parse()?;
    document.settings.horizon = serde_json::from_value(
        json!({"start":"2026-11-01T00:00:00Z","end":"2026-11-02T00:00:00Z"}),
    )?;
    let shift = document
        .domain
        .entities
        .get_mut(&id(8).parse()?)
        .ok_or("shift")?;
    shift["startsAt"] =
        json!({"instant":"2026-11-01T08:00:00Z","local":"2026-11-01T08:00:00","offsetSeconds":0});
    shift["endsAt"] =
        json!({"instant":"2026-11-01T10:00:00Z","local":"2026-11-01T10:00:00","offsetSeconds":0});
    Ok(document)
}
fn with_availability() -> TestResult<ScenarioDocument> {
    let mut document = base()?;
    document.domain.entities.insert(id(30).parse()?, json!({"kind":"availability","id":id(30),"personId":id(1),"availabilityKind":"unavailable",
        "timeWindow":{"kind":"instant","startsAt":"2026-11-01T08:00:00.000-00:00","endsAt":"2026-11-01T10:00:00Z"},
        "effectiveRange":{"startDate":"2026-11-01","endDateExclusive":"2026-12-01"},"source":"original source","note":"original note"}));
    Ok(document)
}
fn weekly() -> Value {
    json!([{"weekdays":["sunday","monday"],"startTime":"08:00:00","endTime":"10:00:00","endDayOffset":0}])
}
fn assert_blocked(preview: &AvailabilityImportPreview, issue: AvailabilityCsvRowIssue) {
    assert_eq!(preview.disposition, PeopleImportDisposition::Blocked);
    assert_eq!(preview.rows[0].issue, Some(issue));
    assert!(preview.batch.is_none());
    assert!(preview.review.is_none());
}

fn assert_requested_consumer(document: &ScenarioDocument) -> TestResult {
    let context = eutheto_domain_api::CompileContext {
        objective_context: None,
        scenario_revision: 1,
        semantic_metadata: BTreeMap::new(),
        control: OperationControl::Cancellation(CancellationToken::default()),
        planning_limits: eutheto_planning_ir::PlanningIrLimitsV1::DEFAULT,
    };
    let problem = compile_workforce(document, &context)?.problem;
    let mut candidate = eutheto_planning_ir::CandidateValues::default();
    for projection in &problem.projections {
        let eutheto_planning_ir::ProjectionExpression::Boolean(variable) = &projection.expression
        else {
            return Err("expected assignment projection".into());
        };
        candidate.booleans.insert(
            variable.clone(),
            projection.entity.id.as_str() == format!("{}.{}", id(1), id(8)),
        );
    }
    let solution = project_workforce_candidate(
        &problem,
        &candidate,
        id(700).parse()?,
        context.planning_limits,
        &context.control,
    )?;
    let requested_cost = |document: &ScenarioDocument| -> TestResult<i64> {
        Ok(score_workforce_solution(document, &solution, None, None)?
            .levels
            .iter()
            .filter_map(|level| {
                level
                    .category_breakdown
                    .iter()
                    .find_map(|(category, value)| {
                        (category.as_str()
                            == "official.workforce.score.preference.requested_time_off")
                            .then_some(*value)
                    })
            })
            .sum())
    };
    assert_eq!(requested_cost(document)?, 0);
    let mut referenced = document.clone();
    referenced.domain.preferences.insert(id(41).parse()?, json!({"kind":"requestedTimeOff","id":id(41),"active":true,"scope":{"people":{"kind":"all"}},"priority":"normal","weight":1,"availabilityIds":[id(30)]}));
    assert_eq!(requested_cost(&referenced)?, 10);
    Ok(())
}

#[test]
fn imported_kinds_keep_their_real_enforcement_and_requested_preference_semantics() -> TestResult {
    let pair = AssignmentPair {
        person_id: id(1).parse()?,
        shift_id: id(8).parse()?,
    };
    for kind in [
        "approvedTimeOff",
        "unavailable",
        "availableOnly",
        "requestedTimeOff",
    ] {
        let document = base()?;
        let mut row = complete_row(id(30));
        row[2] = kind.to_owned();
        // AvailableOnly deliberately permits a different interval than the selected shift.
        if kind == "availableOnly" {
            row[3] = "2026-11-01T11:00".to_owned();
            row[4] = "2026-11-01T12:00".to_owned();
        }
        let proposal = preview(
            &document,
            &source(&[row]),
            &complete_mapping(),
            &[decision(2, true, 30)?],
        )?;
        let applied = commands::apply_batch(&document, proposal.batch.as_ref().ok_or("batch")?)?;
        assert!(applied.document.domain.rules.is_empty());
        assert!(applied.document.domain.preferences.is_empty());
        let evaluation = evaluate_assignment_rules(&applied.document, &[pair], None)?;
        assert_eq!(
            evaluation
                .evaluations
                .iter()
                .any(|record| !record.satisfied),
            kind == "approvedTimeOff"
        );
        let mut ruled = applied.document.clone();
        let rule = json!({"id":id(40),"kind":"availability","active":true,"strength":"required","scope":{"people":{"kind":"all"}}});
        ruled.domain.rules.insert(id(40).parse()?, rule);
        let evaluation = evaluate_assignment_rules(&ruled, &[pair], None)?;
        assert_eq!(
            evaluation
                .evaluations
                .iter()
                .any(|record| !record.satisfied),
            kind != "requestedTimeOff"
        );
        ruled.domain.rules.get_mut(&id(40).parse()?).ok_or("rule")?["active"] = json!(false);
        assert_eq!(
            evaluate_assignment_rules(&ruled, &[pair], None)?
                .evaluations
                .iter()
                .any(|record| !record.satisfied),
            kind == "approvedTimeOff"
        );
        if kind == "requestedTimeOff" {
            assert_requested_consumer(&applied.document)?;
        }
        assert_eq!(
            commands::apply_batch(&applied.document, &applied.inverse)?.document,
            document
        );
    }
    Ok(())
}

#[test]
fn requested_kind_conversion_blocks_active_and_inactive_incoming_references_without_partial_changes()
-> TestResult {
    for active in [true, false] {
        let mut document = with_availability()?;
        document
            .domain
            .entities
            .get_mut(&id(30).parse()?)
            .ok_or("availability")?["availabilityKind"] = json!("requestedTimeOff");
        document.domain.preferences.insert(id(41).parse()?, json!({"kind":"requestedTimeOff","id":id(41),"active":active,"scope":{"people":{"kind":"all"}},"priority":"normal","weight":1,"availabilityIds":[id(30)]}));
        let snapshot = document.clone();
        let policy = mapping(&[AvailabilityCsvField::AvailabilityKind]);
        let proposal = preview(
            &document,
            &source(&[vec![id(30), "approvedTimeOff".to_owned()]]),
            &policy,
            &[],
        )?;
        assert_blocked(&proposal, AvailabilityCsvRowIssue::InvalidAvailability);
        assert_eq!(document, snapshot);
        document.domain.preferences.clear();
        let unreferenced = preview(
            &document,
            &source(&[vec![id(30), "approvedTimeOff".to_owned()]]),
            &policy,
            &[],
        )?;
        let mutation = commands::apply_batch(
            &document,
            unreferenced
                .batch
                .as_ref()
                .ok_or("unreferenced conversion")?,
        )?;
        assert_eq!(
            mutation.document.domain.entities[&id(30).parse()?]["availabilityKind"],
            json!("approvedTimeOff")
        );
        assert_eq!(
            commands::apply_batch(&mutation.document, &mutation.inverse)?.document,
            document
        );
    }
    Ok(())
}

#[test]
fn instant_weekly_transition_table_and_mixed_files_preserve_only_the_selected_variant() -> TestResult
{
    let policy = mapping(&[
        AvailabilityCsvField::WeeklyWindows,
        AvailabilityCsvField::StartsAt,
        AvailabilityCsvField::EndsAt,
    ]);
    for existing_weekly in [false, true] {
        let mut document = with_availability()?;
        if existing_weekly {
            document
                .domain
                .entities
                .get_mut(&id(30).parse()?)
                .ok_or("availability")?["timeWindow"] = json!({"kind":"weekly","windows":weekly()});
        }
        let original = document.domain.entities[&id(30).parse()?].clone();
        for (windows, start, end, issue, variant) in [
            (String::new(), "", "", None, None),
            (
                String::new(),
                "2026-11-01T09:00",
                "",
                if existing_weekly {
                    Some(AvailabilityCsvRowIssue::MissingRequiredField)
                } else {
                    None
                },
                Some("instant"),
            ),
            (
                String::new(),
                "",
                "2026-11-01T11:00",
                if existing_weekly {
                    Some(AvailabilityCsvRowIssue::MissingRequiredField)
                } else {
                    None
                },
                Some("instant"),
            ),
            (
                String::new(),
                "2026-11-01T09:00",
                "2026-11-01T11:00",
                None,
                Some("instant"),
            ),
            (weekly().to_string(), "", "", None, Some("weekly")),
            (
                weekly().to_string(),
                "2026-11-01T09:00",
                "",
                Some(AvailabilityCsvRowIssue::ConflictingTimeWindow),
                None,
            ),
            (
                weekly().to_string(),
                "",
                "2026-11-01T11:00",
                Some(AvailabilityCsvRowIssue::ConflictingTimeWindow),
                None,
            ),
        ] {
            let proposal = preview(
                &document,
                &source(&[vec![id(30), windows, start.to_owned(), end.to_owned()]]),
                &policy,
                &[],
            )?;
            if let Some(issue) = issue {
                assert_blocked(&proposal, issue);
                continue;
            }
            let raw = proposal.rows[0]
                .after
                .as_ref()
                .or(proposal.rows[0].before.as_ref())
                .ok_or("projection")?;
            if let Some(variant) = variant {
                assert_eq!(raw["timeWindow"]["kind"], json!(variant));
            } else {
                assert_eq!(raw, &original);
            }
            if raw["timeWindow"]["kind"] == "weekly" {
                assert!(raw["timeWindow"].get("startsAt").is_none());
            } else {
                assert!(raw["timeWindow"].get("windows").is_none());
            }
            assert_eq!(raw["source"], original["source"]);
        }
    }
    Ok(())
}

#[test]
fn mixed_instant_and_weekly_file_builds_one_native_batch() -> TestResult {
    let mut policy = complete_mapping();
    policy.columns.push(AvailabilityCsvColumn {
        index: 7,
        field: AvailabilityCsvField::WeeklyWindows,
        blank: BlankPolicy::Preserve,
    });
    policy.expected_columns = 8;
    let mut instant = complete_row(id(31));
    instant.push(String::new());
    let mut recurrent = complete_row(id(32));
    recurrent[3].clear();
    recurrent[4].clear();
    recurrent.push(weekly().to_string());
    let result = preview(
        &base()?,
        &source(&[instant, recurrent]),
        &policy,
        &[decision(2, true, 31)?, decision(3, true, 32)?],
    )?;
    assert_eq!(
        result.rows[0].after.as_ref().ok_or("instant")?["timeWindow"]["kind"],
        json!("instant")
    );
    assert_eq!(
        result.rows[1].after.as_ref().ok_or("weekly")?["timeWindow"]["kind"],
        json!("weekly")
    );
    Ok(())
}

fn precision_document() -> TestResult<ScenarioDocument> {
    let mut document = with_availability()?;
    let raw = document
        .domain
        .entities
        .get_mut(&id(30).parse()?)
        .ok_or("availability")?;
    raw["personId"] = json!(id(1).to_ascii_uppercase());
    raw["timeWindow"]["startsAt"] = json!("2026-11-01T03:00:00.123456789-05:00");
    raw["timeWindow"]["endsAt"] = json!("2026-11-01T05:00:00.987654321-05:00");
    Ok(document)
}

#[test]
fn equivalent_precision_instants_retain_raw_spelling_and_native_endpoint_facts() -> TestResult {
    let document = precision_document()?;
    let original = document.domain.entities[&id(30).parse()?].clone();
    let policy = mapping(&[
        AvailabilityCsvField::StartsAt,
        AvailabilityCsvField::EndsAt,
        AvailabilityCsvField::PersonId,
        AvailabilityCsvField::EffectiveStartDate,
        AvailabilityCsvField::Note,
    ]);
    let no_op_text = source(&[vec![
        id(30),
        "2026-11-01T08:00:00.123456789".to_owned(),
        "2026-11-01T10:00:00.987654321".to_owned(),
        id(1),
        String::new(),
        String::new(),
    ]]);
    let no_op = preview(&document, &no_op_text, &policy, &[])?;
    assert_eq!(no_op.disposition, PeopleImportDisposition::NoChanges);
    assert_eq!(no_op.rows[0].before.as_ref(), Some(&original));
    assert!(no_op.rows[0].after.is_none());
    assert!(no_op.rows[0].resolved_starts_at.is_some());
    assert!(no_op.rows[0].resolved_ends_at.is_some());
    Ok(())
}

#[test]
fn partial_leaf_edits_preserve_equivalent_siblings_references_and_exact_inverse() -> TestResult {
    let document = precision_document()?;
    let original = document.domain.entities[&id(30).parse()?].clone();
    let policy = mapping(&[
        AvailabilityCsvField::StartsAt,
        AvailabilityCsvField::EndsAt,
        AvailabilityCsvField::PersonId,
        AvailabilityCsvField::EffectiveStartDate,
        AvailabilityCsvField::Note,
    ]);
    let text = source(&[vec![
        id(30),
        "2026-11-01T08:00:00.123456789".to_owned(),
        "2026-11-01T11:00".to_owned(),
        id(1),
        "2026-10-31".to_owned(),
        "  authored whitespace  ".to_owned(),
    ]]);
    let proposal = preview(&document, &text, &policy, &[])?;
    let after = proposal.rows[0].after.as_ref().ok_or("after")?;
    assert_eq!(
        after["timeWindow"]["startsAt"],
        original["timeWindow"]["startsAt"]
    );
    assert_eq!(after["personId"], original["personId"]);
    assert_eq!(
        after["effectiveRange"]["endDateExclusive"],
        original["effectiveRange"]["endDateExclusive"]
    );
    assert_eq!(after["source"], original["source"]);
    assert_eq!(after["note"], json!("  authored whitespace  "));
    let mutation = commands::apply_batch(&document, proposal.batch.as_ref().ok_or("batch")?)?;
    let undo = commands::apply_batch(&mutation.document, &mutation.inverse)?;
    assert_eq!(undo.document, document);
    assert_eq!(
        commands::apply_batch(&undo.document, &undo.inverse)?.document,
        mutation.document
    );
    let only_start = preview(
        &document,
        &source(&[vec![id(30), "2026-11-01T09:00".to_owned()]]),
        &mapping(&[AvailabilityCsvField::StartsAt]),
        &[],
    )?;
    assert_eq!(
        only_start.rows[0].after.as_ref().ok_or("after")?["timeWindow"]["endsAt"],
        original["timeWindow"]["endsAt"]
    );
    Ok(())
}

fn filtered_weekly_document() -> TestResult<ScenarioDocument> {
    let mut document = with_availability()?;
    document.domain.entities.insert(
        id(50).parse()?,
        json!({"kind":"location","id":id(50),"name":"South","transitions":[]}),
    );
    let raw = document
        .domain
        .entities
        .get_mut(&id(30).parse()?)
        .ok_or("availability")?;
    raw["assignmentTypeIds"] = json!([id(4).replace('-', "")]);
    raw["locationIds"] = json!([id(50).to_ascii_uppercase(), id(5)]);
    let mut windows = weekly();
    windows.as_array_mut().ok_or("weekly")?.push(json!({"weekdays":["tuesday"],"startTime":"11:00:00","endTime":"12:00:00","endDayOffset":0}));
    raw["timeWindow"] = json!({"kind":"weekly","windows":windows});
    Ok(document)
}

fn role_alias_policy() -> TestResult<AvailabilityCsvMapping> {
    let mut policy = mapping(&[
        AvailabilityCsvField::PersonId,
        AvailabilityCsvField::AssignmentTypeIds,
        AvailabilityCsvField::LocationIds,
        AvailabilityCsvField::WeeklyWindows,
    ]);
    policy
        .person_mappings
        .insert("same".to_owned(), id(1).parse()?);
    policy
        .assignment_type_mappings
        .insert("same".to_owned(), id(4).parse()?);
    policy
        .location_mappings
        .insert("same".to_owned(), id(5).parse()?);
    Ok(policy)
}

#[test]
fn role_scoped_aliases_filter_and_weekday_set_equivalence_preserve_raw_spelling() -> TestResult {
    let document = filtered_weekly_document()?;
    let original = document.domain.entities[&id(30).parse()?].clone();
    let policy = role_alias_policy()?;
    let windows = original["timeWindow"]["windows"].clone();
    let mut reversed_days = windows.clone();
    reversed_days[0]["weekdays"] = json!(["monday", "sunday"]);
    let rows = vec![
        id(30),
        "same".to_owned(),
        json!(["same"]).to_string(),
        json!(["same", id(50)]).to_string(),
        reversed_days.to_string(),
    ];
    let no_op = preview(
        &document,
        &source(std::slice::from_ref(&rows)),
        &policy,
        &[],
    )?;
    assert_eq!(no_op.disposition, PeopleImportDisposition::NoChanges);
    assert_eq!(no_op.rows[0].before.as_ref(), Some(&original));
    let mut collision = rows.clone();
    collision[3] = json!(["same", id(5).to_ascii_uppercase()]).to_string();
    assert_blocked(
        &preview(&document, &source(&[collision]), &policy, &[])?,
        AvailabilityCsvRowIssue::InvalidReference,
    );
    Ok(())
}

#[test]
fn weekly_window_order_and_multiplicity_remain_authored_while_other_fields_stay_raw() -> TestResult
{
    let document = filtered_weekly_document()?;
    let original = document.domain.entities[&id(30).parse()?].clone();
    let mut windows = original["timeWindow"]["windows"].clone();
    windows.as_array_mut().ok_or("weekly")?.reverse();
    let rows = vec![
        id(30),
        "same".to_owned(),
        json!(["same"]).to_string(),
        json!(["same", id(50)]).to_string(),
        windows.to_string(),
    ];
    let changed = preview(&document, &source(&[rows]), &role_alias_policy()?, &[])?;
    assert_eq!(
        changed.rows[0].after.as_ref().ok_or("after")?["timeWindow"]["windows"],
        windows
    );
    assert_eq!(
        changed.rows[0].after.as_ref().ok_or("after")?["locationIds"],
        original["locationIds"]
    );
    let duplicate = windows[0].clone();
    windows.as_array_mut().ok_or("weekly")?.push(duplicate);
    let duplicated = preview(
        &document,
        &source(&[vec![id(30), windows.to_string()]]),
        &mapping(&[AvailabilityCsvField::WeeklyWindows]),
        &[],
    )?;
    assert_eq!(
        duplicated.rows[0].after.as_ref().ok_or("after")?["timeWindow"]["windows"],
        windows
    );
    Ok(())
}

#[test]
fn raw_json_rejection_precedes_typed_equivalence_for_weekly_windows_and_filters() -> TestResult {
    let document = with_availability()?;
    let policy = mapping(&[AvailabilityCsvField::WeeklyWindows]);
    for raw in [
        "null".to_owned(), "{}".to_owned(), "[]".to_owned(),
        "[null]".to_owned(), "[{\"weekdays\":[\"sunday\"],\"startTime\":\"08:00:00\",\"endTime\":\"10:00:00\",\"endDayOffset\":0,\"unknown\":true}]".to_owned(),
        "[{\"weekdays\":[\"sunday\",\"sunday\"],\"startTime\":\"08:00:00\",\"endTime\":\"10:00:00\",\"endDayOffset\":0}]".to_owned(),
        "[{\"weekdays\":[],\"startTime\":\"08:00:00\",\"endTime\":\"10:00:00\",\"endDayOffset\":0}]".to_owned(),
        "[{\"weekdays\":[\"sunday\"],\"startTime\":\"10:00:00\",\"endTime\":\"08:00:00\",\"endDayOffset\":0}]".to_owned(),
        "[{\"weekdays\":[\"sunday\"],\"weekdays\":[\"monday\"],\"startTime\":\"08:00:00\",\"endTime\":\"10:00:00\",\"endDayOffset\":0}]".to_owned(),
        " ".to_owned(), "[{\"token\":\"secret\"}]".to_owned(),
    ] {
        let proposal = preview(&document, &source(&[vec![id(30), raw]]), &policy, &[])?;
        assert_eq!(proposal.disposition, PeopleImportDisposition::Blocked);
        assert!(proposal.batch.is_none());
    }
    for field in [
        AvailabilityCsvField::AssignmentTypeIds,
        AvailabilityCsvField::LocationIds,
    ] {
        for raw in [
            "null".to_owned(),
            "[]".to_owned(),
            "{}".to_owned(),
            "[null]".to_owned(),
            "[1]".to_owned(),
            json!([id(1)]).to_string(),
            json!([id(999)]).to_string(),
            json!([id(4), id(4).to_ascii_uppercase()]).to_string(),
        ] {
            assert_eq!(
                preview(
                    &document,
                    &source(&[vec![id(30), raw]]),
                    &mapping(&[field]),
                    &[]
                )?
                .disposition,
                PeopleImportDisposition::Blocked
            );
        }
    }
    let mut invalid_document = document.clone();
    invalid_document
        .domain
        .entities
        .get_mut(&id(30).parse()?)
        .ok_or("availability")?["locationIds"] = Value::Null;
    assert_eq!(
        preview(
            &invalid_document,
            &source(&[vec![id(30)]]),
            &mapping(&[]),
            &[]
        )
        .err()
        .ok_or("current raw document")?
        .code,
        CsvErrorCode::InvalidCurrentDocument
    );
    Ok(())
}

#[test]
fn exactly_empty_cells_clear_only_optional_fields_and_new_rows_require_complete_authoring()
-> TestResult {
    let mut document = with_availability()?;
    let raw = document
        .domain
        .entities
        .get_mut(&id(30).parse()?)
        .ok_or("availability")?;
    raw["assignmentTypeIds"] = json!([id(4)]);
    raw["locationIds"] = json!([id(5)]);
    let mut policy = mapping(&[
        AvailabilityCsvField::AssignmentTypeIds,
        AvailabilityCsvField::LocationIds,
        AvailabilityCsvField::Source,
        AvailabilityCsvField::Note,
    ]);
    let text = source(&[vec![
        id(30),
        String::new(),
        String::new(),
        String::new(),
        String::new(),
    ]]);
    assert_eq!(
        preview(&document, &text, &policy, &[])?.disposition,
        PeopleImportDisposition::NoChanges
    );
    for column in &mut policy.columns {
        column.blank = BlankPolicy::Clear;
    }
    let cleared = preview(&document, &text, &policy, &[])?;
    let after = cleared.rows[0].after.as_ref().ok_or("cleared")?;
    assert!(after.get("locationIds").is_none());
    assert!(after.get("assignmentTypeIds").is_none());
    assert_eq!(after["source"], json!(""));
    assert_eq!(after["note"], json!(""));
    let mut whitespace = vec![
        id(30),
        String::new(),
        String::new(),
        " ".to_owned(),
        "\t".to_owned(),
    ];
    let authored = preview(&document, &source(&[whitespace.clone()]), &policy, &[])?;
    assert_eq!(
        authored.rows[0].after.as_ref().ok_or("authored")?["source"],
        json!(" ")
    );
    assert_eq!(
        authored.rows[0].after.as_ref().ok_or("authored")?["note"],
        json!("\t")
    );
    whitespace[1] = "[]".to_owned();
    assert_blocked(
        &preview(&document, &source(&[whitespace]), &policy, &[])?,
        AvailabilityCsvRowIssue::InvalidCell,
    );
    Ok(())
}

#[test]
fn required_clear_policies_are_refused_and_new_rows_require_complete_authoring() -> TestResult {
    let document = with_availability()?;
    for field in [
        AvailabilityCsvField::PersonId,
        AvailabilityCsvField::AvailabilityKind,
        AvailabilityCsvField::StartsAt,
        AvailabilityCsvField::EndsAt,
        AvailabilityCsvField::WeeklyWindows,
        AvailabilityCsvField::EffectiveStartDate,
        AvailabilityCsvField::EffectiveEndDateExclusive,
    ] {
        let mut required = mapping(&[field]);
        required.columns[0].blank = BlankPolicy::Clear;
        assert_eq!(
            preview(&document, "", &required, &[])
                .err()
                .ok_or("clear required")?
                .code,
            CsvErrorCode::InvalidMapping
        );
    }
    for index in 1..7 {
        let mut row = complete_row(id(31));
        row[index].clear();
        assert_blocked(
            &preview(
                &document,
                &source(&[row]),
                &complete_mapping(),
                &[decision(2, true, 31)?],
            )?,
            AvailabilityCsvRowIssue::MissingRequiredField,
        );
    }
    let added = preview(
        &document,
        &source(&[complete_row(id(31))]),
        &complete_mapping(),
        &[decision(2, true, 31)?],
    )?;
    let new = added.rows[0].after.as_ref().ok_or("new")?;
    assert_eq!(new["source"], json!(""));
    assert_eq!(new["note"], json!(""));
    assert!(new.get("locationIds").is_none());
    assert!(new.get("assignmentTypeIds").is_none());
    assert_eq!(
        new["effectiveRange"]["endDateExclusive"],
        json!("2026-12-01")
    );
    Ok(())
}

#[test]
fn local_intent_rejects_offsets_annotations_rollover_and_resolves_dst_with_captured_policy()
-> TestResult {
    let document = with_availability()?;
    let policy = mapping(&[AvailabilityCsvField::StartsAt, AvailabilityCsvField::EndsAt]);
    for start in [
        "2026-11-01T08:00Z",
        "2026-11-01T08:00-04:00",
        "2026-11-01T08:00[America/New_York]",
        "2026-11-01",
        " ",
    ] {
        assert_blocked(
            &preview(
                &document,
                &source(&[vec![id(30), start.to_owned(), String::new()]]),
                &policy,
                &[],
            )?,
            AvailabilityCsvRowIssue::InvalidLocalTime,
        );
    }
    for (start, end) in [
        ("2026-11-01T10:00", "2026-11-01T10:00"),
        ("2026-11-01T23:00", "2026-11-01T01:00"),
    ] {
        assert_blocked(
            &preview(
                &document,
                &source(&[vec![id(30), start.to_owned(), end.to_owned()]]),
                &policy,
                &[],
            )?,
            AvailabilityCsvRowIssue::NonpositiveInterval,
        );
    }
    Ok(())
}

#[test]
fn dst_fold_and_gap_resolution_retains_native_original_intent_and_signed_offsets() -> TestResult {
    let document = with_availability()?;
    let policy = mapping(&[AvailabilityCsvField::StartsAt, AvailabilityCsvField::EndsAt]);
    let mut ny = fixture()?;
    ny.domain.entities.insert(
        id(30).parse()?,
        document.domain.entities[&id(30).parse()?].clone(),
    );
    let folded = source(&[vec![
        id(30),
        "2026-11-01T01:30".to_owned(),
        "2026-11-01T02:30".to_owned(),
    ]]);
    assert_blocked(
        &preview(&ny, &folded, &policy, &[])?,
        AvailabilityCsvRowIssue::UnresolvableLocalTime,
    );
    for (overlap, instant, offset) in [
        (OverlapPolicy::Earlier, "2026-11-01T05:30:00Z", -14400),
        (OverlapPolicy::Later, "2026-11-01T06:30:00Z", -18000),
    ] {
        ny.settings.overlap_policy = overlap;
        let result = preview(&ny, &folded, &policy, &[])?;
        let fact = result.rows[0]
            .resolved_starts_at
            .as_ref()
            .ok_or("fold fact")?;
        assert_eq!(fact.instant.to_string(), instant);
        assert_eq!(fact.offset_seconds, offset);
        assert_eq!(fact.local.to_string(), "2026-11-01T01:30:00");
    }
    let gap = source(&[vec![
        id(30),
        "2026-03-08T02:30".to_owned(),
        "2026-03-08T04:30".to_owned(),
    ]]);
    assert_blocked(
        &preview(&ny, &gap, &policy, &[])?,
        AvailabilityCsvRowIssue::UnresolvableLocalTime,
    );
    ny.settings.gap_policy = GapPolicy::MoveForward;
    let resolved = preview(&ny, &gap, &policy, &[])?;
    let fact = resolved.rows[0]
        .resolved_starts_at
        .as_ref()
        .ok_or("gap fact")?;
    assert_eq!(fact.instant.to_string(), "2026-03-08T07:30:00Z");
    assert_eq!(fact.local.to_string(), "2026-03-08T02:30:00");
    Ok(())
}

#[test]
fn identity_census_same_id_add_absent_selection_duplicates_and_skip_have_no_implicit_winners()
-> TestResult {
    let document = with_availability()?;
    let policy = mapping(&[AvailabilityCsvField::Note]);
    let no_op = source(&[vec![id(30), String::new()]]);
    assert_eq!(
        preview(&document, &no_op, &policy, &[])?.rows[0].status,
        AvailabilityCsvRowStatus::Unchanged
    );
    let unresolved = preview(
        &document,
        &source(&[complete_row(id(31))]),
        &complete_mapping(),
        &[],
    )?;
    assert_eq!(
        unresolved.rows[0].identity,
        AvailabilityCsvIdentity::Supplied {
            availability_id: id(31).parse()?
        }
    );
    assert_eq!(
        unresolved.rows[0].issue,
        Some(AvailabilityCsvRowIssue::UnresolvedIdentity)
    );
    assert_eq!(
        preview(
            &document,
            &source(&[complete_row(id(31))]),
            &complete_mapping(),
            &[decision(2, true, 32)?]
        )
        .err()
        .ok_or("retarget supplied")?
        .code,
        CsvErrorCode::InvalidDecision
    );
    let mut absent = complete_row(String::new());
    let added = preview(
        &document,
        &source(&[absent.clone()]),
        &complete_mapping(),
        &[decision(2, true, 31)?],
    )?;
    assert_eq!(added.rows[0].identity, AvailabilityCsvIdentity::Absent {});
    assert_eq!(added.rows[0].availability_id, Some(id(31).parse()?));
    absent[0] = id(31);
    assert_eq!(
        preview(
            &document,
            &source(&[absent]),
            &complete_mapping(),
            &[decision(2, true, 31)?]
        )?
        .rows[0]
            .status,
        AvailabilityCsvRowStatus::Added
    );
    let selected = preview(
        &document,
        &source(&[vec![String::new(), "chosen".to_owned()]]),
        &policy,
        &[decision(2, false, 30)?],
    )?;
    assert_eq!(
        selected.rows[0].after.as_ref().ok_or("selected")?["note"],
        json!("chosen")
    );
    Ok(())
}

#[test]
fn owned_document_wrapper_census_and_invalid_identity_cannot_be_added() -> TestResult {
    let document = with_availability()?;
    let policy = mapping(&[AvailabilityCsvField::Note]);
    for target in [1, 7, 14, 100] {
        let row = complete_row(id(target));
        assert_blocked(
            &preview(
                &fixture()?,
                &source(&[row]),
                &complete_mapping(),
                &[decision(2, true, target)?],
            )?,
            AvailabilityCsvRowIssue::ReservedIdentity,
        );
    }
    let text = source(&[complete_row(id(31))]);
    let reserved = BTreeSet::from([id(31).parse()?]);
    let refusal = preview_availability_csv(
        &document,
        Revision::INITIAL,
        &mut Cursor::new(&text),
        &complete_mapping(),
        &[decision(2, true, 31)?],
        &reserved,
        &CancellationToken::default(),
    )?;
    assert_blocked(&refusal, AvailabilityCsvRowIssue::ReservedIdentity);
    assert_blocked(
        &preview(
            &document,
            &source(&[vec!["not-id".to_owned(), "note".to_owned()]]),
            &policy,
            &[decision(2, true, 31)?],
        )?,
        AvailabilityCsvRowIssue::InvalidIdentity,
    );
    Ok(())
}

#[test]
fn duplicate_targets_include_noops_invalid_rows_unknown_ids_and_explicit_selections_except_skip()
-> TestResult {
    let document = with_availability()?;
    let policy = mapping(&[AvailabilityCsvField::Note]);
    for second in [
        vec![id(30), String::new()],
        vec![id(30)],
        vec![id(30), "x".repeat(5000)],
    ] {
        let duplicate = preview(
            &document,
            &source(&[vec![id(30), String::new()], second]),
            &policy,
            &[],
        )?;
        assert!(
            duplicate
                .rows
                .iter()
                .all(|row| row.issue == Some(AvailabilityCsvRowIssue::DuplicateTarget))
        );
        assert!(duplicate.batch.is_none());
    }
    let unknown = preview(
        &document,
        &source(&[vec![id(31), String::new()], vec![id(31), String::new()]]),
        &policy,
        &[],
    )?;
    assert!(
        unknown
            .rows
            .iter()
            .all(|row| row.issue == Some(AvailabilityCsvRowIssue::DuplicateTarget))
    );
    let selected = preview(
        &document,
        &source(&[
            vec![String::new(), String::new()],
            vec![id(30), String::new()],
        ]),
        &policy,
        &[decision(2, false, 30)?],
    )?;
    assert!(
        selected
            .rows
            .iter()
            .all(|row| row.issue == Some(AvailabilityCsvRowIssue::DuplicateTarget))
    );
    let skipped = preview(
        &document,
        &source(&[
            vec![id(30), String::new()],
            vec![id(30), "ignored".to_owned()],
        ]),
        &policy,
        &[AvailabilityCsvRowDecision {
            record: 3,
            decision: AvailabilityCsvDecision::Skip {},
        }],
    )?;
    assert_eq!(skipped.disposition, PeopleImportDisposition::NoChanges);
    assert_eq!(skipped.rows[1].status, AvailabilityCsvRowStatus::Skipped);
    Ok(())
}

#[test]
fn effective_end_is_exclusive_and_recurrence_is_not_clipped_or_expanded() -> TestResult {
    let mut document = with_availability()?;
    let raw = document
        .domain
        .entities
        .get_mut(&id(30).parse()?)
        .ok_or("availability")?;
    raw["availabilityKind"] = json!("approvedTimeOff");
    raw["effectiveRange"] = json!({"startDate":"2026-10-01","endDateExclusive":"2026-11-01"});
    let selected = [AssignmentPair {
        person_id: id(1).parse()?,
        shift_id: id(8).parse()?,
    }];
    assert!(
        evaluate_assignment_rules(&document, &selected, None)?
            .evaluations
            .iter()
            .all(|record| record.satisfied)
    );
    let proposal = preview(
        &document,
        &source(&[vec![id(30), "2026-11-02".to_owned()]]),
        &mapping(&[AvailabilityCsvField::EffectiveEndDateExclusive]),
        &[],
    )?;
    let mutation = commands::apply_batch(&document, proposal.batch.as_ref().ok_or("batch")?)?;
    assert!(
        evaluate_assignment_rules(&mutation.document, &selected, None)?
            .evaluations
            .iter()
            .any(|record| !record.satisfied)
    );
    let mut windows = weekly();
    let duplicate = windows[0].clone();
    windows.as_array_mut().ok_or("weekly")?.push(duplicate);
    let recurrent = preview(
        &document,
        &source(&[vec![id(30), windows.to_string(), "2099-12-31".to_owned()]]),
        &mapping(&[
            AvailabilityCsvField::WeeklyWindows,
            AvailabilityCsvField::EffectiveEndDateExclusive,
        ]),
        &[],
    )?;
    let after = recurrent.rows[0].after.as_ref().ok_or("weekly")?;
    assert_eq!(
        after["timeWindow"],
        json!({"kind":"weekly","windows":windows})
    );
    assert_eq!(
        after["effectiveRange"],
        json!({"startDate":"2026-10-01","endDateExclusive":"2099-12-31"})
    );
    assert!(recurrent.rows[0].resolved_starts_at.is_none());
    assert!(recurrent.rows[0].resolved_ends_at.is_none());
    Ok(())
}

#[test]
fn retained_review_rebuild_binds_source_revision_snapshot_census_facts_and_exact_inverse()
-> TestResult {
    let document = with_availability()?;
    let text = source(&[vec![
        id(30),
        "2026-11-01T08:00".to_owned(),
        "reviewed".to_owned(),
    ]]);
    let approved = preview(
        &document,
        &text,
        &mapping(&[AvailabilityCsvField::StartsAt, AvailabilityCsvField::Note]),
        &[],
    )?;
    let review = approved.review.as_ref().ok_or("review")?;
    let digest = approved.approval_digest.as_deref().ok_or("digest")?;
    assert_eq!(
        decode_availability_csv_review(&serde_json::to_vec(review)?)?,
        *review
    );
    assert_eq!(
        rebuild_availability_csv_review(
            &document,
            Revision::INITIAL,
            &mut Cursor::new(&text),
            review,
            digest,
            &BTreeSet::new(),
            &CancellationToken::default()
        )?,
        approved
    );
    let mutation = commands::apply_batch(&document, approved.batch.as_ref().ok_or("batch")?)?;
    let undo = commands::apply_batch(&mutation.document, &mutation.inverse)?;
    assert_eq!(undo.document, document);
    assert_eq!(
        commands::apply_batch(&undo.document, &undo.inverse)?.document,
        mutation.document
    );
    let without_fact = preview(
        &document,
        &source(&[vec![id(30), String::new(), "reviewed".to_owned()]]),
        &mapping(&[AvailabilityCsvField::StartsAt, AvailabilityCsvField::Note]),
        &[],
    )?;
    assert_eq!(approved.rows[0].after, without_fact.rows[0].after);
    assert_ne!(
        approved.review.as_ref().ok_or("review")?.rows_blake3,
        without_fact.review.as_ref().ok_or("review")?.rows_blake3
    );
    Ok(())
}

#[test]
fn review_wire_rejects_unknown_versions_duplicate_keys_other_family_and_oversize() -> TestResult {
    let document = with_availability()?;
    let proposal = preview(&document, &source(&[vec![id(30)]]), &mapping(&[]), &[])?;
    let review = proposal.review.as_ref().ok_or("review")?;
    let mut raw = serde_json::to_value(review)?;
    raw["unexpected"] = json!(true);
    assert_eq!(
        decode_availability_csv_review(&serde_json::to_vec(&raw)?)
            .err()
            .ok_or("unknown")?
            .code,
        CsvErrorCode::InvalidReview
    );
    let mut newer = review.clone();
    newer.schema_version = 2;
    assert_eq!(
        decode_availability_csv_review(&serde_json::to_vec(&newer)?)
            .err()
            .ok_or("newer")?
            .code,
        CsvErrorCode::UnsupportedVersion
    );
    newer.schema_version = 1;
    newer.format = "eutheto/workforce-shift-csv-review".to_owned();
    assert_eq!(
        decode_availability_csv_review(&serde_json::to_vec(&newer)?)
            .err()
            .ok_or("family")?
            .code,
        CsvErrorCode::UnsupportedVersion
    );
    assert_eq!(
        decode_availability_csv_review(b"{\"format\":\"one\",\"format\":\"two\"}")
            .err()
            .ok_or("duplicate")?
            .code,
        CsvErrorCode::InvalidReview
    );
    assert_eq!(
        decode_availability_csv_review(&vec![b' '; MAX_CSV_REVIEW_BYTES + 1])
            .err()
            .ok_or("oversize")?
            .code,
        CsvErrorCode::ReviewLimit
    );
    Ok(())
}

#[test]
fn complete_exclusion_reports_preserve_every_native_reason_and_source_is_checked_before_skip()
-> TestResult {
    let document = with_availability()?;
    let rows = (0..1000)
        .map(|_| vec!["not-id".to_owned(), "never exported".to_owned()])
        .collect::<Vec<_>>();
    let decisions = (2..502)
        .map(|record| AvailabilityCsvRowDecision {
            record,
            decision: AvailabilityCsvDecision::Skip {},
        })
        .collect::<Vec<_>>();
    let proposal = preview(
        &document,
        &source(&rows),
        &mapping(&[AvailabilityCsvField::Note]),
        &decisions,
    )?;
    assert_eq!(proposal.excluded_rows.len(), 1000);
    assert!(
        proposal.excluded_rows[..500]
            .iter()
            .all(|row| row.skipped && row.issue.is_none())
    );
    assert!(
        proposal.excluded_rows[500..]
            .iter()
            .all(|row| !row.skipped && row.issue == Some(AvailabilityCsvRowIssue::InvalidIdentity))
    );
    assert!(serde_json::to_vec(&proposal.excluded_rows)?.len() < MAX_AVAILABILITY_CSV_REPORT_BYTES);
    assert!(!serde_json::to_string(&proposal.excluded_rows)?.contains("never exported"));
    let skipped = [AvailabilityCsvRowDecision {
        record: 2,
        decision: AvailabilityCsvDecision::Skip {},
    }];
    assert_eq!(
        preview_availability_csv(
            &document,
            Revision::INITIAL,
            &mut Cursor::new(b"header\tname\ninvalid\t\xff"),
            &mapping(&[AvailabilityCsvField::Note]),
            &skipped,
            &BTreeSet::new(),
            &CancellationToken::default(),
        )
        .err()
        .ok_or("invalid UTF-8 must not be skipped")?
        .code,
        CsvErrorCode::InvalidUtf8
    );
    assert!(
        preview(
            &document,
            &"x".repeat(MAX_CSV_SOURCE_BYTES + 1),
            &mapping(&[]),
            &[]
        )
        .is_err()
    );
    Ok(())
}

#[test]
fn mapping_and_decision_policy_is_admitted_before_scanning() -> TestResult {
    let document = with_availability()?;
    let mut duplicate = mapping(&[AvailabilityCsvField::Note, AvailabilityCsvField::Note]);
    assert_eq!(
        preview(&document, "", &duplicate, &[])
            .err()
            .ok_or("duplicate field")?
            .code,
        CsvErrorCode::InvalidMapping
    );
    duplicate.columns[1].field = AvailabilityCsvField::Source;
    duplicate.columns[1].index = 1;
    assert_eq!(
        preview(&document, "", &duplicate, &[])
            .err()
            .ok_or("duplicate column")?
            .code,
        CsvErrorCode::InvalidMapping
    );
    duplicate.columns[1].index = 0;
    assert_eq!(
        preview(&document, "", &duplicate, &[])
            .err()
            .ok_or("identity column")?
            .code,
        CsvErrorCode::InvalidMapping
    );
    let mut too_many = mapping(&[]);
    for index in 0..10001 {
        too_many
            .person_mappings
            .insert(format!("alias-{index}"), id(1).parse()?);
    }
    assert_eq!(
        preview(&document, "", &too_many, &[])
            .err()
            .ok_or("role map limit")?
            .code,
        CsvErrorCode::MappingLimit
    );
    let unsorted = [decision(3, false, 30)?, decision(2, false, 30)?];
    assert_eq!(
        preview(&document, "", &mapping(&[]), &unsorted)
            .err()
            .ok_or("ordered decisions")?
            .code,
        CsvErrorCode::InvalidDecision
    );
    assert_eq!(
        preview(&document, "one\ttwo\n", &mapping(&[]), &[])
            .err()
            .ok_or("header")?
            .code,
        CsvErrorCode::HeaderMismatch
    );
    assert_eq!(
        preview(
            &document,
            "one\n",
            &mapping(&[]),
            &[decision(2, false, 30)?]
        )
        .err()
        .ok_or("outside source")?
        .code,
        CsvErrorCode::InvalidDecision
    );
    Ok(())
}

#[test]
fn cancellation_before_and_during_read_is_refused_without_mutation() -> TestResult {
    struct CancelRead<'a> {
        token: &'a CancellationToken,
    }
    impl Read for CancelRead<'_> {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            self.token.cancel();
            Ok(0)
        }
    }
    let document = with_availability()?;
    let token = CancellationToken::default();
    token.cancel();
    assert_eq!(
        preview_availability_csv(
            &document,
            Revision::INITIAL,
            &mut Cursor::new(""),
            &mapping(&[]),
            &[],
            &BTreeSet::new(),
            &token
        )
        .err()
        .ok_or("before read")?
        .code,
        CsvErrorCode::Cancelled
    );
    let token = CancellationToken::default();
    assert_eq!(
        preview_availability_csv(
            &document,
            Revision::INITIAL,
            &mut CancelRead { token: &token },
            &mapping(&[]),
            &[],
            &BTreeSet::new(),
            &token
        )
        .err()
        .ok_or("during read")?
        .code,
        CsvErrorCode::Cancelled
    );
    assert_eq!(document, with_availability()?);
    Ok(())
}

#[test]
fn actual_mutations_are_one_bounded_batch_and_projection_budget_charges_every_retained_tree()
-> TestResult {
    let document = base()?;
    let rows = (1000..2001)
        .map(|index| complete_row(id(index)))
        .collect::<Vec<_>>();
    let decisions = (1000..2001)
        .zip(2..)
        .map(|(index, record)| decision(record, true, index))
        .collect::<TestResult<Vec<_>>>()?;
    assert_eq!(
        preview(&document, &source(&rows), &complete_mapping(), &decisions)
            .err()
            .ok_or("mutation cap")?
            .code,
        CsvErrorCode::MutationLimit
    );
    let mut policy = complete_mapping();
    policy.columns.push(AvailabilityCsvColumn {
        index: 7,
        field: AvailabilityCsvField::WeeklyWindows,
        blank: BlankPolicy::Preserve,
    });
    policy.expected_columns = 8;
    let windows = json!(vec![weekly()[0].clone(); 128]).to_string();
    let rows = (1000..1700)
        .map(|index| {
            let mut row = complete_row(id(index));
            row[3].clear();
            row[4].clear();
            row.push(windows.clone());
            row
        })
        .collect::<Vec<_>>();
    assert_eq!(
        preview(&document, &source(&rows), &policy, &decisions[..700])
            .err()
            .ok_or("projection cap")?
            .code,
        CsvErrorCode::PreviewLimit
    );
    Ok(())
}

#[test]
fn original_record_limit_and_aggregate_role_mapping_bytes_are_not_per_map_loopholes() -> TestResult
{
    let document = with_availability()?;
    let mut policy = mapping(&[]);
    policy.has_header = false;
    let text = "not-id\n".repeat(10001);
    assert_eq!(
        preview(&document, &text, &policy, &[])
            .err()
            .ok_or("record limit")?
            .code,
        CsvErrorCode::DataRecordLimit
    );
    let mut aggregate = mapping(&[]);
    for index in 0..2000 {
        let token = format!("{index:04}-{}", "a".repeat(200));
        aggregate
            .person_mappings
            .insert(token.clone(), id(1).parse()?);
        aggregate
            .assignment_type_mappings
            .insert(token.clone(), id(4).parse()?);
        aggregate.location_mappings.insert(token, id(5).parse()?);
    }
    assert_eq!(
        preview(&document, "", &aggregate, &[])
            .err()
            .ok_or("aggregate role map")?
            .code,
        CsvErrorCode::MappingLimit
    );
    Ok(())
}

fn approved_note() -> TestResult<(ScenarioDocument, String, AvailabilityImportPreview)> {
    let document = with_availability()?;
    let text = source(&[vec![
        id(30),
        "2026-11-01T08:00".to_owned(),
        "reviewed".to_owned(),
    ]]);
    let approved = preview(
        &document,
        &text,
        &mapping(&[AvailabilityCsvField::StartsAt, AvailabilityCsvField::Note]),
        &[],
    )?;
    Ok((document, text, approved))
}

#[test]
fn retained_rebuild_refuses_changed_revision_document_source_or_native_census() -> TestResult {
    let (document, text, approved) = approved_note()?;
    let review = approved.review.as_ref().ok_or("review")?;
    let digest = approved.approval_digest.as_deref().ok_or("digest")?;
    let mut altered = document.clone();
    altered.metadata.title = "changed".to_owned();
    for (snapshot, revision, input, census) in [
        (&document, Revision::new(1), text.clone(), BTreeSet::new()),
        (&altered, Revision::INITIAL, text.clone(), BTreeSet::new()),
        (
            &document,
            Revision::INITIAL,
            format!("{text}\n"),
            BTreeSet::new(),
        ),
        (
            &document,
            Revision::INITIAL,
            text.clone(),
            BTreeSet::from([id(90).parse()?]),
        ),
    ] {
        assert_eq!(
            rebuild_availability_csv_review(
                snapshot,
                revision,
                &mut Cursor::new(input),
                review,
                digest,
                &census,
                &CancellationToken::default()
            )
            .err()
            .ok_or("stale")?
            .code,
            CsvErrorCode::StaleReview
        );
    }
    Ok(())
}

#[test]
fn retained_rebuild_refuses_tampered_rows_batch_report_or_mapping() -> TestResult {
    let (document, text, approved) = approved_note()?;
    let review = approved.review.as_ref().ok_or("review")?;
    let digest = approved.approval_digest.as_deref().ok_or("digest")?;
    for part in 0..4 {
        let mut tampered = review.clone();
        match part {
            0 => tampered.rows_blake3 = "0".repeat(64),
            1 => tampered.changes_blake3 = "0".repeat(64),
            2 => tampered.excluded_blake3 = "0".repeat(64),
            _ => tampered.mapping.columns[1].blank = BlankPolicy::Clear,
        }
        assert_eq!(
            rebuild_availability_csv_review(
                &document,
                Revision::INITIAL,
                &mut Cursor::new(&text),
                &tampered,
                digest,
                &BTreeSet::new(),
                &CancellationToken::default()
            )
            .err()
            .ok_or("tampered")?
            .code,
            CsvErrorCode::StaleReview
        );
    }
    Ok(())
}
