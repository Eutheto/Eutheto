use super::contracts::{
    CURSOR_POSITION_BYTES, MAX_PAGE, MAX_PROJECTION_VISITS, SetupPageV1, WorkforcePositionV1,
    WorkforceSetupResultV1, WorkforceSetupViewDataV1,
};
use eutheto_domain_api::{
    ContractJsonLimits, DomainPackError, DomainSetupQueryV1, SetupContinuationV1, SetupViewContext,
    bounded_json_size, validate_contract_value,
};
use eutheto_types::{OperationControl, ScenarioId};
use serde::{Deserialize, Serialize};
use serde_json::json;

pub(super) type Result<T, E = DomainPackError> = std::result::Result<T, E>;

pub(super) fn invalid(path: &str, message: &str) -> DomainPackError {
    DomainPackError::InvalidPayload {
        path: path.to_owned(),
        message: message.to_owned(),
    }
}

pub(super) struct ProjectionBudget<'a> {
    control: &'a OperationControl,
    visits: u32,
}

impl<'a> ProjectionBudget<'a> {
    pub fn new(control: &'a OperationControl) -> Self {
        Self { control, visits: 0 }
    }

    pub fn control(&self) -> &'a OperationControl {
        self.control
    }

    pub fn visit(&mut self) -> Result<()> {
        self.control.check()?;
        self.visits = self
            .visits
            .checked_add(1)
            .filter(|visits| *visits <= MAX_PROJECTION_VISITS)
            .ok_or(DomainPackError::ResourceLimitExceeded)?;
        Ok(())
    }
}

pub(super) fn continuation_position(
    query: &DomainSetupQueryV1,
    scenario_id: ScenarioId,
    context: SetupViewContext,
) -> Result<Option<WorkforcePositionV1>> {
    let Some(cursor) = &query.continuation else {
        return Ok(None);
    };
    if cursor.schema_version != 1
        || cursor.scenario_id != scenario_id
        || cursor.revision != context.revision
        || cursor.query_fingerprint != context.query_fingerprint
    {
        return Err(invalid(
            "/query/continuation",
            "continuation does not match this input",
        ));
    }
    validate_contract_value(
        &json!({}),
        &cursor.position,
        ContractJsonLimits {
            max_serialized_bytes: CURSOR_POSITION_BYTES,
            max_depth: 4,
            max_string_bytes: CURSOR_POSITION_BYTES,
            max_collection_items: 16,
        },
    )?;
    WorkforcePositionV1::deserialize(&cursor.position)
        .map(Some)
        .map_err(|_| {
            invalid(
                "/query/continuation/position",
                "invalid continuation position",
            )
        })
}

struct PageEntry<T> {
    row: T,
    bytes: usize,
    position: WorkforcePositionV1,
}

/// Counts every matching record but only projects the requested prefix. Final framing is
/// measured separately, so byte paging neither copies rows nor repeatedly serializes them.
pub(super) struct PageBuilder<T> {
    total: u32,
    remaining: u32,
    limit: usize,
    byte_limit: usize,
    row_bytes: usize,
    stopped: bool,
    entries: Vec<PageEntry<T>>,
}

impl<T: Serialize> PageBuilder<T> {
    pub fn new(limit: u16, byte_limit: usize) -> Result<Self> {
        if !(1..=MAX_PAGE).contains(&limit) {
            return Err(invalid(
                "/query/parameters/limit",
                "page limit is outside its allowed range",
            ));
        }
        Ok(Self {
            total: 0,
            remaining: 0,
            limit: usize::from(limit),
            byte_limit,
            row_bytes: 0,
            stopped: false,
            entries: Vec::new(),
        })
    }

    /// Call once per matching identity, including identities before the continuation.
    /// The projection closure is not called for rows outside the bounded candidate page.
    pub fn observe(
        &mut self,
        after_cursor: bool,
        project: impl FnOnce() -> Result<(T, WorkforcePositionV1)>,
    ) -> Result<()> {
        self.total = self
            .total
            .checked_add(1)
            .ok_or(DomainPackError::ResourceLimitExceeded)?;
        if !after_cursor {
            return Ok(());
        }
        self.remaining = self
            .remaining
            .checked_add(1)
            .ok_or(DomainPackError::ResourceLimitExceeded)?;
        if self.stopped || self.entries.len() == self.limit {
            return Ok(());
        }
        let (row, position) = project()?;
        let bytes = match bounded_json_size(&row, self.byte_limit) {
            Ok(bytes) => bytes,
            Err(_) if !self.entries.is_empty() => {
                self.stopped = true;
                return Ok(());
            }
            Err(_) => return Err(DomainPackError::ResourceLimitExceeded),
        };
        let combined = self
            .row_bytes
            .checked_add(bytes)
            .ok_or(DomainPackError::ResourceLimitExceeded)?;
        if combined > self.byte_limit && !self.entries.is_empty() {
            self.stopped = true;
            return Ok(());
        }
        self.row_bytes = combined;
        self.entries.push(PageEntry {
            row,
            bytes,
            position,
        });
        Ok(())
    }

    pub fn finish(
        mut self,
        scenario_id: ScenarioId,
        context: SetupViewContext,
        wrap: impl Fn(SetupPageV1<T>) -> WorkforceSetupViewDataV1,
    ) -> Result<WorkforceSetupViewDataV1> {
        loop {
            let count = u32::try_from(self.entries.len())
                .map_err(|_| DomainPackError::ResourceLimitExceeded)?;
            if count == 0 && self.remaining != 0 {
                return Err(DomainPackError::ResourceLimitExceeded);
            }
            let continuation = if count < self.remaining {
                let last = self
                    .entries
                    .last()
                    .ok_or(DomainPackError::ResourceLimitExceeded)?;
                Some(SetupContinuationV1 {
                    schema_version: 1,
                    scenario_id,
                    revision: context.revision,
                    query_fingerprint: context.query_fingerprint,
                    position: serde_json::to_value(last.position).map_err(|_| {
                        invalid("/query/continuation", "continuation serialization failed")
                    })?,
                })
            } else {
                None
            };
            let framing = WorkforceSetupResultV1 {
                schema_version: 1,
                result: wrap(SetupPageV1 {
                    total_items: self.total,
                    items: Vec::new(),
                    continuation: continuation.clone(),
                }),
            };
            let framing_bytes = bounded_json_size(&framing, self.byte_limit)
                .map_err(|_| DomainPackError::ResourceLimitExceeded)?;
            let bytes = framing_bytes
                .checked_add(self.row_bytes)
                .and_then(|bytes| bytes.checked_add(self.entries.len().saturating_sub(1)))
                .ok_or(DomainPackError::ResourceLimitExceeded)?;
            if bytes <= self.byte_limit {
                return Ok(wrap(SetupPageV1 {
                    total_items: self.total,
                    items: self.entries.into_iter().map(|entry| entry.row).collect(),
                    continuation,
                }));
            }
            let removed = self
                .entries
                .pop()
                .ok_or(DomainPackError::ResourceLimitExceeded)?;
            self.row_bytes = self
                .row_bytes
                .checked_sub(removed.bytes)
                .ok_or(DomainPackError::ResourceLimitExceeded)?;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::setup::contracts::CommandChangeV1;
    use eutheto_types::{Change, ChangeKind, Revision};
    use std::error::Error;

    #[test]
    fn byte_pages_preserve_whole_changes_and_count_cursor_framing()
    -> std::result::Result<(), Box<dyn Error>> {
        let scenario_id = "018f7b40-a000-7000-8000-000000000100".parse()?;
        let context = SetupViewContext {
            revision: Revision::INITIAL,
            query_fingerprint: [7; 32],
        };
        let rows = [
            CommandChangeV1 {
                ordinal: 0,
                change: Change {
                    kind: ChangeKind::Updated,
                    path: "/settings".to_owned(),
                    before: Some(json!({"value": "a".repeat(128)})),
                    after: Some(json!({"value": "b".repeat(128)})),
                },
            },
            CommandChangeV1 {
                ordinal: 1,
                change: Change {
                    kind: ChangeKind::Updated,
                    path: "/settings".to_owned(),
                    before: Some(json!({"value": "b".repeat(128)})),
                    after: Some(json!({"value": "c".repeat(128)})),
                },
            },
        ];
        let expected = WorkforceSetupResultV1 {
            schema_version: 1,
            result: WorkforceSetupViewDataV1::CommandChanges(SetupPageV1 {
                total_items: 2,
                items: vec![rows[0].clone()],
                continuation: Some(SetupContinuationV1 {
                    schema_version: 1,
                    scenario_id,
                    revision: context.revision,
                    query_fingerprint: context.query_fingerprint,
                    position: serde_json::to_value(WorkforcePositionV1::Ordinal {
                        next_ordinal: 1,
                    })?,
                }),
            }),
        };
        let limit = serde_json::to_vec(&expected)?.len();
        let build = |byte_limit, start| -> Result<WorkforceSetupViewDataV1> {
            let mut page = PageBuilder::new(2, byte_limit)?;
            for row in &rows {
                page.observe(row.ordinal >= start, || {
                    Ok((
                        row.clone(),
                        WorkforcePositionV1::Ordinal {
                            next_ordinal: row.ordinal + 1,
                        },
                    ))
                })?;
            }
            page.finish(
                scenario_id,
                context,
                WorkforceSetupViewDataV1::CommandChanges,
            )
        };
        let actual = WorkforceSetupResultV1 {
            schema_version: 1,
            result: build(limit, 0)?,
        };
        assert_eq!(
            serde_json::to_value(actual)?,
            serde_json::to_value(expected)?
        );
        assert!(matches!(
            build(limit - 1, 0),
            Err(DomainPackError::ResourceLimitExceeded)
        ));
        let WorkforceSetupViewDataV1::CommandChanges(last) = build(limit, 1)? else {
            return Err("wrong result family".into());
        };
        assert_eq!(last.total_items, 2);
        assert_eq!(
            serde_json::to_value(last.items)?,
            serde_json::to_value([&rows[1]])?
        );
        assert!(last.continuation.is_none());
        Ok(())
    }
}
