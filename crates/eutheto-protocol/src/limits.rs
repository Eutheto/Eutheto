use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;

use serde::Deserialize;

use crate::ProtocolFault;

pub const DESCRIPTOR_BYTES: &[u8] =
    include_bytes!("../../../protocol/generated/eutheto.worker.v1.descriptor.pb");
pub const POLICY_JSON: &str = include_str!("../../../protocol/version.json");
/// Assumption-core evidence is admitted only through the verified single-worker satisfaction profile.
/// The native worker disables presolve for nonempty assumptions (upstream issue #5141).
pub const SUFFICIENT_ASSUMPTIONS_ENABLED: bool = true;
/// Pinned OR-Tools 9.15 `num_workers` upper bound.
pub const MAX_ORTOOLS_WORKER_THREADS: u32 = 10_000;
pub(crate) const APPLIED_PARAMETERS_HASH_ALGORITHM: &str = "sha256";
pub(crate) const APPLIED_PARAMETERS_DOMAIN: &[u8; 36] = b"eutheto.applied-solve-parameters.v1\0";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameClass {
    Handshake,
    SolveRequest,
    WorkerEvent,
}

impl FrameClass {
    #[must_use]
    pub const fn policy_name(self) -> &'static str {
        match self {
            Self::Handshake => "handshake",
            Self::SolveRequest => "solve_request",
            Self::WorkerEvent => "worker_event",
        }
    }
}

const PROTOCOL_IDENTITY: &str = "eutheto.solver-worker";
const PROTOCOL_PACKAGE: &str = "eutheto.worker.v1";
const UNKNOWN_MAJOR_ACTION: &str = "typed_handshake_error_then_close";
const HANDSHAKE_ROUTES: &[&str] = &[
    "ParentFrame.handshake_request",
    "WorkerFrame.handshake_response",
];
const SOLVE_REQUEST_ROUTES: &[&str] = &["ParentFrame.solve_request"];
const WORKER_EVENT_ROUTES: &[&str] = &[
    "WorkerFrame.started",
    "WorkerFrame.progress",
    "WorkerFrame.incumbent",
    "WorkerFrame.finished",
    "WorkerFrame.error",
];

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProtocolPolicy {
    applied_parameters_hash: AppliedParametersHashPolicy,
    protocol: String,
    compatibility: CompatibilityPolicy,
    package: String,
    version: ProtocolVersion,
    frame_classes: FrameClasses,
    framing: FramingPolicy,
    limits: GlobalLimits,
    #[serde(default)]
    field_limits: BTreeMap<String, FieldLimit>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct AppliedParametersHashPolicy {
    algorithm: String,
    domain_separator: String,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct ProtocolVersion {
    major: u32,
    minor: u32,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct CompatibilityPolicy {
    accepted_protocol_majors: Vec<u32>,
    unknown_major_action: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct FrameClasses {
    handshake: FrameClassPolicy,
    solve_request: FrameClassPolicy,
    worker_event: FrameClassPolicy,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct FrameClassPolicy {
    max_payload_bytes: usize,
    routes: Vec<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct GlobalLimits {
    max_nesting_depth: usize,
    max_repeated_field_items: usize,
    max_worker_threads: u32,
    max_string_bytes: usize,
    total_session_bytes: usize,
    frames_per_session: usize,
    events_per_session: usize,
    events_per_second: usize,
    max_stderr_bytes: usize,
}
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct FramingPolicy {
    length_prefix_bytes: usize,
    length_prefix_order: String,
    min_payload_bytes: usize,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FieldLimit {
    #[serde(default)]
    max_bytes: Option<usize>,
    #[serde(default)]
    max_count: Option<usize>,
}

impl ProtocolPolicy {
    /// Parses and validates one protocol-policy document.
    ///
    /// # Errors
    ///
    /// Returns a policy fault when JSON decoding fails or any required ceiling,
    /// route, field limit, package, or protocol identity is empty or zero.
    pub fn parse(source: &str) -> Result<Self, ProtocolFault> {
        let policy: Self = serde_json::from_str(source)
            .map_err(|error| ProtocolFault::Policy(error.to_string()))?;
        policy.validate()?;
        Ok(policy)
    }
    #[must_use]
    pub const fn protocol_major(&self) -> u32 {
        self.version.major
    }

    #[must_use]
    pub const fn protocol_minor(&self) -> u32 {
        self.version.minor
    }

    #[must_use]
    pub fn accepts_protocol_major(&self, major: u32) -> bool {
        self.compatibility.accepted_protocol_majors.contains(&major)
    }

    #[must_use]
    pub const fn max_nesting_depth(&self) -> usize {
        self.limits.max_nesting_depth
    }

    #[must_use]
    pub const fn max_repeated_field_items(&self) -> usize {
        self.limits.max_repeated_field_items
    }

    #[must_use]
    pub const fn max_worker_threads(&self) -> u32 {
        self.limits.max_worker_threads
    }

    #[must_use]
    pub const fn max_string_bytes(&self) -> usize {
        self.limits.max_string_bytes
    }

    #[must_use]
    pub const fn total_session_bytes(&self) -> usize {
        self.limits.total_session_bytes
    }

    #[must_use]
    pub const fn frames_per_session(&self) -> usize {
        self.limits.frames_per_session
    }

    #[must_use]
    pub const fn events_per_session(&self) -> usize {
        self.limits.events_per_session
    }

    #[must_use]
    pub const fn events_per_second(&self) -> usize {
        self.limits.events_per_second
    }

    #[must_use]
    pub const fn max_stderr_bytes(&self) -> usize {
        self.limits.max_stderr_bytes
    }

    #[must_use]
    pub const fn length_prefix_bytes(&self) -> usize {
        self.framing.length_prefix_bytes
    }

    #[must_use]
    pub fn length_prefix_order(&self) -> &str {
        &self.framing.length_prefix_order
    }

    #[must_use]
    pub const fn min_payload_bytes(&self) -> usize {
        self.framing.min_payload_bytes
    }

    #[must_use]
    fn frame(&self, class: FrameClass) -> &FrameClassPolicy {
        match class {
            FrameClass::Handshake => &self.frame_classes.handshake,
            FrameClass::SolveRequest => &self.frame_classes.solve_request,
            FrameClass::WorkerEvent => &self.frame_classes.worker_event,
        }
    }

    #[must_use]
    pub fn frame_cap(&self, class: FrameClass) -> usize {
        self.frame(class).max_payload_bytes
    }

    #[must_use]
    pub fn field_limit(&self, fully_qualified_field: &str) -> FieldLimit {
        self.field_limits
            .get(fully_qualified_field)
            .copied()
            .unwrap_or_default()
    }
    pub(crate) fn field_limits(&self) -> impl Iterator<Item = (&str, FieldLimit)> {
        self.field_limits
            .iter()
            .map(|(name, limit)| (name.as_str(), *limit))
    }

    fn validate(&self) -> Result<(), ProtocolFault> {
        if self.applied_parameters_hash.algorithm != APPLIED_PARAMETERS_HASH_ALGORITHM
            || self.applied_parameters_hash.domain_separator.as_bytes() != APPLIED_PARAMETERS_DOMAIN
        {
            return Err(ProtocolFault::Policy(
                "applied-parameter hash policy does not match the runtime".to_owned(),
            ));
        }
        if self.package != PROTOCOL_PACKAGE || self.protocol != PROTOCOL_IDENTITY {
            return Err(ProtocolFault::Policy(
                "protocol or package identity does not match the runtime".to_owned(),
            ));
        }
        if self.compatibility.accepted_protocol_majors.is_empty()
            || !self
                .compatibility
                .accepted_protocol_majors
                .contains(&self.version.major)
            || self.compatibility.unknown_major_action != UNKNOWN_MAJOR_ACTION
        {
            return Err(ProtocolFault::Policy(
                "protocol-major compatibility policy is invalid".to_owned(),
            ));
        }
        let mut accepted_majors = BTreeSet::new();
        if self
            .compatibility
            .accepted_protocol_majors
            .iter()
            .any(|major| *major == 0 || !accepted_majors.insert(*major))
        {
            return Err(ProtocolFault::Policy(
                "accepted protocol majors must be nonzero and unique".to_owned(),
            ));
        }
        if self.framing.length_prefix_bytes != 4
            || self.framing.length_prefix_order != "big-endian"
            || self.framing.min_payload_bytes != 1
        {
            return Err(ProtocolFault::Policy(
                "framing must use a four-byte big-endian length and nonempty payloads".to_owned(),
            ));
        }
        for (name, class, expected_routes) in [
            ("handshake", &self.frame_classes.handshake, HANDSHAKE_ROUTES),
            (
                "solve_request",
                &self.frame_classes.solve_request,
                SOLVE_REQUEST_ROUTES,
            ),
            (
                "worker_event",
                &self.frame_classes.worker_event,
                WORKER_EVENT_ROUTES,
            ),
        ] {
            if class.max_payload_bytes == 0 {
                return Err(ProtocolFault::Policy(format!(
                    "frame class {name} must have a nonzero cap"
                )));
            }
            let actual = class
                .routes
                .iter()
                .map(String::as_str)
                .collect::<BTreeSet<_>>();
            let expected = expected_routes.iter().copied().collect::<BTreeSet<_>>();
            if class.routes.len() != expected_routes.len() || actual != expected {
                return Err(ProtocolFault::Policy(format!(
                    "frame class {name} routes do not match the implemented route set"
                )));
            }
        }
        let limits = self.limits;
        if limits.max_nesting_depth == 0
            || limits.max_repeated_field_items == 0
            || limits.max_string_bytes == 0
            || limits.total_session_bytes == 0
            || limits.frames_per_session == 0
            || limits.events_per_session == 0
            || limits.events_per_second == 0
            || limits.max_stderr_bytes == 0
            || limits.max_worker_threads != MAX_ORTOOLS_WORKER_THREADS
        {
            return Err(ProtocolFault::Policy(
                "all global limits must be nonzero".to_owned(),
            ));
        }
        for (field, limit) in &self.field_limits {
            if field.is_empty()
                || limit.max_bytes == Some(0)
                || limit.max_count == Some(0)
                || (limit.max_bytes.is_none() && limit.max_count.is_none())
            {
                return Err(ProtocolFault::Policy(format!(
                    "field limit {field:?} is empty or zero"
                )));
            }
        }
        Ok(())
    }
}
impl FieldLimit {
    #[must_use]
    pub const fn max_bytes(self) -> Option<usize> {
        self.max_bytes
    }

    #[must_use]
    pub const fn max_count(self) -> Option<usize> {
        self.max_count
    }
}

/// Returns the validated policy embedded in this crate.
///
/// # Errors
///
/// Returns the cached policy fault when the checked-in document is invalid.
pub fn checked_in_policy() -> Result<&'static ProtocolPolicy, ProtocolFault> {
    static POLICY: LazyLock<Result<ProtocolPolicy, ProtocolFault>> =
        LazyLock::new(|| ProtocolPolicy::parse(POLICY_JSON));
    match &*POLICY {
        Ok(policy) => Ok(policy),
        Err(error) => Err(error.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::{FrameClass, POLICY_JSON, ProtocolPolicy};
    use crate::ProtocolFault;

    #[test]
    fn policy_rejects_zero_caps() {
        let source = POLICY_JSON.replacen(
            "\"max_payload_bytes\": 1048576",
            "\"max_payload_bytes\": 0",
            1,
        );
        assert!(matches!(
            ProtocolPolicy::parse(&source),
            Err(ProtocolFault::Policy(message))
                if message == "frame class handshake must have a nonzero cap"
        ));
    }

    #[test]
    fn checked_policy_models_compatibility_and_framing() -> Result<(), ProtocolFault> {
        let policy = ProtocolPolicy::parse(POLICY_JSON)?;
        assert_eq!(policy.framing.length_prefix_bytes, 4);
        assert_eq!(policy.framing.length_prefix_order, "big-endian");
        assert_eq!(policy.framing.min_payload_bytes, 1);
        assert!(
            policy
                .compatibility
                .accepted_protocol_majors
                .contains(&policy.version.major)
        );
        Ok(())
    }

    #[test]
    fn policy_rejects_identity_compatibility_framing_and_unknown_keys() {
        for invalid in [
            POLICY_JSON.replacen("eutheto.solver-worker", "other", 1),
            POLICY_JSON.replacen(
                "\"length_prefix_bytes\": 4",
                "\"length_prefix_bytes\": 3",
                1,
            ),
            POLICY_JSON.replacen("typed_handshake_error_then_close", "silently_continue", 1),
            POLICY_JSON.replacen("\"minor\": 1", "\"minor\": 1, \"unexpected\": true", 1),
            POLICY_JSON.replacen("\"algorithm\": \"sha256\"", "\"algorithm\": \"sha512\"", 1),
            POLICY_JSON.replacen(
                "eutheto.applied-solve-parameters.v1",
                "eutheto.applied-solve-parameters.v2",
                1,
            ),
        ] {
            assert!(ProtocolPolicy::parse(&invalid).is_err());
        }
    }

    #[test]
    fn policy_rejects_missing_extra_and_typo_routes() {
        for invalid in [
            POLICY_JSON.replacen("        \"ParentFrame.handshake_request\",\n", "", 1),
            POLICY_JSON.replacen(
                "\"ParentFrame.handshake_request\",",
                "\"ParentFrame.handshake_request\",\n        \"Bogus.x\",",
                1,
            ),
            POLICY_JSON.replacen("ParentFrame.handshake_request", "Bogus.x", 1),
        ] {
            assert!(matches!(
                ProtocolPolicy::parse(&invalid),
                Err(ProtocolFault::Policy(message))
                    if message == "frame class handshake routes do not match the implemented route set"
            ));
        }
    }

    #[test]
    fn frame_class_names_are_stable_policy_keys() {
        assert_eq!(FrameClass::Handshake.policy_name(), "handshake");
        assert_eq!(FrameClass::SolveRequest.policy_name(), "solve_request");
        assert_eq!(FrameClass::WorkerEvent.policy_name(), "worker_event");
    }
}
