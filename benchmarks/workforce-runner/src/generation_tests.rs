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
        "domains/workforce/fixtures/v1/../clinic-tiny.json",
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
    expected.cases[0].expected.disposition = ExpectedDisposition::Accepted;
    replace_bound(root, &mut valid.manifest.clone(), &expected, false)?;
    assert!(load(root).is_err());
    std::fs::write(root.join(EXPECTED_PATH), &expected_bytes)?;

    let mut repair: RepairFixture = serde_json::from_slice(&repair_bytes)?;
    repair.execution_phase = 6;
    replace_bound(root, &mut valid.manifest.clone(), &repair, true)?;
    assert!(load(root).is_err());
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
