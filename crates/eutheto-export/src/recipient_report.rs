// SPDX-License-Identifier: Apache-2.0
//! Self-contained presentation bytes, never native acceptance or source custody.

use crate::{
    ExportError, PreparedPublication, PublicationFailpoint, prepare_atomic_with_control, sha256_hex,
};
use eutheto_types::{
    MAX_RECIPIENT_HTML_BYTES, MAX_RECIPIENT_PRESENTATION_COST, MAX_RECIPIENT_REPORT_BYTES,
    OperationControl, RecipientContentV1, RecipientLabelV1, RecipientRendererV1, RecipientReportV1,
};
use std::io::{self, Write};
use std::path::Path;

#[path = "generated/recipient_renderer.rs"]
mod renderer;

const JAVASCRIPT: &[u8] = include_bytes!("generated/recipient-renderer.js");
const CSS: &[u8] = include_bytes!("generated/recipient-renderer.css");

/// The renderer identity captured in every recipient preview.
#[must_use]
pub fn recipient_renderer() -> RecipientRendererV1 {
    RecipientRendererV1 {
        version: renderer::RENDERER_VERSION,
        javascript_sha256: renderer::JAVASCRIPT_SHA256.to_owned(),
        css_sha256: renderer::CSS_SHA256.to_owned(),
    }
}

/// Immutable encoded output. The application must separately admit its original source
/// and retain approval/publication custody; public hashes cannot provide either.
#[derive(Debug)]
pub struct EncodedRecipientHtml {
    bytes: Vec<u8>,
    canonical_model: Vec<u8>,
    model_sha256: String,
    html_sha256: String,
}

impl EncodedRecipientHtml {
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Exact approved model bytes, retained without reserializing the source report.
    #[must_use]
    pub fn canonical_model_bytes(&self) -> &[u8] {
        &self.canonical_model
    }

    #[must_use]
    pub fn model_sha256(&self) -> &str {
        &self.model_sha256
    }

    #[must_use]
    pub fn html_sha256(&self) -> &str {
        &self.html_sha256
    }

    /// Owned storage charged while a preview or create operation retains this output.
    #[must_use]
    pub fn retained_bytes(&self) -> usize {
        size_of::<Self>()
            + self.bytes.capacity()
            + self.canonical_model.capacity()
            + self.model_sha256.capacity()
            + self.html_sha256.capacity()
    }

    /// Stage these exact bytes without replacing a destination. The caller owns the
    /// final native revision/custody check and publication lease.
    ///
    /// # Errors
    /// Returns interruption or staging I/O/verification failures.
    pub fn prepare_destination(
        &self,
        destination: &Path,
        control: &OperationControl,
    ) -> Result<PreparedPublication, ExportError> {
        prepare_atomic_with_control(
            destination,
            &self.bytes,
            PublicationFailpoint::None,
            &mut || control.check().map_err(ExportError::from),
        )
    }
}

/// Encode one intrinsically valid, already privacy-filtered report with the exact
/// compiled renderer. This function does not authenticate the report's claims.
///
/// # Errors
/// Rejects invalid models, renderer mismatches, byte overflow and interruption.
pub fn encode_recipient_html(
    report: &RecipientReportV1,
    control: &OperationControl,
) -> Result<EncodedRecipientHtml, ExportError> {
    let mut model = ReportWriter::new(MAX_RECIPIENT_REPORT_BYTES, control);
    if let Err(error) = serde_json::to_writer(&mut model, report) {
        return Err(model.failure.take().unwrap_or(ExportError::Json(error)));
    }
    // Bound encoded input before walking potentially long textual fields or indexes.
    control.check().map_err(ExportError::from)?;
    report
        .validate()
        .map_err(|error| ExportError::InvalidModel(error.to_string()))?;
    if report.renderer.version != renderer::RENDERER_VERSION
        || report.renderer.javascript_sha256 != renderer::JAVASCRIPT_SHA256
        || report.renderer.css_sha256 != renderer::CSS_SHA256
    {
        return Err(ExportError::InvalidModel(
            "recipient renderer identity differs".to_owned(),
        ));
    }
    check_presentation_cost(report, model.bytes.len())?;
    let model_sha256 = sha256_hex(&model.bytes);
    let mut html = ReportWriter::new(MAX_RECIPIENT_HTML_BYTES, control);
    let result = (|| -> io::Result<()> {
        html.write_all(b"<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; base-uri 'none'; form-action 'none'; connect-src 'none'; object-src 'none'; script-src '")?;
        html.write_all(renderer::JAVASCRIPT_CSP.as_bytes())?;
        html.write_all(b"'; style-src '")?;
        html.write_all(renderer::CSS_CSP.as_bytes())?;
        html.write_all(b"'\"><!-- Project renderer NOTICE\n")?;
        html.write_all(include_bytes!("../../../NOTICE"))?;
        html.write_all(b"\nProject renderer LICENSE\n")?;
        html.write_all(include_bytes!("../../../LICENSE"))?;
        html.write_all(b"\n--><title>Eutheto shared plan</title><style>")?;
        html.write_all(CSS)?;
        html.write_all(b"</style></head><body><main id=\"recipient-report\"></main><noscript>This shared plan requires JavaScript for its local, offline tables and filters.</noscript><script id=\"recipient-data\" type=\"application/json\">")?;
        // Escape the HTML raw-text boundary, not merely JavaScript string syntax.
        // serde_json emits UTF-8; iterate bytes so ordinary spans need no extra copy.
        let mut start = 0;
        let bytes = &model.bytes;
        let mut index = 0;
        while index < bytes.len() {
            let (escaped, width): (&[u8], usize) = match bytes[index] {
                b'<' => (b"\\u003c", 1),
                b'>' => (b"\\u003e", 1),
                b'&' => (b"\\u0026", 1),
                0xe2 if bytes.get(index..index + 3) == Some(&[0xe2, 0x80, 0xa8]) => (b"\\u2028", 3),
                0xe2 if bytes.get(index..index + 3) == Some(&[0xe2, 0x80, 0xa9]) => (b"\\u2029", 3),
                _ => {
                    if index % 4096 == 0 {
                        html.check()?;
                    }
                    index += 1;
                    continue;
                }
            };
            html.write_all(&bytes[start..index])?;
            html.write_all(escaped)?;
            index += width;
            start = index;
        }
        html.write_all(&bytes[start..])?;
        html.write_all(b"</script><script>")?;
        html.write_all(JAVASCRIPT)?;
        html.write_all(b"</script></body></html>")?;
        html.check()
    })();
    if let Err(error) = result {
        return Err(html.failure.take().unwrap_or(ExportError::Io(error)));
    }
    let html_sha256 = sha256_hex(&html.bytes);
    control.check().map_err(ExportError::from)?;
    Ok(EncodedRecipientHtml {
        bytes: html.bytes,
        canonical_model: model.bytes,
        model_sha256,
        html_sha256,
    })
}

// A conservative census avoids constructing reference-expanded strings merely to
// discover they are too large. Keep arithmetic aligned with the pure renderer.
fn check_presentation_cost(
    report: &RecipientReportV1,
    encoded_bytes: usize,
) -> Result<(), ExportError> {
    let RecipientContentV1::Workforce(schedule) = &report.content;
    let mut rows = schedule.people.len()
        + schedule.assignment_types.len()
        + schedule.locations.len()
        + schedule.shifts.len()
        + schedule.assignments.len();
    let mut maximum_text = report
        .plan_version
        .len()
        .max(report.time_zone.as_str().len())
        .max(64);
    for label in schedule
        .people
        .iter()
        .map(|person| &person.label)
        .chain(schedule.assignment_types.iter().map(|entity| &entity.label))
        .chain(schedule.locations.iter().map(|entity| &entity.label))
    {
        let (RecipientLabelV1::Generated(text) | RecipientLabelV1::Authored(text)) = label;
        maximum_text = maximum_text.max(text.len());
    }
    if let Some(staffing) = &schedule.staffing {
        rows += staffing.len();
    }
    if let Some(changes) = &schedule.changes {
        rows += changes.len();
        for comparison in changes {
            rows += comparison.rows.len();
        }
    }
    if let Some(groups) = &schedule.fairness_actuals {
        rows += groups.len();
        for group in groups {
            rows += group.people.len() + group.qualifying_shifts.len() + group.periods.len();
            for period in &group.periods {
                rows += period.actuals.len();
            }
        }
    }
    // Intrinsic validation already bounds the census and typed temporal strings.
    let cost = maximum_text
        .checked_mul(16)
        .and_then(|value| value.checked_add(4096))
        .and_then(|value| value.checked_mul(rows + 1))
        .and_then(|value| value.checked_add(encoded_bytes * 4 + 65_536));
    if cost.is_none_or(|value| value > MAX_RECIPIENT_PRESENTATION_COST) {
        return Err(ExportError::ResourceLimitExceeded);
    }
    Ok(())
}

/// The same prospective bound applies to serialization and escaped HTML allocation.
struct ReportWriter<'a> {
    bytes: Vec<u8>,
    maximum: usize,
    control: &'a OperationControl,
    failure: Option<ExportError>,
}

impl<'a> ReportWriter<'a> {
    fn new(maximum: usize, control: &'a OperationControl) -> Self {
        Self {
            bytes: Vec::new(),
            maximum,
            control,
            failure: None,
        }
    }

    fn check(&mut self) -> io::Result<()> {
        if let Err(interruption) = self.control.check() {
            self.failure = Some(interruption.into());
            return Err(io::Error::other("recipient export interrupted"));
        }
        Ok(())
    }
}

impl Write for ReportWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.check()?;
        let Some(required) = self
            .bytes
            .len()
            .checked_add(bytes.len())
            .filter(|length| *length <= self.maximum)
        else {
            self.failure = Some(ExportError::ResourceLimitExceeded);
            return Err(io::Error::other("recipient export byte limit exceeded"));
        };
        if required > self.bytes.capacity() {
            let capacity = required
                .max(self.bytes.capacity().saturating_mul(2))
                .min(self.maximum);
            if self
                .bytes
                .try_reserve_exact(capacity - self.bytes.len())
                .is_err()
            {
                self.failure = Some(ExportError::ResourceLimitExceeded);
                return Err(io::Error::other("recipient export allocation refused"));
            }
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.check()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eutheto_types::{CancellationToken, RecipientLabelV1};

    #[test]
    fn recipient_deadline_is_not_user_cancellation() -> Result<(), Box<dyn std::error::Error>> {
        use eutheto_types::{DurationMillis, ParentSolveBudget, SystemMonotonicClock};
        let budget = ParentSolveBudget::new(
            DurationMillis::ZERO,
            std::sync::Arc::new(SystemMonotonicClock::new()),
            CancellationToken::new(),
        )?;
        let expired = OperationControl::Solve(budget.phase_view());
        assert!(matches!(
            encode_recipient_html(&report()?, &expired),
            Err(ExportError::DeadlineExceeded)
        ));
        let directory = tempfile::tempdir()?;
        let destination = directory.path().join("report.html");
        let live = OperationControl::Cancellation(CancellationToken::new());
        let encoded = encode_recipient_html(&report()?, &live)?;
        assert!(matches!(
            encoded.prepare_destination(&destination, &expired),
            Err(ExportError::DeadlineExceeded)
        ));
        let prepared = encoded.prepare_destination(&destination, &live)?;
        assert!(matches!(
            prepared.publish_controlled(&expired),
            Err(ExportError::DeadlineExceeded)
        ));
        assert!(matches!(
            crate::encode_json_controlled(&serde_json::json!({"value": 1}), &expired),
            Err(ExportError::DeadlineExceeded)
        ));
        assert!(matches!(
            crate::prepare_json_atomic_controlled(&destination, &serde_json::json!({}), &expired),
            Err(ExportError::DeadlineExceeded)
        ));
        assert!(matches!(
            crate::prepare_text_atomic_controlled(&destination, "value\n", &expired),
            Err(ExportError::DeadlineExceeded)
        ));
        assert!(!destination.exists());
        assert_eq!(std::fs::read_dir(directory.path())?.count(), 0);
        Ok(())
    }

    fn report() -> Result<RecipientReportV1, serde_json::Error> {
        serde_json::from_value(serde_json::json!({
            "format": "eutheto/share-result", "schemaVersion": 1,
            "profile": "participant-friendly", "generatedAt": "2024-03-10T08:00:00Z",
            "applicationVersion": "test", "renderer": recipient_renderer(),
            "title": {"kind": "authored", "text": "</script><script>globalThis.injected=true</script>&\u{2028}\u{2029}"},
            "planVersion": "Test plan", "locale": "en-US", "timeZone": "UTC",
            "horizon": {"start": "2024-03-10T00:00:00Z", "end": "2024-03-11T00:00:00Z"},
            "source": {
                "scenarioId": "018f0f04-0000-7000-8000-000000000001", "scenarioRevision": 1,
                "solutionId": "018f0f04-0000-7000-8000-000000000002", "resultSchemaVersion": 1,
                "acceptedResultChecksum": "0".repeat(64), "verificationReportChecksum": "1".repeat(64),
                "sourceDocumentHash": "2".repeat(64), "origin": "verifiedFile",
                "observedCurrentRevision": null, "verification": "verifiedFeasible",
                "solver": {"backendId": "test", "recordedVersion": null}
            },
            "privacy": {"includeAuthoredLabels": true},
            "content": {"kind": "workforce", "schedule": {
                "people": [], "assignmentTypes": [], "locations": [], "shifts": [],
                "assignments": [], "staffing": null, "changes": null, "fairnessActuals": null
            }}
        }))
    }

    #[test]
    fn recipient_html_preserves_inert_text_and_publishes_without_clobber()
    -> Result<(), Box<dyn std::error::Error>> {
        let control = OperationControl::Cancellation(CancellationToken::new());
        let report = report()?;
        let encoded = encode_recipient_html(&report, &control)?;
        let html = std::str::from_utf8(encoded.as_bytes())?;
        let payload = html
            .split_once("<script id=\"recipient-data\" type=\"application/json\">")
            .ok_or("missing inert payload")?
            .1
            .split_once("</script>")
            .ok_or("missing payload end")?
            .0;
        assert!(!payload.contains(['<', '>', '&', '\u{2028}', '\u{2029}']));
        assert_eq!(serde_json::from_str::<RecipientReportV1>(payload)?, report);
        assert_eq!(
            serde_json::from_slice::<RecipientReportV1>(encoded.canonical_model_bytes())?,
            report,
        );
        assert_eq!(
            sha256_hex(encoded.canonical_model_bytes()),
            encoded.model_sha256(),
        );
        let directory = tempfile::tempdir()?;
        let destination = directory.path().join("plan.html");
        encoded
            .prepare_destination(&destination, &control)?
            .publish_controlled(&control)?;
        assert_eq!(std::fs::read(&destination)?, encoded.as_bytes());
        assert!(matches!(
            encoded
                .prepare_destination(&destination, &control)?
                .publish_controlled(&control),
            Err(ExportError::DestinationExists(_))
        ));
        assert_eq!(std::fs::read(&destination)?, encoded.as_bytes());
        assert_eq!(std::fs::read_dir(directory.path())?.count(), 1);
        Ok(())
    }

    #[test]
    fn recipient_html_refuses_unapproved_or_unbounded_models()
    -> Result<(), Box<dyn std::error::Error>> {
        let token = CancellationToken::new();
        let control = OperationControl::Cancellation(token.clone());
        let mut report = report()?;
        report.privacy.include_authored_labels = false;
        assert!(matches!(
            encode_recipient_html(&report, &control),
            Err(ExportError::InvalidModel(_))
        ));
        report.privacy.include_authored_labels = true;
        report.renderer.version += 1;
        assert!(matches!(
            encode_recipient_html(&report, &control),
            Err(ExportError::InvalidModel(_))
        ));
        report.renderer = recipient_renderer();
        report.title = RecipientLabelV1::Authored("x".repeat(MAX_RECIPIENT_REPORT_BYTES));
        assert!(matches!(
            encode_recipient_html(&report, &control),
            Err(ExportError::ResourceLimitExceeded)
        ));
        token.cancel();
        assert!(matches!(
            encode_recipient_html(&report, &control),
            Err(ExportError::Cancelled)
        ));
        Ok(())
    }

    #[test]
    fn recipient_html_bounds_repeated_labels_before_presentation()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut value = serde_json::to_value(report()?)?;
        let endpoint = |hour: &str| {
            serde_json::json!({
                "resolved": {"instant": format!("2024-03-10T{hour}:00:00Z"),
                    "local": format!("2024-03-10T{hour}:00:00"), "offsetSeconds": 0},
                "actual": format!("2024-03-10T{hour}:00:00")
            })
        };
        let schedule = &mut value["content"]["schedule"];
        schedule["assignmentTypes"] = serde_json::json!([{
            "entityId": "018f0f04-0000-7000-8000-000000000003",
            "label": {"kind": "authored", "text": "x".repeat(81_920)}
        }]);
        schedule["shifts"] = (0..49)
            .map(|index| {
                serde_json::json!({
                    "shiftId": format!("018f0f04-0000-7000-8000-{:012}", index + 100),
                    "assignmentTypeId": "018f0f04-0000-7000-8000-000000000003", "locationId": null,
                    "interval": {"startsAt": endpoint("08"), "endsAt": endpoint("09")},
                    "reportingDate": "2024-03-10"
                })
            })
            .collect();
        let mut report: RecipientReportV1 = serde_json::from_value(value)?;
        let control = OperationControl::Cancellation(CancellationToken::new());
        assert!(serde_json::to_vec(&report)?.len() < MAX_RECIPIENT_REPORT_BYTES);
        assert!(matches!(
            encode_recipient_html(&report, &control),
            Err(ExportError::ResourceLimitExceeded)
        ));
        let RecipientContentV1::Workforce(schedule) = &mut report.content;
        schedule.shifts.pop();
        let encoded = encode_recipient_html(&report, &control)?;
        assert_eq!(
            encoded.model_sha256(),
            sha256_hex(&serde_json::to_vec(&report)?)
        );
        Ok(())
    }
}
