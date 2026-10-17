use super::{NativeOperationRequest, decode_diagnostic_request, validate_checksum};
use eutheto_core::DiagnosticExportRequestV1;
use eutheto_types::{OperationId, RequestId, Revision, ScenarioId, SolveRunId, SystemIdGenerator};
use serde_json::json;
use std::error::Error;

#[test]
fn checksum_admission_matches_core_hex_shape() {
    assert!(validate_checksum(&"A".repeat(64), "/checksum").is_ok());
    assert!(validate_checksum(&"g".repeat(64), "/checksum").is_err());
    assert!(validate_checksum(&"a".repeat(63), "/checksum").is_err());
}

#[test]
fn diagnostic_request_deserialization_rejects_unknown_fields() -> Result<(), Box<dyn Error>> {
    let request = DiagnosticExportRequestV1 {
        schema_version: 1,
        operation_id: OperationId::new(&SystemIdGenerator)?,
        request_id: RequestId::new(&SystemIdGenerator)?,
        scenario_id: ScenarioId::new(&SystemIdGenerator)?,
        source_revision: Revision::INITIAL,
        run_id: SolveRunId::new(&SystemIdGenerator)?,
        run_manifest_checksum: "a".repeat(64),
        evidence_checksum: "b".repeat(64),
    };
    let mut value = serde_json::to_value(request)?;
    value
        .as_object_mut()
        .ok_or("diagnostic request did not serialize as an object")?
        .insert("unexpected".to_owned(), json!(true));
    assert!(
        decode_diagnostic_request::<DiagnosticExportRequestV1>(NativeOperationRequest(Some(value)))
            .is_err()
    );
    Ok(())
}
