use super::*;

fn repository() -> Result<tempfile::TempDir> {
    let root = tempfile::tempdir()?;
    for (relative, bytes) in BUILD_SOURCES {
        let path = root.path().join(RUNNER_ROOT).join(relative);
        std::fs::create_dir_all(path.parent().context("source parent")?)?;
        std::fs::write(path, bytes)?;
    }
    generate(root.path(), false)?;
    Ok(root)
}

fn replace_bound(
    root: &Path,
    manifest: &mut CorpusManifest,
    value: &impl Serialize,
    repair: bool,
) -> Result<()> {
    let bytes = json(value)?;
    let target = if repair {
        &mut manifest.repair
    } else {
        &mut manifest.expectations
    };
    *target = binding(target.path.clone(), &bytes)?;
    std::fs::write(root.join(&target.path), bytes)?;
    std::fs::write(root.join(CORPUS_PATH), json(manifest)?)?;
    Ok(())
}

#[allow(clippy::too_many_lines)]
#[test]
fn corpus_rejects_untrusted_or_resealed_inputs() -> Result<()> {
    let directory = repository()?;
    let root = directory.path();
    let valid = load(root)?;
    let manifest_bytes = std::fs::read(root.join(CORPUS_PATH))?;
    let expected_bytes = std::fs::read(root.join(EXPECTED_PATH))?;
    let repair_bytes = std::fs::read(root.join(REPAIR_PATH))?;

    let mut manifest = valid.manifest.clone();
    manifest.schema_version += 1;
    std::fs::write(root.join(CORPUS_PATH), json(&manifest)?)?;
    assert!(load(root).is_err());

    for path in [
        "../outside.json",
        "/absolute.json",
        "domains/workforce/fixtures/v2/../clinic-tiny.json",
    ] {
        let mut manifest = valid.manifest.clone();
        manifest.cases[0].input.path = path.to_owned();
        std::fs::write(root.join(CORPUS_PATH), json(&manifest)?)?;
        assert!(load(root).is_err());
    }

    let mut manifest = valid.manifest.clone();
    manifest.profile.budget_milliseconds += 1;
    std::fs::write(root.join(CORPUS_PATH), json(&manifest)?)?;
    assert!(load(root).is_err());

    let mut expected: ExpectedCorpus = serde_json::from_slice(&expected_bytes)?;
    expected.cases.pop();
    replace_bound(root, &mut valid.manifest.clone(), &expected, false)?;
    assert!(load(root).is_err());

    let mut expected: ExpectedCorpus = serde_json::from_slice(&expected_bytes)?;
    expected
        .cases
        .iter_mut()
        .find(|case| case.expected.id == CaseId::AppendixF)
        .context("Appendix-F expectation")?
        .expected
        .disposition = ExpectedDisposition::Accepted;
    replace_bound(root, &mut valid.manifest.clone(), &expected, false)?;
    assert!(load(root).is_err());
    std::fs::write(root.join(EXPECTED_PATH), &expected_bytes)?;

    let mut repair: RepairFixture = serde_json::from_slice(&repair_bytes)?;
    repair.execution_phase = 6;
    replace_bound(root, &mut valid.manifest.clone(), &repair, true)?;
    assert!(load(root).is_err());
    let recipe: RepairFixture = serde_json::from_slice(&repair_bytes)?;
    let before = valid
        .fixtures
        .iter()
        .find(|fixture| fixture.expected.id == CaseId::PractitionerWorkflow)
        .context("practitioner repair source")?;
    let domain = eutheto_workforce::validation::validate_document(&before.snapshot.document)?;
    let mut people = domain.entities.values().filter_map(|entity| {
        if let eutheto_workforce::model::WorkforceEntity::Person(person) = entity {
            Some(person.id)
        } else {
            None
        }
    });
    let first_person = people.next().context("first repair person")?;
    let second_person = people.next().context("second repair person")?;
    let first_payload = cases::repair_payload(&recipe, first_person);
    let mut second_payload = cases::repair_payload(&recipe, second_person);
    let eutheto_workforce::model::WorkforceEntity::Availability(first_change) =
        &first_payload.entity
    else {
        anyhow::bail!("first repair payload is not availability")
    };
    let eutheto_workforce::model::WorkforceEntity::Availability(second_change) =
        &mut second_payload.entity
    else {
        anyhow::bail!("second repair payload is not availability")
    };
    assert_eq!(first_change.person_id, first_person);
    assert_eq!(second_change.person_id, second_person);
    assert_ne!(first_person, second_person);
    assert_eq!(first_change.id, recipe.availability_id);
    assert_eq!(
        first_change.availability_kind,
        eutheto_workforce::model::AvailabilityKind::Unavailable
    );
    assert_eq!(first_change.time_window, recipe.time_window);
    assert_eq!(first_change.effective_range, recipe.effective_range);
    assert!(first_change.assignment_type_ids.is_none() && first_change.location_ids.is_none());
    second_change.person_id = first_person;
    assert_eq!(first_payload, second_payload);

    let alternate_target = eutheto_workforce::temporal::resolve_shifts(
        &before.snapshot.document,
        &eutheto_types::CancellationToken::new(),
    )?
    .into_iter()
    .find(|shift| {
        shift.reporting_date.to_string() == "2027-02-10" && shift.id != recipe.target_shift_id
    })
    .context("same-day non-clinic repair target")?
    .id;
    let current = serde_json::to_value(&recipe)?;
    for (field, value) in [
        ("schemaVersion", serde_json::json!(1)),
        ("schemaVersion", serde_json::json!(3)),
        ("commandId", serde_json::json!("untrusted.command")),
        ("availabilityId", current["targetShiftId"].clone()),
        ("targetShiftId", serde_json::json!("malformed-target")),
        ("targetShiftId", current["availabilityId"].clone()),
        ("targetShiftId", serde_json::to_value(alternate_target)?),
        ("personId", serde_json::to_value(first_person)?),
        ("payload", serde_json::to_value(&first_payload)?),
        (
            "timeWindow",
            serde_json::json!({
                "kind":"instant","startsAt":"2027-02-11T13:00:00Z",
                "endsAt":"2027-02-12T01:00:00Z"
            }),
        ),
        (
            "timeWindow",
            serde_json::json!({"kind":"weekly","windows":[]}),
        ),
        (
            "effectiveRange",
            serde_json::json!({
                "startDate":"2027-02-01","endDateExclusive":"2027-03-02"
            }),
        ),
    ] {
        let mut changed = current.clone();
        changed[field] = value;
        replace_bound(root, &mut valid.manifest.clone(), &changed, true)?;
        assert!(
            load(root).is_err(),
            "resealed recipe field {field} was admitted"
        );
    }
    let mut legacy = current;
    let object = legacy.as_object_mut().context("recipe object")?;
    for field in [
        "availabilityId",
        "targetShiftId",
        "timeWindow",
        "effectiveRange",
    ] {
        object.remove(field);
    }
    object.insert("schemaVersion".to_owned(), serde_json::json!(1));
    object.insert("payload".to_owned(), serde_json::to_value(&first_payload)?);
    replace_bound(root, &mut valid.manifest.clone(), &legacy, true)?;
    assert!(
        load(root).is_err(),
        "legacy person-bound recipe was admitted"
    );
    std::fs::write(root.join(REPAIR_PATH), &repair_bytes)?;
    std::fs::write(root.join(CORPUS_PATH), &manifest_bytes)?;

    let input = root.join(valid.manifest.cases[0].input.path.clone());
    let original = std::fs::read(&input)?;
    let mut altered = original.clone();
    altered.push(b' ');
    std::fs::write(&input, altered)?;
    assert!(load(root).is_err());
    std::fs::write(&input, original)?;

    let extra = root.join(FIXTURE_ROOT).join("unindexed.json");
    std::fs::write(&extra, b"{}")?;
    assert!(load(root).is_err());
    std::fs::remove_file(extra)?;
    load(root)?;

    #[cfg(unix)]
    reject_linked_generation(root)?;
    Ok(())
}

#[cfg(unix)]
fn reject_linked_generation(root: &Path) -> Result<()> {
    let outside = tempfile::tempdir()?;
    let marker = outside.path().join("sentinel");
    std::fs::write(&marker, b"unchanged")?;
    let fixtures = root.join(FIXTURE_ROOT);
    let saved = root.join("saved-fixtures");
    std::fs::rename(&fixtures, &saved)?;
    std::os::unix::fs::symlink(outside.path(), &fixtures)?;
    assert!(generate(root, false).is_err());
    assert_eq!(std::fs::read(&marker)?, b"unchanged");
    assert_eq!(std::fs::read_dir(outside.path())?.count(), 1);
    std::fs::remove_file(&fixtures)?;
    std::fs::rename(saved, fixtures)?;
    load(root)?;
    Ok(())
}
