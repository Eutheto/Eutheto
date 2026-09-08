#![forbid(unsafe_code)]

#[path = "../../../domains/workforce/core/tests/support/mod.rs"]
mod workforce_fixture;

use eutheto_core::HeadlessService;
use eutheto_import::MigrationRegistryKind;
use eutheto_types::{
    ActorRef, AppError, CancellationToken, CommandEnvelope, CommandSource, DomainCommandEnvelope,
    FixedClock, FixedIdGenerator, FixedMonotonicClock, OperationControl, Revision, ScenarioCommand,
    ScenarioSnapshotV1, ValidationSeverity,
};
use eutheto_workforce::{commands, model::WorkforceEntity};
use serde_json::{Value, json};
use std::{collections::BTreeSet, error::Error, sync::Arc};
use workforce_fixture::id;

trait BoxedResult<T> {
    fn boxed(self) -> Result<T, Box<dyn Error>>;
}

impl<T> BoxedResult<T> for Result<T, AppError> {
    fn boxed(self) -> Result<T, Box<dyn Error>> {
        self.map_err(|error| std::io::Error::other(format!("{error:?}")).into())
    }
}

fn service() -> Result<HeadlessService, Box<dyn Error>> {
    HeadlessService::new(
        Arc::new(FixedClock::new("2026-09-07T12:00:00Z".parse()?)),
        Arc::new(FixedMonotonicClock::default()),
        Arc::new(FixedIdGenerator::new([])),
    )
    .boxed()
}

fn snapshot() -> Result<ScenarioSnapshotV1, Box<dyn Error>> {
    let mut document = workforce_fixture::fixture()?;
    // Keep the real fall-back-day manual shift, without an ambiguous recurring occurrence.
    document.domain.entities.remove(&id(6).parse()?);
    document.domain.locked_assignments.clear();
    document.domain.rules.insert(
        id(30).parse()?,
        json!({
            "id": id(30), "kind": "coverage", "active": true, "strength": "required",
            "scope": {"people": {"kind": "all"}}
        }),
    );
    Ok(ScenarioSnapshotV1::current(
        Revision::new(7),
        document,
        BTreeSet::default(),
    ))
}

fn rename_person(
    snapshot: &ScenarioSnapshotV1,
    name: &str,
) -> Result<CommandEnvelope, Box<dyn Error>> {
    let value = snapshot
        .document
        .domain
        .entities
        .get(&id(1).parse()?)
        .ok_or("missing fixture person")?;
    let mut entity: WorkforceEntity = serde_json::from_value(value.clone())?;
    let WorkforceEntity::Person(person) = &mut entity else {
        return Err("wrong fixture entity kind".into());
    };
    name.clone_into(&mut person.name);
    Ok(CommandEnvelope {
        command_id: id(200).parse()?,
        scenario_id: snapshot.document.scenario_id,
        expected_revision: snapshot.revision,
        actor: ActorRef {
            actor_id: None,
            display_name: "Headless scenario test".to_owned(),
        },
        source: CommandSource::System,
        command: ScenarioCommand::ApplyDomainCommand(DomainCommandEnvelope {
            command_type: commands::UPDATE_ENTITY.to_owned(),
            payload: serde_json::to_value(commands::EntityPayload { entity })?,
        }),
    })
}

#[test]
fn typed_workforce_mutation_rejects_stale_edits_without_changing_either_snapshot()
-> Result<(), Box<dyn Error>> {
    let service = service()?;
    let original = snapshot()?;
    let original_bytes = serde_json::to_vec(&original)?;
    let cancellation = CancellationToken::default();
    let control = OperationControl::Cancellation(cancellation.clone());
    let envelope = rename_person(&original, "River revised")?;
    let applied = service
        .apply_scenario(original.clone(), &envelope, &cancellation)
        .boxed()?;
    assert_eq!(applied.snapshot.revision, Revision::new(8));
    assert_eq!(
        applied.snapshot.document.domain.entities[&id(1).parse()?]["name"],
        json!("River revised")
    );
    assert_eq!(serde_json::to_vec(&original)?, original_bytes);
    assert_eq!(
        applied.snapshot.document.domain.entities[&id(8).parse()?],
        original.document.domain.entities[&id(8).parse()?]
    );

    let committed_bytes = service
        .encode_scenario(&applied.snapshot, &control)
        .boxed()?;
    let mut stale = rename_person(&original, "Stale overwrite")?;
    stale.command_id = id(201).parse()?;
    assert!(matches!(
        service.apply_scenario(applied.snapshot.clone(), &stale, &cancellation),
        Err(AppError::Conflict { expected_revision, actual_revision })
            if expected_revision == original.revision
                && actual_revision == applied.snapshot.revision
    ));
    assert_eq!(
        service
            .encode_scenario(&applied.snapshot, &control)
            .boxed()?,
        committed_bytes
    );
    assert_eq!(serde_json::to_vec(&original)?, original_bytes);

    // A rejected edit must not consume a revision or make this snapshot unusable.
    stale.expected_revision = applied.snapshot.revision;
    let rebased = service
        .apply_scenario(applied.snapshot, &stale, &cancellation)
        .boxed()?;
    assert_eq!(rebased.snapshot.revision, Revision::new(9));
    assert_eq!(
        rebased.snapshot.document.domain.entities[&id(1).parse()?]["name"],
        json!("Stale overwrite")
    );
    Ok(())
}

#[test]
fn standalone_ingress_preserves_opaque_data_and_reports_migration_but_requires_dependency_closure()
-> Result<(), Box<dyn Error>> {
    let service = service()?;
    let control = OperationControl::Cancellation(CancellationToken::default());
    let mut source = snapshot()?;
    source.extensions.insert(
        "nonsemantic.future.presentation".to_owned(),
        json!({"layout": [null, {"weight": 3, "visible": false}], "note": "opaque"}),
    );
    // The frozen snapshot is genuine portable-v1 input; conversion must report its provenance.
    let inspected = service
        .decode_scenario(&serde_json::to_vec(&source)?, &control)
        .boxed()?;
    assert_eq!(inspected.original_schema_version, 1);
    assert!(inspected.applied_migrations.iter().any(|migration| {
        migration.registry == MigrationRegistryKind::Portable
            && migration.from_version == 1
            && migration.to_version == 2
    }));
    assert_eq!(inspected.scenario.document, source.document);
    assert_eq!(inspected.scenario.extensions, source.extensions);
    assert_eq!(inspected.scenario.revision, source.revision);

    let encoded = service
        .encode_scenario(&inspected.scenario, &control)
        .boxed()?;
    let current = service.decode_scenario(&encoded, &control).boxed()?;
    assert_eq!(current.scenario, inspected.scenario);
    assert!(current.applied_migrations.is_empty());

    // Duplicate keys must fail before a permissive JSON decoder can choose a value.
    let duplicate = String::from_utf8(encoded.clone())?.replacen('{', "{\"revision\":0,", 1);
    assert!(matches!(
        service.decode_scenario(duplicate.as_bytes(), &control),
        Err(AppError::Validation(_))
    ));

    // Both dependency classes require supplemental custody that standalone JSON cannot carry.
    for dependency in [
        json!({"assetId": "clinic-map.svg"}),
        json!({"scenarioId": id(101)}),
    ] {
        let mut dependent = current.scenario.clone();
        dependent
            .extensions
            .insert("example.dependency".to_owned(), dependency.clone());
        assert!(matches!(
            service.encode_scenario(&dependent, &control),
            Err(AppError::Validation(_))
        ));
        let mut wire: Value = serde_json::from_slice(&encoded)?;
        wire["extensions"]["example.dependency"] = dependency;
        assert!(matches!(
            service.decode_scenario(&serde_json::to_vec(&wire)?, &control),
            Err(AppError::Validation(_))
        ));
    }
    assert_eq!(
        service
            .encode_scenario(&current.scenario, &control)
            .boxed()?,
        encoded
    );
    Ok(())
}

#[test]
fn bounded_domain_work_is_operational_during_portable_io_and_mutation() -> Result<(), Box<dyn Error>>
{
    let service = HeadlessService::new(
        Arc::new(FixedClock::new("2026-09-07T12:00:00Z".parse()?)),
        Arc::new(FixedMonotonicClock::default()),
        Arc::new(FixedIdGenerator::new([id(400).parse()?])),
    )
    .boxed()?;
    let cancellation = CancellationToken::default();
    let control = OperationControl::Cancellation(cancellation.clone());
    let mut source = snapshot()?;
    let mut wire: Value =
        serde_json::from_slice(&service.encode_scenario(&source, &control).boxed()?)?;
    // Valid host metadata can exceed a pack's smaller finite inspection allowance.
    source.document.metadata.description =
        "x".repeat(eutheto_domain_api::ContractJsonLimits::DEFAULT.max_string_bytes + 1);
    wire["metadata"]["description"] = json!(source.document.metadata.description);
    let envelope = rename_person(&source, "River revised")?;
    for (operation, error) in [
        (
            "decode",
            service
                .decode_scenario(&serde_json::to_vec(&wire)?, &control)
                .err(),
        ),
        (
            "legacy decode",
            service
                .decode_scenario(&serde_json::to_vec(&source)?, &control)
                .err(),
        ),
        (
            "creation",
            service
                .create_scenario(
                    source.document.metadata.title.clone(),
                    source.document.metadata.description.clone(),
                    source.document.domain_pack.clone(),
                    source.document.settings.clone(),
                    &control,
                )
                .err(),
        ),
        ("encode", service.encode_scenario(&source, &control).err()),
        (
            "mutation",
            service
                .apply_scenario(source, &envelope, &cancellation)
                .err(),
        ),
    ] {
        assert!(
            matches!(&error, Some(AppError::Protocol(failure)) if failure.code == "operation.resource_limit"),
            "{operation}: {error:?}"
        );
    }
    Ok(())
}

#[test]
fn full_readiness_reports_impossible_coverage_as_a_finding_not_malformed_input()
-> Result<(), Box<dyn Error>> {
    let service = service()?;
    let control = OperationControl::Cancellation(CancellationToken::default());
    let mut scenario = snapshot()?;
    assert!(
        service
            .validate_full(&scenario, &control)
            .boxed()?
            .issues
            .is_empty()
    );

    // One person cannot fill two places on this shift, but the authored coverage is well formed.
    scenario
        .document
        .domain
        .entities
        .get_mut(&id(8).parse()?)
        .ok_or("missing fixture shift")?["coverage"]["count"] = json!(2);
    let before = serde_json::to_vec(&scenario)?;
    let report = service.validate_full(&scenario, &control).boxed()?;
    assert!(report.issues.iter().any(|issue| {
        issue.code == "official.workforce.candidate_shortage"
            && issue.severity == ValidationSeverity::Error
    }));
    assert_eq!(serde_json::to_vec(&scenario)?, before);
    Ok(())
}
