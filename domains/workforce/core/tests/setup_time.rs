mod support;

use eutheto_domain_api::{
    DomainPack, DomainPackError, DomainSetupQueryV1, DomainViewInput, SetupViewContext,
};
use eutheto_types::{
    CancellationToken, GapPolicy, Horizon, OperationControl, OverlapPolicy, Revision,
    ScenarioDocument,
};
use eutheto_workforce::{
    WorkforcePack,
    setup::contracts::{WorkforceSetupResultV1, WorkforceSetupViewDataV1},
};
use jiff::civil::Time;
use serde_json::json;
use std::error::Error;

type TestResult<T = ()> = Result<T, Box<dyn Error>>;

fn read_view(
    document: &ScenarioDocument,
    view_id: &str,
    parameters: serde_json::Value,
) -> Result<WorkforceSetupViewDataV1, DomainPackError> {
    let request = DomainSetupQueryV1 {
        schema_version: 1,
        view_id: view_id.to_owned(),
        parameters,
        continuation: None,
    };
    let output = WorkforcePack.build_view(
        DomainViewInput::StoredSetup {
            document,
            query: &request,
            context: SetupViewContext {
                revision: Revision::INITIAL,
                query_fingerprint: [3; 32],
            },
        },
        &OperationControl::Cancellation(CancellationToken::new()),
    )?;
    serde_json::from_value::<WorkforceSetupResultV1>(output.view.data)
        .map(|value| value.result)
        .map_err(|error| DomainPackError::Contract(error.to_string()))
}

#[test]
fn local_time_preparation_rejects_external_time_authority() -> TestResult {
    let document = support::fixture()?;
    for local in [
        "2026-11-02T12:00:00+00:00",
        "2026-11-02T12:00:00[Asia/Tokyo]",
    ] {
        let Err(DomainPackError::SetupValidation(issue)) = read_view(
            &document,
            "official.workforce.setup.local_time_resolution",
            json!({"local":local}),
        ) else {
            return Err(
                "an offset or timezone annotation was not rejected as a local field error".into(),
            );
        };
        assert_eq!(issue.code, "workforce.time.invalid_local");
        assert_eq!(issue.field_path.as_deref(), Some("/query/parameters/local"));
    }
    Ok(())
}

#[test]
fn scalar_resolution_preserves_gap_intent_and_both_fold_choices() -> TestResult {
    let mut document = support::fixture()?;
    for (local, gap, overlap, instant, offset) in [
        (
            "2026-03-08T02:30:00",
            GapPolicy::MoveForward,
            OverlapPolicy::Reject,
            "2026-03-08T07:30:00Z",
            -14_400,
        ),
        (
            "2026-11-01T01:30:00",
            GapPolicy::Reject,
            OverlapPolicy::Earlier,
            "2026-11-01T05:30:00Z",
            -14_400,
        ),
        (
            "2026-11-01T01:30:00",
            GapPolicy::Reject,
            OverlapPolicy::Later,
            "2026-11-01T06:30:00Z",
            -18_000,
        ),
    ] {
        document.settings.gap_policy = gap;
        document.settings.overlap_policy = overlap;
        let WorkforceSetupViewDataV1::LocalTimeResolution(resolved) = read_view(
            &document,
            "official.workforce.setup.local_time_resolution",
            json!({"local":local}),
        )?
        else {
            return Err("wrong scalar result family".into());
        };
        assert_eq!(resolved.local, local.parse()?);
        assert_eq!(resolved.instant, instant.parse()?);
        assert_eq!(resolved.offset_seconds, offset);
    }
    // The spring input above is deliberately outside this captured November horizon.
    assert_eq!(
        document.settings.horizon.start,
        "2026-11-01T04:00:00Z".parse()?
    );
    Ok(())
}

#[test]
fn rejected_and_malformed_local_times_return_safe_exact_field_findings() -> TestResult {
    let document = support::fixture()?;
    for (local, code) in [
        ("2026-03-08T02:30:00", "workforce.time.gap"),
        ("2026-11-01T01:30:00", "workforce.time.overlap"),
        (
            "secret-sentinel /private/path",
            "workforce.time.invalid_local",
        ),
    ] {
        let Err(DomainPackError::SetupValidation(issue)) = read_view(
            &document,
            "official.workforce.setup.local_time_resolution",
            json!({"local":local}),
        ) else {
            return Err("expected a field-addressable local-time finding".into());
        };
        assert_eq!(issue.code, code);
        assert_eq!(issue.field_path.as_deref(), Some("/query/parameters/local"));
        assert!(!issue.message.contains("secret-sentinel"));
        assert!(!issue.message.contains("/private/path"));
    }
    Ok(())
}

#[test]
fn native_dates_project_civil_boundaries_and_clamp_large_or_terminal_horizons() -> TestResult {
    let mut document = support::fixture()?;
    document.domain.entities.clear();
    document.domain.locked_assignments.clear();
    for (zone, start, end, first_date, end_date) in [
        (
            "Asia/Tokyo",
            "2026-10-31T15:00:00Z",
            "2026-11-01T15:00:00Z",
            "2026-11-01",
            "2026-11-02",
        ),
        (
            "America/New_York",
            "2026-11-01T04:00:00Z",
            "2028-01-01T05:00:00Z",
            "2026-11-01",
            "2028-01-01",
        ),
        (
            "Pacific/Kiritimati",
            "9999-12-29T10:00:00Z",
            "9999-12-30T10:00:00Z",
            "9999-12-30",
            "9999-12-31",
        ),
    ] {
        document.settings.time_zone = zone.parse()?;
        document.settings.horizon = Horizon::new(start.parse()?, end.parse()?)?;
        let WorkforceSetupViewDataV1::Overview(facts) =
            read_view(&document, "official.workforce.setup.overview", json!({}))?
        else {
            return Err("wrong overview result family".into());
        };
        assert_eq!(facts.planning_dates.start_date, first_date.parse()?);
        assert_eq!(facts.planning_dates.end_date_exclusive, end_date.parse()?);
        assert_eq!(
            facts.initial_work_window.start_date,
            facts.planning_dates.start_date
        );
        assert!(
            facts.initial_work_window.end_date_exclusive <= facts.planning_dates.end_date_exclusive
        );
        let days = facts
            .initial_work_window
            .start_date
            .to_datetime(Time::MIN)
            .duration_until(
                facts
                    .initial_work_window
                    .end_date_exclusive
                    .to_datetime(Time::MIN),
            )
            .as_secs()
            / 86_400;
        assert!((1..=366).contains(&days));
    }
    Ok(())
}
