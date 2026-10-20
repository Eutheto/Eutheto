#![forbid(unsafe_code)]

use eutheto_core::{
    AppCommand, AppDependencies, AppPaths, AppQuery, AppQueryResult,
    DIAGNOSTIC_COPY_API_SCHEMA_VERSION, DIAGNOSTIC_EXPORT_API_SCHEMA_VERSION,
    DiagnosticCopyRequestV1, DiagnosticExportRequestV1, EuthetoApp, ProjectScope,
};
use eutheto_domain_ir::AssumptionGroupId;
use eutheto_types::{
    CancellationToken, FixedClock, FixedMonotonicClock, OperationControl, OperationId, RequestId,
    Revision, ScenarioId, SolveRunId, SystemIdGenerator,
};
use std::{error::Error, sync::Arc};
use tempfile::TempDir;

const NOW: &str = "2026-09-07T12:00:00Z";
const HASH: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

type TestResult<T = ()> = Result<T, Box<dyn Error>>;

fn boxed(error: &eutheto_types::AppError) -> Box<dyn Error> {
    std::io::Error::other(format!("{error:?}")).into()
}

struct Fixture {
    directory: TempDir,
    app: EuthetoApp,
}

impl Fixture {
    async fn new() -> TestResult<Self> {
        let home = dirs::home_dir().ok_or("platform home directory is unavailable")?;
        let directory = tempfile::Builder::new()
            .prefix("eutheto-diagnostic-actions-")
            .tempdir_in(home)?;
        let app = EuthetoApp::open(AppDependencies {
            paths: AppPaths {
                database: directory.path().join("eutheto.sqlite"),
                safety_backups: directory.path().join("backups"),
            },
            clock: Arc::new(FixedClock::new(NOW.parse()?)),
            monotonic_clock: Arc::new(FixedMonotonicClock::default()),
            ids: Arc::new(SystemIdGenerator),
            cancellation: CancellationToken::new(),
        })
        .await
        .map_err(|error| boxed(&error))?;
        Ok(Self { directory, app })
    }

    fn copy_request() -> TestResult<DiagnosticCopyRequestV1> {
        Ok(DiagnosticCopyRequestV1 {
            schema_version: DIAGNOSTIC_COPY_API_SCHEMA_VERSION,
            operation_id: OperationId::new(&SystemIdGenerator)?,
            request_id: RequestId::new(&SystemIdGenerator)?,
            scenario_id: ScenarioId::new(&SystemIdGenerator)?,
            source_revision: Revision::INITIAL,
            expected_library_revision: Revision::INITIAL,
            run_id: SolveRunId::new(&SystemIdGenerator)?,
            run_manifest_checksum: HASH.to_owned(),
            evidence_checksum: HASH.to_owned(),
            group_id: AssumptionGroupId::new("tests.diagnostic.group")?,
        })
    }
}

#[tokio::test]
async fn missing_copy_authority_is_rejected_without_creating_a_project() -> TestResult {
    let fixture = Fixture::new().await?;
    let result = fixture
        .app
        .execute(AppCommand::CreateDiagnosticCopy {
            request: Fixture::copy_request()?,
            control: OperationControl::Cancellation(CancellationToken::new()),
        })
        .await;
    assert!(result.is_err());
    assert!(matches!(
        fixture
            .app
            .query(AppQuery::ListProjects(ProjectScope::All))
            .await
            .map_err(|error| boxed(&error))?,
        AppQueryResult::Projects(projects) if projects.is_empty()
    ));
    Ok(())
}

#[tokio::test]
async fn missing_export_authority_is_rejected_without_staging_a_destination() -> TestResult {
    let fixture = Fixture::new().await?;
    let destination = fixture.directory.path().join("diagnostic.json");
    let request = DiagnosticExportRequestV1 {
        schema_version: DIAGNOSTIC_EXPORT_API_SCHEMA_VERSION,
        operation_id: OperationId::new(&SystemIdGenerator)?,
        request_id: RequestId::new(&SystemIdGenerator)?,
        scenario_id: ScenarioId::new(&SystemIdGenerator)?,
        source_revision: Revision::INITIAL,
        run_id: SolveRunId::new(&SystemIdGenerator)?,
        run_manifest_checksum: HASH.to_owned(),
        evidence_checksum: HASH.to_owned(),
    };
    let result = fixture
        .app
        .prepare_diagnostic_export(
            request,
            OperationControl::Cancellation(CancellationToken::new()),
        )
        .await;
    assert!(result.is_err());
    assert!(!destination.exists());
    Ok(())
}
