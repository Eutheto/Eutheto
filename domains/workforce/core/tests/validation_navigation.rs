mod support;

use eutheto_domain_api::DomainPack;
use eutheto_types::{CancellationToken, OperationControl};
use eutheto_workforce::WorkforcePack;
use serde_json::json;
use std::error::Error;

type TestResult = Result<(), Box<dyn Error>>;

#[test]
fn full_validation_preserves_template_endpoint_and_occurrence() -> TestResult {
    for (timing, endpoint) in [
        (
            json!({"kind":"elapsedDuration","startTime":"01:30:00","durationMinutes":60}),
            "startTime",
        ),
        (
            json!({"kind":"localWindow","startTime":"00:30:00","endTime":"01:30:00","endDayOffset":0}),
            "endTime",
        ),
    ] {
        let mut document = support::fixture()?;
        document
            .domain
            .entities
            .get_mut(&support::id(6).parse()?)
            .ok_or("template")?["timing"] = timing;
        let report = WorkforcePack.validate_full(
            &document,
            &OperationControl::Cancellation(CancellationToken::new()),
        )?;
        let issue = report
            .issues
            .iter()
            .find(|issue| issue.code == "official.workforce.temporal_review")
            .ok_or("temporal finding")?;
        assert_eq!(
            issue.field_path,
            Some(format!(
                "/domain/entities/{}/timing/{endpoint}",
                support::id(6)
            ))
        );
        assert!(issue.message.contains("Occurrence date: 2026-11-01"));
    }
    Ok(())
}

#[test]
fn full_validation_preserves_authored_weekly_row_not_first_window() -> TestResult {
    let mut document = support::fixture()?;
    document.domain.entities.remove(&support::id(6).parse()?);
    document.domain.locked_assignments.clear();
    document.domain.entities.insert(support::id(20).parse()?, json!({
        "kind":"availability", "id":support::id(20), "personId":support::id(1),
        "availabilityKind":"unavailable",
        "source":"", "note":"",
        "effectiveRange":{"startDate":"2026-10-31","endDateExclusive":"2026-11-03"},
        "timeWindow":{"kind":"weekly","windows":[
            {"weekdays":["sunday"],"startTime":"03:00:00","endTime":"04:00:00","endDayOffset":0},
            {"weekdays":["sunday"],"startTime":"01:30:00","endTime":"02:30:00","endDayOffset":0}
        ]}
    }));
    document.domain.rules.insert(
        support::id(21).parse()?,
        json!({
            "kind":"availability", "id":support::id(21), "active":true,
            "strength":"required", "scope":{"people":{"kind":"all"}}
        }),
    );
    let report = WorkforcePack.validate_full(
        &document,
        &OperationControl::Cancellation(CancellationToken::new()),
    )?;
    let issue = report
        .issues
        .iter()
        .find(|issue| issue.code == "official.workforce.temporal_review")
        .ok_or("temporal finding")?;
    assert_eq!(
        issue.field_path,
        Some(format!(
            "/domain/entities/{}/timeWindow/windows/1/startTime",
            support::id(20)
        ))
    );
    assert!(issue.message.contains("Occurrence date: 2026-11-01"));
    Ok(())
}

#[test]
fn full_validation_targets_the_reversed_instant_availability_end() -> TestResult {
    let mut document = support::fixture()?;
    document.domain.entities.remove(&support::id(6).parse()?);
    document.domain.locked_assignments.clear();
    document.domain.entities.insert(
        support::id(20).parse()?,
        json!({
            "kind":"availability", "id":support::id(20), "personId":support::id(1),
            "availabilityKind":"unavailable",
            "source":"", "note":"",
            "effectiveRange":{"startDate":"2026-10-31","endDateExclusive":"2026-11-03"},
            "timeWindow":{"kind":"instant",
                "startsAt":"2026-11-01T09:00:00Z","endsAt":"2026-11-01T08:00:00Z"}
        }),
    );
    let report = WorkforcePack.validate_full(
        &document,
        &OperationControl::Cancellation(CancellationToken::new()),
    )?;
    assert!(report.issues.iter().any(|issue| issue.field_path
        == Some(format!(
            "domain.entities.{}.timeWindow.endsAt",
            support::id(20)
        ))));
    Ok(())
}

#[test]
fn full_validation_identifies_authored_person_active_range_endpoint() -> TestResult {
    for (start, end, field) in [
        ("2018-11-04", "2018-11-05", "startDate"),
        ("2018-11-03", "2018-11-04", "endDateExclusive"),
    ] {
        let mut document = support::fixture()?;
        document.settings.time_zone = "America/Sao_Paulo".parse()?;
        document.settings.horizon = eutheto_types::Horizon::new(
            "2018-11-03T03:00:00Z".parse()?,
            "2018-11-06T02:00:00Z".parse()?,
        )?;
        document.domain.entities.remove(&support::id(6).parse()?);
        document.domain.entities.remove(&support::id(8).parse()?);
        document.domain.locked_assignments.clear();
        document
            .domain
            .entities
            .get_mut(&support::id(1).parse()?)
            .ok_or("person")?["activeRange"] =
            json!({"kind":"dateRange","startDate":start,"endDateExclusive":end});
        let report = WorkforcePack.validate_full(
            &document,
            &OperationControl::Cancellation(CancellationToken::new()),
        )?;
        let issue = report
            .issues
            .iter()
            .find(|issue| issue.code == "official.workforce.temporal_review")
            .ok_or("active-range temporal finding")?;
        assert_eq!(
            issue.field_path,
            Some(format!(
                "/domain/entities/{}/activeRange/{field}",
                support::id(1)
            ))
        );
    }
    Ok(())
}

#[test]
fn full_validation_targets_the_malformed_stored_end_not_start() -> TestResult {
    let mut document = support::fixture()?;
    document
        .domain
        .entities
        .get_mut(&support::id(8).parse()?)
        .ok_or("instance")?["endsAt"]["local"] = json!("2026-11-01T03:30:00");
    let report = WorkforcePack.validate_full(
        &document,
        &OperationControl::Cancellation(CancellationToken::new()),
    )?;
    assert_eq!(
        report.issues[0].field_path,
        Some(format!("domain.entities.{}.endsAt.local", support::id(8)))
    );
    Ok(())
}

#[test]
fn full_validation_targets_later_qualification_grant_expiry() -> TestResult {
    let mut document = support::fixture()?;
    document
        .domain
        .entities
        .get_mut(&support::id(1).parse()?)
        .ok_or("person")?["qualificationGrants"] = json!([
        {"qualificationId":support::id(11)},
        {"qualificationId":support::id(11), "effectiveFrom":"2026-11-02T00:00:00Z", "expiresAt":"2026-11-01T00:00:00Z"}
    ]);
    let report = WorkforcePack.validate_full(
        &document,
        &OperationControl::Cancellation(CancellationToken::new()),
    )?;
    assert_eq!(
        report.issues[0].field_path,
        Some(format!(
            "domain.entities.{}.qualificationGrants.1.expiresAt",
            support::id(1)
        ))
    );
    Ok(())
}

#[test]
fn full_validation_preserves_rule_scope_reference_fields() -> TestResult {
    for (owner, selection, field) in [
        (
            "scope",
            json!({"people":{"kind":"selected","personIds":[support::id(99)]}}),
            "people.personIds",
        ),
        (
            "beforeScope",
            json!({"people":{"kind":"selected","personIds":[support::id(99)]}}),
            "people.personIds",
        ),
        (
            "afterScope",
            json!({"people":{"kind":"selected","personIds":[support::id(99)]}}),
            "people.personIds",
        ),
        (
            "scope",
            json!({"people":{"kind":"all"},"teamIds":[support::id(99)]}),
            "teamIds",
        ),
        (
            "scope",
            json!({"people":{"kind":"all"},"assignmentTypeIds":[support::id(99)]}),
            "assignmentTypeIds",
        ),
        (
            "scope",
            json!({"people":{"kind":"all"},"locationIds":[support::id(99)]}),
            "locationIds",
        ),
    ] {
        let mut document = support::fixture()?;
        document.domain.rules.clear();
        let mut rule = json!({
            "kind":"minimumRest", "id":support::id(21), "active":true,
            "strength":"required", "minimumMinutes":600,
            "scope":{"people":{"kind":"all"}},
            "beforeScope":{"people":{"kind":"all"}},
            "afterScope":{"people":{"kind":"all"}}
        });
        rule[owner] = selection;
        document.domain.rules.insert(support::id(21).parse()?, rule);
        let report = WorkforcePack.validate_full(
            &document,
            &OperationControl::Cancellation(CancellationToken::new()),
        )?;
        assert_eq!(
            report.issues[0].field_path,
            Some(format!("domain.rules.{}.{owner}.{field}", support::id(21)))
        );
    }
    Ok(())
}
