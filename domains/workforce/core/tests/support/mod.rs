use eutheto_types::ScenarioDocument;
use serde_json::{Value, json};
use std::error::Error;

pub fn id(index: u32) -> String {
    format!("018f7b40-a000-7000-8000-{index:012x}")
}

pub fn fixture() -> Result<ScenarioDocument, Box<dyn Error>> {
    let entities = [
        json!({"kind":"person", "id":id(1), "name":"River", "externalId":"staff-01",
            "activeRange":{"kind":"always"}, "qualificationGrants":[{"qualificationId":id(11)}],
            "eligibleAssignmentTypeIds":[id(4)], "workloadWeight":{"numerator":1,"denominator":1},
            "workloadTarget":{"bucketId":id(3),"calendarId":id(2),"membership":"reportingDate","target":120},
            "tags":["night"], "teamIds":[]}),
        json!({"kind":"calendar", "id":id(2), "name":"Days", "period":{"kind":"day","startTime":"00:00:00"}}),
        json!({"kind":"workloadBucket", "id":id(3), "name":"Hours", "measurement":"elapsedMinutes", "overlappingContribution":"union"}),
        json!({"kind":"assignmentType", "id":id(4), "name":"Clinic", "category":"clinic", "timeBehavior":"elapsed",
            "defaultDurationMinutes":60, "qualifications":{"kind":"matches","allQualificationIds":[id(11)],"anyQualificationIds":[]},
            "locationBehavior":{"kind":"required"}, "workloadBucketIds":[id(3)]}),
        json!({"kind":"location", "id":id(5), "name":"North", "transitions":[]}),
        json!({"kind":"shiftTemplate", "id":id(6), "name":"Sunday Clinic", "assignmentTypeId":id(4), "locationId":id(5),
            "recurrence":{"weekdays":["sunday"],"effectiveRange":{"startDate":"2026-11-01","endDateExclusive":"2026-12-01"},"excludedDates":[]},
            "timing":{"kind":"elapsedDuration","startTime":"01:30:00","durationMinutes":60},
            "coverage":{"kind":"exact","count":1,"qualificationMinimums":[]}, "tags":[], "reportingAttribution":"startLocalDate",
            "occurrenceIdentities":{(id(7)):{"id":id(7),"localStartDate":"2026-11-01"}}}),
        json!({"kind":"shiftInstance", "id":id(8), "assignmentTypeId":id(4), "locationId":id(5),
            "startsAt":{"instant":"2026-11-01T05:30:00Z","local":"2026-11-01T01:30:00","offsetSeconds":-14400},
            "endsAt":{"instant":"2026-11-01T07:30:00Z","local":"2026-11-01T02:30:00","offsetSeconds":-18000},
            "coverage":{"kind":"exact","count":1,"qualificationMinimums":[]}, "tags":[], "reportingAttribution":"startLocalDate", "origin":{"kind":"manual"}}),
        json!({"kind":"scorePolicy", "id":id(9), "profileKey":"clinic", "levels":[{"levelKey":"preferences","label":"Preferences"}],
            "priorityMapping":[
                {"priority":"low","levelKey":"preferences","scale":1}, {"priority":"normal","levelKey":"preferences","scale":10},
                {"priority":"high","levelKey":"preferences","scale":100}, {"priority":"veryHigh","levelKey":"preferences","scale":1000}],
            "tieBreak":"stableAssignmentRank", "workloadPolicies":{(id(10)):{"id":id(10),"bucketId":id(3),"calendarId":id(2),"membership":"reportingDate",
                "peerGroup":{"people":{"kind":"filter","allTags":["night"],"anyTags":[]}}, "targetMode":{"kind":"personTargets"}, "penalty":{"kind":"absolute"}}}}),
        json!({"kind":"qualification", "id":id(11), "name":"Clinician", "description":""}),
    ];
    let entities: serde_json::Map<String, Value> = entities
        .into_iter()
        .map(|value| {
            let key = value["id"]
                .as_str()
                .ok_or("fixture record has no identity")?
                .to_owned();
            Ok((key, value))
        })
        .collect::<Result<_, Box<dyn Error>>>()?;
    Ok(serde_json::from_value(json!({
        "format":"eutheto/scenario", "formatVersion":1, "scenarioId":id(100),
        "domainPack":{"id":"official.workforce","schemaVersion":1},
        "metadata":{"title":"Clinic","description":"","createdAt":"2026-09-01T00:00:00Z","updatedAt":"2026-09-01T00:00:00Z"},
        "settings":{"timeZone":"America/New_York","locale":"en-US","units":"metric",
            "horizon":{"start":"2026-11-01T04:00:00Z","end":"2026-11-02T05:00:00Z"}, "gapPolicy":"reject","overlapPolicy":"reject"},
        "domain":{"entities":entities,"rules":{},"preferences":{},"lockedAssignments":{
            (id(14)):{"id":id(14),"personId":id(1),"shiftId":id(7),"state":{"kind":"hard"}}
        }},
        "extensions":{"nonsemantic.example.annotation":{"note":"Keep this exact extension"}}
    }))?)
}
