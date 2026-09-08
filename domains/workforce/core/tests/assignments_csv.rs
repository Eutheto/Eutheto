use eutheto_domain_ir::{
    AssignmentValue, DomainAssignment, DomainAssignmentId, DomainEntityId, DomainEntityKindId,
    DomainEntityRef, NormalizedSolution,
};
use eutheto_types::{
    CancellationToken, DurationMillis, FixedMonotonicClock, MonotonicClock, OperationControl,
    PackId, ParentSolveBudget,
};
use eutheto_workforce::{
    assignments_csv::{
        AssignmentsCsvError, AssignmentsCsvErrorCode as Code, MAX_ASSIGNMENTS_CSV_BYTES,
        MAX_ASSIGNMENTS_CSV_CELL_BYTES, MAX_ASSIGNMENTS_CSV_ROWS, decode_assignments_csv,
        encode_assignments_csv,
    },
    model::AssignmentPair,
};
use std::{
    error::Error,
    io::{self, Read},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

type Result<T = ()> = std::result::Result<T, Box<dyn Error>>;
const FRAMING: &str = "eutheto/assignments,1\nassignment_id,person_id,shift_id\n";
const KIND: &str = "official.workforce.assignment";

fn id(index: u32) -> String {
    format!("018f7b40-a000-7000-8000-{index:012x}")
}

fn control() -> OperationControl {
    OperationControl::Cancellation(CancellationToken::new())
}

fn pair(person: u32, shift: u32) -> Result<AssignmentPair> {
    Ok(AssignmentPair {
        person_id: id(person).parse()?,
        shift_id: id(shift).parse()?,
    })
}

fn assignment(person: u32, shift: u32, selected: bool) -> Result<DomainAssignment> {
    let pair = format!("{}.{}", id(person), id(shift));
    Ok(DomainAssignment {
        id: DomainAssignmentId::new(format!("{KIND}.{pair}"))?,
        entity: DomainEntityRef {
            kind: DomainEntityKindId::new(KIND)?,
            id: DomainEntityId::new(pair)?,
        },
        value: AssignmentValue::Boolean(selected),
        evidence: Vec::new(),
    })
}

fn solution(assignments: Vec<DomainAssignment>) -> Result<NormalizedSolution> {
    Ok(NormalizedSolution {
        schema_version: 1,
        pack_id: PackId::new("official.workforce")?,
        scenario_id: id(100).parse()?,
        scenario_revision: 1,
        projection_version: 1,
        solution_id: id(101).parse()?,
        assignments,
    })
}

fn row(person: u32, shift: u32) -> String {
    format!(
        "{KIND}.{}.{},{},{}\n",
        id(person),
        id(shift),
        id(person),
        id(shift)
    )
}

fn decode(mut bytes: &[u8]) -> std::result::Result<Vec<AssignmentPair>, AssignmentsCsvError> {
    decode_assignments_csv(&mut bytes, &control())
}

fn rejection(bytes: &[u8], code: Code, row: Option<u32>) {
    assert_eq!(decode(bytes), Err(AssignmentsCsvError { code, row }));
}

#[test]
fn empty_and_false_only_schedules_keep_both_framing_records() -> Result {
    for assignments in [vec![], vec![assignment(1, 2, false)?]] {
        let encoded = encode_assignments_csv(&solution(assignments)?, &control())?;
        assert_eq!(encoded, FRAMING.as_bytes());
        assert_eq!(decode(&encoded)?, Vec::<AssignmentPair>::new());
    }
    assert_eq!(
        decode(FRAMING.trim_end().as_bytes())?,
        Vec::<AssignmentPair>::new()
    );
    rejection(b"", Code::InvalidVersion, Some(1));
    rejection(b"eutheto/assignments,1\n", Code::InvalidHeader, Some(2));
    Ok(())
}

#[test]
fn selected_round_trip_orders_identity_and_keeps_repeated_people_across_shifts() -> Result {
    let mut source = solution(vec![
        assignment(2, 8, true)?,
        assignment(1, 9, true)?,
        assignment(1, 8, true)?,
        assignment(1, 7, false)?,
    ])?;
    let expected = format!("{FRAMING}{}{}{}", row(1, 8), row(1, 9), row(2, 8));
    let encoded = encode_assignments_csv(&source, &control())?;
    assert_eq!(encoded, expected.as_bytes());
    assert_eq!(
        decode(&encoded)?,
        vec![pair(1, 8)?, pair(1, 9)?, pair(2, 8)?]
    );
    source.assignments.reverse();
    assert_eq!(encode_assignments_csv(&source, &control())?, encoded);
    let reversed = format!("{FRAMING}{}{}{}", row(2, 8), row(1, 9), row(1, 8));
    assert_eq!(decode(reversed.as_bytes())?, decode(&encoded)?);
    Ok(())
}

struct Chunked<'a> {
    bytes: &'a [u8],
    maximum: usize,
}

impl Read for Chunked<'_> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        let count = output.len().min(self.maximum).min(self.bytes.len());
        output[..count].copy_from_slice(&self.bytes[..count]);
        self.bytes = &self.bytes[count..];
        Ok(count)
    }
}

#[test]
fn quoted_crlf_and_chunk_boundaries_preserve_the_schedule() -> Result {
    let input = format!(
        "\"eutheto/assignments\",\"1\"\r\n\"assignment_id\",\"person_id\",\"shift_id\"\r\n\"{KIND}.{}.{}\",\"{}\",\"{}\"",
        id(1),
        id(2),
        id(1),
        id(2),
    );
    // Single-byte reads split every CRLF, quote boundary and identity segment.
    for maximum in [1, 7, 4096] {
        let mut reader = Chunked {
            bytes: input.as_bytes(),
            maximum,
        };
        assert_eq!(
            decode_assignments_csv(&mut reader, &control())?,
            vec![pair(1, 2)?]
        );
    }
    Ok(())
}

#[test]
fn framing_and_field_counts_are_exact() {
    for (input, code, row) in [
        ("eutheto/assignments,2\n", Code::InvalidVersion, 1),
        ("other,1\n", Code::InvalidVersion, 1),
        ("eutheto/assignments,1,extra\n", Code::FieldCount, 1),
        ("eutheto/assignments\n", Code::FieldCount, 1),
        (
            "eutheto/assignments,1\nassignment_id,shift_id,person_id\n",
            Code::InvalidHeader,
            2,
        ),
        (
            "eutheto/assignments,1\nassignment_id,person_id\n",
            Code::FieldCount,
            2,
        ),
        (
            "eutheto/assignments,1\nassignment_id,person_id,shift_id,extra\n",
            Code::FieldCount,
            2,
        ),
        ("eutheto/assignments;1\n", Code::FieldCount, 1),
        ("\u{feff}eutheto/assignments,1\n", Code::MalformedCsv, 1),
    ] {
        rejection(input.as_bytes(), code, Some(row));
    }
    for data in ["a,b\n", "a,b,c,d\n", "\n"] {
        rejection(
            format!("{FRAMING}{data}").as_bytes(),
            Code::FieldCount,
            Some(3),
        );
    }
}

#[test]
fn malformed_quotes_and_record_terminators_are_not_repaired() {
    for data in [
        "ab\"cd,b,c\n",
        "\"a\"x,b,c\n",
        "\"a\" ,b,c\n",
        "\"a,b,c\n",
        "a,b,c\r",
        "a,b,c\rx",
        "a,b,c\r\r\n",
    ] {
        rejection(
            format!("{FRAMING}{data}").as_bytes(),
            Code::MalformedCsv,
            Some(3),
        );
    }
    // Legal embedded commas/newlines/escaped quotes are decoded rather than rejected
    // as CSV syntax, but can never become valid typed assignment identities.
    for data in [
        "\"a,b\",b,c\n",
        "\"a\nb\",b,c\n",
        "\"a\r\nb\",b,c\n",
        "\"a\"\"b\",b,c\n",
    ] {
        rejection(
            format!("{FRAMING}{data}").as_bytes(),
            Code::InvalidIdentity,
            Some(3),
        );
    }
}

#[test]
fn original_utf8_is_validated_before_quote_removal_including_split_codepoints() {
    for data in [
        &b"\"\xc3\"\xa9,b,c\n"[..],
        &b"\"\xff\",b,c\n"[..],
        &b"\"\xe2\x82"[..],
    ] {
        let mut bytes = FRAMING.as_bytes().to_vec();
        bytes.extend_from_slice(data);
        for maximum in [1, 4096] {
            let mut reader = Chunked {
                bytes: &bytes,
                maximum,
            };
            assert_eq!(
                decode_assignments_csv(&mut reader, &control()),
                Err(AssignmentsCsvError {
                    code: Code::InvalidUtf8,
                    row: None,
                })
            );
        }
    }
    let bytes = format!("{FRAMING}\"é\",b,c\n");
    let mut reader = Chunked {
        bytes: bytes.as_bytes(),
        maximum: 1,
    };
    assert_eq!(
        decode_assignments_csv(&mut reader, &control()),
        Err(AssignmentsCsvError {
            code: Code::InvalidIdentity,
            row: Some(3),
        })
    );
}

#[test]
fn duplicate_identity_and_pair_disagreement_are_rejected() -> Result {
    rejection(
        format!("{FRAMING}{}{}", row(1, 2), row(1, 2)).as_bytes(),
        Code::DuplicateAssignment,
        Some(4),
    );
    let source = solution(vec![assignment(1, 2, true)?, assignment(1, 2, true)?])?;
    assert_eq!(
        encode_assignments_csv(&source, &control()),
        Err(AssignmentsCsvError {
            code: Code::DuplicateAssignment,
            row: None,
        })
    );
    let mismatch = format!("{FRAMING}{KIND}.{}.{},{},{}\n", id(1), id(2), id(1), id(3));
    rejection(mismatch.as_bytes(), Code::InvalidIdentity, Some(3));
    let mut bad_source = solution(vec![assignment(1, 2, true)?])?;
    bad_source.assignments[0].entity.id = DomainEntityId::new(format!("{}.{}", id(1), id(3)))?;
    assert_eq!(
        encode_assignments_csv(&bad_source, &control()),
        Err(AssignmentsCsvError {
            code: Code::InvalidIdentity,
            row: None,
        })
    );
    Ok(())
}

#[test]
fn identity_rejects_lock_uuid_foreign_namespace_and_noncanonical_uuid_spellings() {
    let person = id(1);
    let shift = id(2);
    let valid_id = format!("{KIND}.{person}.{shift}");
    for identity in [id(3), format!("official.other.assignment.{person}.{shift}")] {
        let input = format!("{FRAMING}{identity},{person},{shift}\n");
        rejection(input.as_bytes(), Code::InvalidIdentity, Some(3));
    }
    for noncanonical in [
        person.to_uppercase(),
        person.replace('-', ""),
        person.replacen("7000", "4000", 1),
    ] {
        // Both the assignment and cell agree, so rejection must enforce the typed
        // canonical UUIDv7 requirement, not merely compare two submitted strings.
        let input = format!("{FRAMING}{KIND}.{noncanonical}.{shift},{noncanonical},{shift}\n");
        rejection(input.as_bytes(), Code::InvalidIdentity, Some(3));
    }
    let input = format!("{FRAMING}{valid_id}, {person},{shift}\n");
    rejection(input.as_bytes(), Code::InvalidIdentity, Some(3));
}

#[test]
fn decoded_cell_limit_counts_utf8_bytes_and_unescaped_quotes() {
    for cell in [
        "a".repeat(MAX_ASSIGNMENTS_CSV_CELL_BYTES),
        "é".repeat(MAX_ASSIGNMENTS_CSV_CELL_BYTES / 2),
    ] {
        rejection(
            format!("{FRAMING}\"{cell}\",b,c\n").as_bytes(),
            Code::InvalidIdentity,
            Some(3),
        );
        rejection(
            format!("{FRAMING}\"{cell}x\",b,c\n").as_bytes(),
            Code::CellLimit,
            Some(3),
        );
    }
    let escaped = "\"\"".repeat(MAX_ASSIGNMENTS_CSV_CELL_BYTES);
    rejection(
        format!("{FRAMING}\"{escaped}\",b,c\n").as_bytes(),
        Code::InvalidIdentity,
        Some(3),
    );
    rejection(
        format!("{FRAMING}\"{escaped}\"\"\",b,c\n").as_bytes(),
        Code::CellLimit,
        Some(3),
    );
}

#[test]
fn inclusive_source_cap_and_output_cap_are_enforced_without_reading_ahead() -> Result {
    let row_bytes = row(1, 1).len();
    let count = (MAX_ASSIGNMENTS_CSV_BYTES - FRAMING.len()) / row_bytes;
    let source = solution(
        (0..u32::try_from(count)?)
            .map(|shift| assignment(1, shift, true))
            .collect::<Result<Vec<_>>>()?,
    )?;
    let encoded = encode_assignments_csv(&source, &control())?;
    let expected: Vec<_> = (0..u32::try_from(count)?)
        .map(|shift| pair(1, shift))
        .collect::<Result<_>>()?;
    assert_eq!(decode(&encoded)?, expected);
    let mut too_large = source;
    too_large
        .assignments
        .push(assignment(1, u32::try_from(count)?, true)?);
    assert_eq!(
        encode_assignments_csv(&too_large, &control()),
        Err(AssignmentsCsvError {
            code: Code::ByteLimit,
            row: None,
        })
    );

    // Switch enough LF records to CRLF to hit the inclusive input byte cap exactly,
    // without adding fields, changing identity, or padding arbitrary user text.
    let mut extra = MAX_ASSIGNMENTS_CSV_BYTES - encoded.len();
    let mut exact = Vec::with_capacity(MAX_ASSIGNMENTS_CSV_BYTES + 2);
    for byte in encoded {
        if byte == b'\n' && extra != 0 {
            exact.push(b'\r');
            extra -= 1;
        }
        exact.push(byte);
    }
    assert_eq!(extra, 0);
    assert_eq!(exact.len(), MAX_ASSIGNMENTS_CSV_BYTES);
    assert_eq!(decode(&exact)?, expected);
    exact.extend_from_slice(b"\n\n");
    let mut unread = exact.as_slice();
    assert_eq!(
        decode_assignments_csv(&mut unread, &control()),
        Err(AssignmentsCsvError {
            code: Code::ByteLimit,
            row: None,
        })
    );
    assert_eq!(unread, b"\n");
    Ok(())
}

#[test]
fn selected_row_cap_is_inclusive_and_false_decisions_do_not_consume_it() -> Result {
    // Every valid canonical row is large enough that the byte cap is reached before
    // 100,000 rows. At that row count the encoder must reject bytes, not rows.
    let assignments = (0..u32::try_from(MAX_ASSIGNMENTS_CSV_ROWS)?)
        .map(|shift| assignment(1, shift, true))
        .collect::<Result<Vec<_>>>()?;
    let mut source = solution(assignments)?;
    source.assignments.push(assignment(2, 1, false)?);
    assert_eq!(
        encode_assignments_csv(&source, &control()),
        Err(AssignmentsCsvError {
            code: Code::ByteLimit,
            row: None,
        })
    );
    source.assignments.push(assignment(2, 2, true)?);
    assert_eq!(
        encode_assignments_csv(&source, &control()),
        Err(AssignmentsCsvError {
            code: Code::RowLimit,
            row: None,
        })
    );
    Ok(())
}

struct InterruptingReader<'a> {
    bytes: &'a [u8],
    token: CancellationToken,
}

impl Read for InterruptingReader<'_> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        let read = self.bytes.read(output)?;
        self.token.cancel();
        Ok(read)
    }
}

struct AdvancingClock(AtomicU64);

impl MonotonicClock for AdvancingClock {
    fn now(&self) -> Duration {
        Duration::from_millis(self.0.fetch_add(1, Ordering::Relaxed))
    }
}

#[test]
fn interruption_distinguishes_cancellation_and_deadline_before_and_during_work() -> Result {
    let source = solution(vec![assignment(1, 2, true)?])?;
    let bytes = format!("{FRAMING}{}", row(1, 2));
    let token = CancellationToken::new();
    token.cancel();
    let cancelled = OperationControl::Cancellation(token);
    let cancellation = Err(AssignmentsCsvError {
        code: Code::Cancelled,
        row: None,
    });
    assert_eq!(encode_assignments_csv(&source, &cancelled), cancellation);
    assert_eq!(
        decode_assignments_csv(&mut bytes.as_bytes(), &cancelled),
        Err(AssignmentsCsvError {
            code: Code::Cancelled,
            row: None,
        })
    );

    let token = CancellationToken::new();
    let mut reader = InterruptingReader {
        bytes: bytes.as_bytes(),
        token: token.clone(),
    };
    assert_eq!(
        decode_assignments_csv(&mut reader, &OperationControl::Cancellation(token)),
        Err(AssignmentsCsvError {
            code: Code::Cancelled,
            row: None,
        })
    );
    let clock = Arc::new(FixedMonotonicClock::default());
    let budget = ParentSolveBudget::new(
        DurationMillis::new(1)?,
        clock.clone(),
        CancellationToken::new(),
    )?;
    clock.advance(Duration::from_millis(1))?;
    let expired = OperationControl::Solve(budget.phase_view());
    assert_eq!(
        encode_assignments_csv(&source, &expired),
        Err(AssignmentsCsvError {
            code: Code::DeadlineExceeded,
            row: None,
        })
    );
    assert_eq!(
        decode_assignments_csv(&mut bytes.as_bytes(), &expired),
        Err(AssignmentsCsvError {
            code: Code::DeadlineExceeded,
            row: None,
        })
    );

    for encode in [false, true] {
        let budget = ParentSolveBudget::new(
            DurationMillis::new(3)?,
            Arc::new(AdvancingClock(AtomicU64::new(0))),
            CancellationToken::new(),
        )?;
        let running = OperationControl::Solve(budget.phase_view());
        assert_eq!(running.check(), Ok(()));
        let error = if encode {
            encode_assignments_csv(&source, &running)
                .err()
                .ok_or("expected interrupted encoding")?
        } else {
            decode_assignments_csv(&mut bytes.as_bytes(), &running)
                .err()
                .ok_or("expected interrupted decoding")?
        };
        assert_eq!(
            error,
            AssignmentsCsvError {
                code: Code::DeadlineExceeded,
                row: None
            }
        );
    }
    Ok(())
}

#[test]
fn io_errors_are_bounded_and_transient_interruption_is_retried() -> Result {
    struct Failing;
    impl Read for Failing {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::other("sensitive path and source details"))
        }
    }
    struct Retry<'a> {
        bytes: &'a [u8],
        interrupted: bool,
    }
    impl Read for Retry<'_> {
        fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
            if !self.interrupted {
                self.interrupted = true;
                return Err(io::ErrorKind::Interrupted.into());
            }
            self.bytes.read(output)
        }
    }
    assert_eq!(
        decode_assignments_csv(&mut Failing, &control()),
        Err(AssignmentsCsvError {
            code: Code::Io,
            row: None,
        })
    );
    let bytes = format!("{FRAMING}{}", row(1, 2));
    let mut reader = Retry {
        bytes: bytes.as_bytes(),
        interrupted: false,
    };
    assert_eq!(
        decode_assignments_csv(&mut reader, &control())?,
        vec![pair(1, 2)?]
    );
    Ok(())
}
