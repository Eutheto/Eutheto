use eutheto_solver_api::{
    BackendStability, CapabilityMatrix, DescriptorError, LicenseMetadata, SolverDescriptor,
    SolverDistribution, SupportMatrixError,
};
use eutheto_types::{BackendId, NamespacedIdError};
use thiserror::Error;

/// Stable public identifier for the OR-Tools CP-SAT backend.
pub const ORTOOLS_BACKEND_ID: &str = "solver.ortools-cp-sat";
/// Exact OR-Tools source version implemented by this adapter.
pub const ORTOOLS_VERSION: &str = "9.15.6755";
/// Version of the Rust-to-worker adapter contract.
pub const ORTOOLS_ADAPTER_VERSION: &str = "0.1.0";

/// Builds the immutable public descriptor for the bundled OR-Tools worker.
///
/// Capability declarations are derived directly from the generated production
/// support matrix so the descriptor cannot become a second source of truth.
///
/// # Errors
///
/// Returns an error if a reviewed identifier or descriptor field no longer
/// satisfies the shared solver API contract.
pub fn ortools_descriptor() -> Result<SolverDescriptor, OrToolsDescriptorError> {
    let id = BackendId::new(ORTOOLS_BACKEND_ID)?;
    let capabilities = CapabilityMatrix::generated()?.backend_capabilities(&id)?;
    let descriptor = SolverDescriptor {
        id,
        display_name: "OR-Tools CP-SAT".to_owned(),
        version: ORTOOLS_VERSION.to_owned(),
        adapter_version: ORTOOLS_ADAPTER_VERSION.to_owned(),
        distribution: SolverDistribution::BundledWorker,
        license: LicenseMetadata {
            spdx_expression: "Apache-2.0".to_owned(),
            license_name: "Apache License 2.0".to_owned(),
            source_url: Some(
                "https://github.com/google/or-tools/archive/refs/tags/v9.15.tar.gz".to_owned(),
            ),
        },
        stability: BackendStability::Beta,
        capabilities,
    };
    descriptor.validate()?;
    Ok(descriptor)
}

/// A reviewed OR-Tools descriptor constant no longer satisfies its shared contract.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum OrToolsDescriptorError {
    #[error("the reviewed OR-Tools descriptor contains an invalid backend identifier")]
    InvalidBackendIdentifier(#[from] NamespacedIdError),
    #[error("the generated OR-Tools support-matrix column is invalid: {0}")]
    InvalidCapability(#[from] SupportMatrixError),
    #[error("the reviewed OR-Tools descriptor is invalid: {0}")]
    InvalidDescriptor(#[from] DescriptorError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptor_exposes_exact_bundled_worker_identity_and_capabilities()
    -> Result<(), OrToolsDescriptorError> {
        let descriptor = ortools_descriptor()?;

        assert_eq!(descriptor.id.as_str(), ORTOOLS_BACKEND_ID);
        assert_eq!(descriptor.display_name, "OR-Tools CP-SAT");
        assert_eq!(descriptor.version, ORTOOLS_VERSION);
        assert_eq!(descriptor.adapter_version, ORTOOLS_ADAPTER_VERSION);
        assert_eq!(descriptor.distribution, SolverDistribution::BundledWorker);
        assert_eq!(descriptor.stability, BackendStability::Beta);
        assert_eq!(descriptor.license.spdx_expression, "Apache-2.0");
        assert_eq!(descriptor.license.license_name, "Apache License 2.0");
        assert_eq!(
            descriptor.license.source_url.as_deref(),
            Some("https://github.com/google/or-tools/archive/refs/tags/v9.15.tar.gz")
        );
        CapabilityMatrix::generated()?.validate_descriptor(&descriptor)?;
        descriptor.validate()?;
        Ok(())
    }
}
