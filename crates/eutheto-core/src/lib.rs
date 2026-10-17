//! Platform-neutral application services, including authenticated stateless manual review and
//! journaled atomic selection/protection admission. Accepted originals remain immutable.

#[path = "app.rs"]
mod application;
mod counterfactual;
mod infeasibility;
mod infeasibility_diagnostic;
mod native_observations;
mod verification;

pub use application::*;
pub use counterfactual::*;
pub use verification::*;

use eutheto_types::FoundationStatus;

/// Immutable service that reports the capability of this compiled foundation.
///
/// Its identity is fixed by Cargo at compile time. It performs no environment,
/// filesystem, clock, network, or mutable-state access.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FoundationStatusService {
    package_name: &'static str,
    package_version: &'static str,
}

impl FoundationStatusService {
    /// Creates a service bound to this compiled core package.
    #[must_use]
    pub const fn current() -> Self {
        Self {
            package_name: env!("CARGO_PKG_NAME"),
            package_version: env!("CARGO_PKG_VERSION"),
        }
    }

    /// Returns the Cargo package name embedded at compile time.
    #[must_use]
    pub const fn package_name(self) -> &'static str {
        self.package_name
    }

    /// Returns the Cargo package version embedded at compile time.
    #[must_use]
    pub const fn package_version(self) -> &'static str {
        self.package_version
    }

    /// Returns the stable Phase-01 application capability status.
    #[must_use]
    pub const fn status(self) -> FoundationStatus {
        FoundationStatus::phase_01()
    }
}

impl Default for FoundationStatusService {
    fn default() -> Self {
        Self::current()
    }
}

/// Returns the stable status of the compiled Phase-01 application core.
#[must_use]
pub const fn foundation_status() -> FoundationStatus {
    FoundationStatusService::current().status()
}

#[cfg(test)]
mod tests {
    use super::{FoundationStatusService, foundation_status};
    use eutheto_types::{CapabilityState, FOUNDATION_STATUS_SCHEMA_VERSION};

    #[test]
    fn service_reports_compiled_foundation_deterministically() {
        let first = FoundationStatusService::current();
        let second = FoundationStatusService::current();

        assert_eq!(first, second);
        assert_eq!(first.package_name(), env!("CARGO_PKG_NAME"));
        assert_eq!(first.package_version(), env!("CARGO_PKG_VERSION"));
        assert_eq!(first.status(), foundation_status());
        assert_eq!(
            first.status().schema_version,
            FOUNDATION_STATUS_SCHEMA_VERSION
        );
        assert_eq!(first.status().capability, CapabilityState::Phase01Core);
    }
}
