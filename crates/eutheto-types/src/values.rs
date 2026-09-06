//! Checked scalar and time-zone value objects.

use jiff::Timestamp;
use jiff::civil::DateTime;
use jiff::tz::{AmbiguousOffset, TimeZone};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::str::FromStr;

/// Error produced by checked integer value objects.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum CheckedValueError {
    /// A value that must be non-negative was negative.
    Negative,
    /// An arithmetic operation exceeded the `i64` range.
    Overflow,
}

impl fmt::Display for CheckedValueError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Negative => "value must not be negative",
            Self::Overflow => "checked arithmetic overflow",
        })
    }
}

impl std::error::Error for CheckedValueError {}

macro_rules! checked_non_negative {
    ($name:ident, $description:literal) => {
        #[doc = $description]
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
        #[serde(transparent)]
        pub struct $name(i64);

        impl $name {
            /// Zero in this unit.
            pub const ZERO: Self = Self(0);

            /// Creates a non-negative value.
            ///
            /// # Errors
            ///
            /// Returns [`CheckedValueError::Negative`] when `value` is negative.
            pub const fn new(value: i64) -> Result<Self, CheckedValueError> {
                if value < 0 {
                    Err(CheckedValueError::Negative)
                } else {
                    Ok(Self(value))
                }
            }

            /// Returns the underlying integer.
            #[must_use]
            pub const fn value(self) -> i64 {
                self.0
            }

            /// Adds two values without wrapping.
            ///
            /// # Errors
            ///
            /// Returns [`CheckedValueError::Overflow`] when the sum exceeds `i64`.
            pub const fn checked_add(self, other: Self) -> Result<Self, CheckedValueError> {
                match self.0.checked_add(other.0) {
                    Some(value) => Ok(Self(value)),
                    None => Err(CheckedValueError::Overflow),
                }
            }

            /// Subtracts two values without wrapping or producing a negative value.
            ///
            /// # Errors
            ///
            /// Returns a typed error when the result is negative or outside `i64`.
            pub const fn checked_sub(self, other: Self) -> Result<Self, CheckedValueError> {
                match self.0.checked_sub(other.0) {
                    Some(value) if value >= 0 => Ok(Self(value)),
                    Some(_) => Err(CheckedValueError::Negative),
                    None => Err(CheckedValueError::Overflow),
                }
            }

            /// Multiplies by a non-negative factor without wrapping.
            ///
            /// # Errors
            ///
            /// Returns a typed error for a negative factor or an `i64` overflow.
            pub const fn checked_mul(self, factor: i64) -> Result<Self, CheckedValueError> {
                if factor < 0 {
                    return Err(CheckedValueError::Negative);
                }
                match self.0.checked_mul(factor) {
                    Some(value) => Ok(Self(value)),
                    None => Err(CheckedValueError::Overflow),
                }
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let value = i64::deserialize(deserializer)?;
                Self::new(value).map_err(serde::de::Error::custom)
            }
        }
    };
}

checked_non_negative!(Minutes, "A checked whole-minute duration.");
checked_non_negative!(Millimeters, "A checked whole-millimeter distance.");
checked_non_negative!(Penalty, "A checked non-negative objective penalty.");
checked_non_negative!(Capacity, "A checked non-negative capacity.");

/// Largest revision exactly representable by every Phase-01 JSON/TypeScript client.
pub const REVISION_MAX_V1: u64 = 9_007_199_254_740_991;

/// Monotonic scenario revision used for optimistic concurrency.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct Revision(u64);

impl Revision {
    /// Initial revision of a new scenario.
    pub const INITIAL: Self = Self(0);

    /// Creates a revision and asserts the version-1 portable numeric invariant.
    ///
    /// Use [`Self::try_new`] when the value is not already trusted.
    ///
    /// # Panics
    ///
    /// Panics when `value` exceeds [`REVISION_MAX_V1`]. Use [`Self::try_new`]
    /// for untrusted, persisted, or computed values.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        assert!(
            value <= REVISION_MAX_V1,
            "revision exceeds the version-1 portable numeric bound"
        );
        Self(value)
    }

    /// Creates a revision from an untrusted or persisted value.
    ///
    /// # Errors
    ///
    /// Returns [`CheckedValueError::Overflow`] above [`REVISION_MAX_V1`].
    pub const fn try_new(value: u64) -> Result<Self, CheckedValueError> {
        if value > REVISION_MAX_V1 {
            Err(CheckedValueError::Overflow)
        } else {
            Ok(Self(value))
        }
    }

    /// Returns the underlying revision number.
    #[must_use]
    pub const fn value(self) -> u64 {
        self.0
    }

    /// Advances this revision without exceeding the version-1 client bound.
    ///
    /// # Errors
    ///
    /// Returns [`CheckedValueError::Overflow`] at [`REVISION_MAX_V1`].
    pub const fn checked_next(self) -> Result<Self, CheckedValueError> {
        self.checked_add(1)
    }

    /// Adds a revision delta without exceeding the version-1 client bound.
    ///
    /// # Errors
    ///
    /// Returns [`CheckedValueError::Overflow`] when the sum exceeds
    /// [`REVISION_MAX_V1`].
    pub const fn checked_add(self, delta: u64) -> Result<Self, CheckedValueError> {
        match self.0.checked_add(delta) {
            Some(value) if value <= REVISION_MAX_V1 => Ok(Self(value)),
            Some(_) | None => Err(CheckedValueError::Overflow),
        }
    }
}

impl<'de> Deserialize<'de> for Revision {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = u64::deserialize(deserializer)?;
        Self::try_new(value).map_err(serde::de::Error::custom)
    }
}

/// Validated RFC 3339 timestamp serialized as a string.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Rfc3339Timestamp(Timestamp);

impl Rfc3339Timestamp {
    /// Parses a timestamp with an explicit UTC offset.
    ///
    /// # Errors
    ///
    /// Returns a Jiff parse error for an invalid or offset-free timestamp.
    pub fn parse(value: &str) -> Result<Self, jiff::Error> {
        value.parse().map(Self)
    }

    /// Wraps a Jiff timestamp.
    #[must_use]
    pub const fn from_timestamp(value: Timestamp) -> Self {
        Self(value)
    }

    /// Returns the precise instant.
    #[must_use]
    pub const fn as_timestamp(self) -> Timestamp {
        self.0
    }
}

impl fmt::Display for Rfc3339Timestamp {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for Rfc3339Timestamp {
    type Err = jiff::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

impl Serialize for Rfc3339Timestamp {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Rfc3339Timestamp {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).map_err(serde::de::Error::custom)
    }
}

/// Clock interface used wherever wall time enters deterministic core logic.
pub trait Clock: Send + Sync {
    /// Returns the current UTC instant.
    fn now(&self) -> Rfc3339Timestamp;
}

/// Clock backed by the operating system.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Rfc3339Timestamp {
        Rfc3339Timestamp(Timestamp::now())
    }
}

/// Immutable clock for deterministic tests and replay.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FixedClock {
    now: Rfc3339Timestamp,
}

impl FixedClock {
    /// Creates a clock fixed at `now`.
    #[must_use]
    pub const fn new(now: Rfc3339Timestamp) -> Self {
        Self { now }
    }
}

impl Clock for FixedClock {
    fn now(&self) -> Rfc3339Timestamp {
        self.now
    }
}

/// Validated IANA time-zone identifier.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct IanaTimeZone(String);

impl IanaTimeZone {
    /// Validates an IANA identifier against Jiff's bundled database.
    ///
    /// # Errors
    ///
    /// Returns a Jiff error when the identifier has no bundled time-zone entry.
    pub fn parse(value: &str) -> Result<Self, jiff::Error> {
        TimeZone::get(value).map(|_| Self(value.to_owned()))
    }

    /// Returns the IANA identifier.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn time_zone(&self) -> Result<TimeZone, jiff::Error> {
        TimeZone::get(&self.0)
    }
}

impl fmt::Display for IanaTimeZone {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl FromStr for IanaTimeZone {
    type Err = jiff::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

impl Serialize for IanaTimeZone {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for IanaTimeZone {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).map_err(serde::de::Error::custom)
    }
}

/// Error returned for an invalid bounded locale tag.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LocaleError;

impl fmt::Display for LocaleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("locale must be a bounded hyphen-separated language tag")
    }
}

impl std::error::Error for LocaleError {}

/// Explicit locale tag used for display and pack behavior.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct LocaleTag(String);

impl LocaleTag {
    /// Validates a compact BCP-47-shaped locale tag.
    ///
    /// # Errors
    ///
    /// Returns [`LocaleError`] for empty, oversized, or malformed tags.
    pub fn parse(value: &str) -> Result<Self, LocaleError> {
        let valid = !value.is_empty()
            && value.len() <= 64
            && value.split('-').all(|part| {
                !part.is_empty()
                    && part.len() <= 8
                    && part.bytes().all(|byte| byte.is_ascii_alphanumeric())
            });
        if valid {
            Ok(Self(value.to_owned()))
        } else {
            Err(LocaleError)
        }
    }

    /// Returns the locale tag.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for LocaleTag {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl FromStr for LocaleTag {
    type Err = LocaleError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

impl Serialize for LocaleTag {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for LocaleTag {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).map_err(serde::de::Error::custom)
    }
}

/// Local civil date and time with no implicit zone.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct LocalWallTime(DateTime);

impl LocalWallTime {
    /// Parses an ISO civil date-time.
    ///
    /// # Errors
    ///
    /// Returns a Jiff parse error for an invalid local date-time.
    pub fn parse(value: &str) -> Result<Self, jiff::Error> {
        value.parse().map(Self)
    }

    /// Wraps an already valid Jiff civil date-time without formatting or reparsing.
    #[must_use]
    pub const fn from_datetime(value: DateTime) -> Self {
        Self(value)
    }

    /// Returns the Jiff civil date-time.
    #[must_use]
    pub const fn as_datetime(self) -> DateTime {
        self.0
    }
}

impl fmt::Display for LocalWallTime {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for LocalWallTime {
    type Err = jiff::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

impl Serialize for LocalWallTime {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for LocalWallTime {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).map_err(serde::de::Error::custom)
    }
}

/// Policy for a nonexistent local time during a forward DST transition.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum GapPolicy {
    /// Return a typed resolution failure.
    Reject,
    /// Move by the transition gap to the corresponding valid later wall time.
    MoveForward,
    /// Require the owning domain pack to resolve the value explicitly.
    PackDefined,
}

/// Policy for a repeated local time during a backward DST transition.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum OverlapPolicy {
    /// Select the first occurrence of the local time.
    Earlier,
    /// Select the second occurrence of the local time.
    Later,
    /// Reject the ambiguous local time.
    Reject,
}

/// Kind of local-time resolution failure safe to return through APIs.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TimeResolutionFailureKind {
    /// The local time falls in a DST gap.
    Gap,
    /// The local time falls in a DST overlap.
    Overlap,
    /// Pack-owned gap resolution is required.
    PackResolutionRequired,
    /// The time zone database rejected the operation.
    InvalidTimeZone,
}

/// User-safe local-time resolution failure.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TimeResolutionError {
    /// Stable failure classification.
    pub kind: TimeResolutionFailureKind,
}

impl fmt::Display for TimeResolutionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self.kind {
            TimeResolutionFailureKind::Gap => "local time does not exist in the selected time zone",
            TimeResolutionFailureKind::Overlap => {
                "local time occurs twice in the selected time zone"
            }
            TimeResolutionFailureKind::PackResolutionRequired => {
                "local time requires explicit domain-pack resolution"
            }
            TimeResolutionFailureKind::InvalidTimeZone => "time zone could not be resolved",
        })
    }
}

impl std::error::Error for TimeResolutionError {}

/// Deterministically resolved local wall time.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResolvedLocalTime {
    /// Resolved precise instant.
    pub instant: Rfc3339Timestamp,
    /// Original local intent.
    pub local: LocalWallTime,
    /// Selected UTC offset in seconds.
    pub offset_seconds: i32,
}

/// Resolves a local wall time using explicit gap and overlap policies.
///
/// # Errors
///
/// Returns [`TimeResolutionError`] when policy rejects an ambiguity, pack
/// resolution is required, or the time-zone operation fails.
pub fn resolve_local_time(
    local: LocalWallTime,
    zone: &IanaTimeZone,
    gap_policy: GapPolicy,
    overlap_policy: OverlapPolicy,
) -> Result<ResolvedLocalTime, TimeResolutionError> {
    let time_zone = zone.time_zone().map_err(|_| TimeResolutionError {
        kind: TimeResolutionFailureKind::InvalidTimeZone,
    })?;
    let ambiguous = time_zone.to_ambiguous_zoned(local.as_datetime());
    let offset = ambiguous.offset();
    let zoned = match offset {
        AmbiguousOffset::Unambiguous { .. } => ambiguous.unambiguous(),
        AmbiguousOffset::Gap { .. } => match gap_policy {
            GapPolicy::Reject => {
                return Err(TimeResolutionError {
                    kind: TimeResolutionFailureKind::Gap,
                });
            }
            GapPolicy::MoveForward => ambiguous.compatible(),
            GapPolicy::PackDefined => {
                return Err(TimeResolutionError {
                    kind: TimeResolutionFailureKind::PackResolutionRequired,
                });
            }
        },
        AmbiguousOffset::Fold { .. } => match overlap_policy {
            OverlapPolicy::Earlier => ambiguous.earlier(),
            OverlapPolicy::Later => ambiguous.later(),
            OverlapPolicy::Reject => {
                return Err(TimeResolutionError {
                    kind: TimeResolutionFailureKind::Overlap,
                });
            }
        },
    }
    .map_err(|_| TimeResolutionError {
        kind: TimeResolutionFailureKind::InvalidTimeZone,
    })?;

    Ok(ResolvedLocalTime {
        instant: Rfc3339Timestamp::from_timestamp(zoned.timestamp()),
        local,
        offset_seconds: zoned.offset().seconds(),
    })
}

/// Error returned when a horizon is empty or reversed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HorizonError;

impl fmt::Display for HorizonError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("horizon end must be after its start")
    }
}

impl std::error::Error for HorizonError {}

/// Explicit half-open scenario horizon `[start, end)`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Horizon {
    /// Inclusive horizon start.
    pub start: Rfc3339Timestamp,
    /// Exclusive horizon end.
    pub end: Rfc3339Timestamp,
}

impl Horizon {
    /// Creates a non-empty, increasing horizon.
    ///
    /// # Errors
    ///
    /// Returns [`HorizonError`] when `end` is not strictly after `start`.
    pub fn new(start: Rfc3339Timestamp, end: Rfc3339Timestamp) -> Result<Self, HorizonError> {
        if start < end {
            Ok(Self { start, end })
        } else {
            Err(HorizonError)
        }
    }
}

impl<'de> Deserialize<'de> for Horizon {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct SerializedHorizon {
            start: Rfc3339Timestamp,
            end: Rfc3339Timestamp,
        }

        let value = SerializedHorizon::deserialize(deserializer)?;
        Self::new(value.start, value.end).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Clock, FixedClock, GapPolicy, IanaTimeZone, LocalWallTime, OverlapPolicy, REVISION_MAX_V1,
        Revision, Rfc3339Timestamp, TimeResolutionFailureKind, resolve_local_time,
    };

    #[test]
    fn revision_v1_bound_is_exact_at_json_boundaries() -> Result<(), Box<dyn std::error::Error>> {
        let maximum = Revision::try_new(REVISION_MAX_V1)?;
        assert_eq!(
            serde_json::to_string(&maximum)?,
            REVISION_MAX_V1.to_string()
        );
        assert_eq!(
            serde_json::from_str::<Revision>(&REVISION_MAX_V1.to_string())?,
            maximum
        );
        assert!(maximum.checked_next().is_err());
        assert!(Revision::try_new(REVISION_MAX_V1 + 1).is_err());
        assert!(std::panic::catch_unwind(|| Revision::new(REVISION_MAX_V1 + 1)).is_err());
        assert!(serde_json::from_str::<Revision>(&format!("{}", REVISION_MAX_V1 + 1)).is_err());
        Ok(())
    }

    #[test]
    fn fixed_clock_never_advances() -> Result<(), Box<dyn std::error::Error>> {
        let instant = Rfc3339Timestamp::parse("2026-08-28T23:00:00Z")?;
        let clock = FixedClock::new(instant);
        assert_eq!(clock.now(), instant);
        assert_eq!(clock.now(), instant);
        Ok(())
    }

    #[test]
    fn chicago_spring_gap_is_explicit() -> Result<(), Box<dyn std::error::Error>> {
        let zone = IanaTimeZone::parse("America/Chicago")?;
        let local = LocalWallTime::parse("2026-03-08T02:30:00")?;
        let rejected = resolve_local_time(local, &zone, GapPolicy::Reject, OverlapPolicy::Earlier);
        assert_eq!(
            rejected.map_err(|error| error.kind),
            Err(TimeResolutionFailureKind::Gap)
        );

        let moved =
            resolve_local_time(local, &zone, GapPolicy::MoveForward, OverlapPolicy::Earlier)?;
        assert_eq!(moved.instant.to_string(), "2026-03-08T08:30:00Z");
        Ok(())
    }

    #[test]
    fn explicit_scenario_zone_controls_resolution_independently_of_host_time_zone()
    -> Result<(), Box<dyn std::error::Error>> {
        let local = LocalWallTime::parse("2026-07-15T12:00:00")?;
        let chicago = resolve_local_time(
            local,
            &IanaTimeZone::parse("America/Chicago")?,
            GapPolicy::Reject,
            OverlapPolicy::Earlier,
        )?;
        let tokyo = resolve_local_time(
            local,
            &IanaTimeZone::parse("Asia/Tokyo")?,
            GapPolicy::Reject,
            OverlapPolicy::Earlier,
        )?;

        assert_eq!(chicago.instant.to_string(), "2026-07-15T17:00:00Z");
        assert_eq!(chicago.offset_seconds, -18_000);
        assert_eq!(tokyo.instant.to_string(), "2026-07-15T03:00:00Z");
        assert_eq!(tokyo.offset_seconds, 32_400);
        Ok(())
    }

    #[test]
    fn chicago_fall_fold_selects_each_instant() -> Result<(), Box<dyn std::error::Error>> {
        let zone = IanaTimeZone::parse("America/Chicago")?;
        let local = LocalWallTime::parse("2026-11-01T01:30:00")?;
        let earlier = resolve_local_time(local, &zone, GapPolicy::Reject, OverlapPolicy::Earlier)?;
        let later = resolve_local_time(local, &zone, GapPolicy::Reject, OverlapPolicy::Later)?;

        assert_eq!(earlier.instant.to_string(), "2026-11-01T06:30:00Z");
        assert_eq!(later.instant.to_string(), "2026-11-01T07:30:00Z");
        assert_eq!(earlier.offset_seconds, -18_000);
        assert_eq!(later.offset_seconds, -21_600);
        Ok(())
    }
}
