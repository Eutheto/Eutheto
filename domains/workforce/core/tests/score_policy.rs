use eutheto_domain_ir::MAX_SCORE_LEVELS;
use eutheto_workforce::model::{
    DeviationPenalty, PeerGroup, PenaltyBreakpoint, PersonSelection, PersonTarget,
    PreferencePriority, PriorityMapping, ScoreLevel, TieBreakPolicy, WindowMembership,
    WorkforceScorePolicy, WorkloadPolicy, WorkloadTargetMode,
};
use eutheto_workforce::validation::{MAX_WEIGHT, validate_score_policy_shape};
use std::{collections::BTreeMap, error::Error};

fn policy() -> Result<WorkforceScorePolicy, Box<dyn Error>> {
    Ok(WorkforceScorePolicy {
        id: "018f7b40-a000-7000-8000-000000000001".parse()?,
        profile_key: "clinic".to_owned(),
        levels: vec![ScoreLevel {
            level_key: "preferences".to_owned(),
            label: "Preferences".to_owned(),
        }],
        priority_mapping: [
            PreferencePriority::Low,
            PreferencePriority::Normal,
            PreferencePriority::High,
            PreferencePriority::VeryHigh,
        ]
        .into_iter()
        .map(|priority| PriorityMapping {
            priority,
            level_key: "preferences".to_owned(),
            scale: 1,
        })
        .collect(),
        workload_policies: BTreeMap::new(),
        tie_break: TieBreakPolicy::StableAssignmentRank,
    })
}

fn workload() -> Result<WorkloadPolicy, Box<dyn Error>> {
    Ok(WorkloadPolicy {
        id: "018f7b40-a000-7000-8000-000000000002".parse()?,
        bucket_id: "018f7b40-a000-7000-8000-000000000003".parse()?,
        calendar_id: "018f7b40-a000-7000-8000-000000000004".parse()?,
        membership: WindowMembership::ReportingDate,
        peer_group: PeerGroup {
            people: PersonSelection::All {},
            team_ids: None,
        },
        target_mode: WorkloadTargetMode::Explicit {
            targets: vec![PersonTarget {
                person_id: "018f7b40-a000-7000-8000-000000000005".parse()?,
                target: 120,
            }],
        },
        penalty: DeviationPenalty::Piecewise {
            breakpoints: vec![
                PenaltyBreakpoint {
                    from_deviation: 0,
                    slope: 1,
                },
                PenaltyBreakpoint {
                    from_deviation: 60,
                    slope: 2,
                },
            ],
        },
    })
}

#[test]
fn priority_mapping_is_total_unique_and_references_declared_levels() -> Result<(), Box<dyn Error>> {
    let original = policy()?;
    validate_score_policy_shape(&original)?;
    let mut duplicate = original.clone();
    duplicate.priority_mapping[3].priority = PreferencePriority::Low;
    assert!(validate_score_policy_shape(&duplicate).is_err());
    let mut unknown_level = original;
    unknown_level.priority_mapping[0].level_key = "undeclared".to_owned();
    assert!(validate_score_policy_shape(&unknown_level).is_err());
    Ok(())
}

#[test]
fn policy_leaves_capacity_for_rank_and_checks_scale_boundaries() -> Result<(), Box<dyn Error>> {
    let mut value = policy()?;
    for index in 1..MAX_SCORE_LEVELS - 1 {
        value.levels.push(ScoreLevel {
            level_key: format!("level-{index}"),
            label: format!("Level {index}"),
        });
    }
    value.priority_mapping[0].scale = MAX_WEIGHT;
    validate_score_policy_shape(&value)?;
    value.levels.push(ScoreLevel {
        level_key: "overflow".to_owned(),
        label: "Overflow".to_owned(),
    });
    assert!(validate_score_policy_shape(&value).is_err());
    value.levels.pop();
    value.priority_mapping[0].scale = MAX_WEIGHT + 1;
    assert!(validate_score_policy_shape(&value).is_err());
    value.priority_mapping[0].scale = 0;
    assert!(validate_score_policy_shape(&value).is_err());
    Ok(())
}

#[test]
fn piecewise_penalties_order_starts_without_requiring_convexity() -> Result<(), Box<dyn Error>> {
    let mut value = policy()?;
    let original = workload()?;
    value
        .workload_policies
        .insert(original.id, original.clone());
    validate_score_policy_shape(&value)?;
    for breakpoints in [
        vec![PenaltyBreakpoint {
            from_deviation: 1,
            slope: 1,
        }],
        vec![
            PenaltyBreakpoint {
                from_deviation: 0,
                slope: 1,
            },
            PenaltyBreakpoint {
                from_deviation: 0,
                slope: 2,
            },
        ],
    ] {
        let mut invalid = original.clone();
        invalid.penalty = DeviationPenalty::Piecewise { breakpoints };
        value.workload_policies.insert(invalid.id, invalid);
        assert!(validate_score_policy_shape(&value).is_err());
    }
    for slopes in [[2, 1], [0, 0]] {
        let mut explicit = original.clone();
        explicit.penalty = DeviationPenalty::Piecewise {
            breakpoints: vec![
                PenaltyBreakpoint {
                    from_deviation: 0,
                    slope: slopes[0],
                },
                PenaltyBreakpoint {
                    from_deviation: 60,
                    slope: slopes[1],
                },
            ],
        };
        value.workload_policies.insert(explicit.id, explicit);
        validate_score_policy_shape(&value)?;
    }
    Ok(())
}

#[test]
fn workload_definitions_require_matching_ids_targets_and_explicit_population()
-> Result<(), Box<dyn Error>> {
    let mut value = policy()?;
    let original = workload()?;
    value
        .workload_policies
        .insert(value.id.try_into()?, original.clone());
    assert!(validate_score_policy_shape(&value).is_err());
    value.workload_policies.clear();
    let mut duplicate_targets = original.clone();
    if let WorkloadTargetMode::Explicit { targets } = &mut duplicate_targets.target_mode {
        targets.push(targets[0]);
    }
    value
        .workload_policies
        .insert(duplicate_targets.id, duplicate_targets);
    assert!(validate_score_policy_shape(&value).is_err());
    let mut empty_population = original;
    empty_population.peer_group.people = PersonSelection::Selected { person_ids: vec![] };
    value
        .workload_policies
        .insert(empty_population.id, empty_population);
    assert!(validate_score_policy_shape(&value).is_err());
    Ok(())
}
