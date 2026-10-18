// SPDX-License-Identifier: Apache-2.0
// Report values are data. This source never evaluates report strings as Typst.
#let report = json(sys.inputs.report)
#let schedule = report.content.schedule
#let people = schedule.people.map(p => (p.personId, p)).to-dict()
#let work-types = schedule.assignmentTypes.map(t => (t.entityId, t)).to-dict()
#let locations = schedule.locations.map(l => (l.entityId, l)).to-dict()
#let shifts = schedule.shifts.map(s => (s.shiftId, s)).to-dict()
#set document(title: report.title.text, author: (), date: none)
#set page(
  paper: "a4", margin: 18mm,
  header: text(size: 8pt)[
    Eutheto · Plan revision #report.source.scenarioRevision · Verified feasible · #report.timeZone \
    Result #report.source.solutionId · Generated #report.generatedAt
  ],
  footer: context align(center)[Page #counter(page).display("1 / 1", both: true)],
)
#set text(font: ("Libertinus Serif", "DejaVu Sans Mono"), size: 10pt, lang: "en")
#show regex("\\S{32}"): it => [#it#sym.zws]
#set par(leading: 0.6em)
#set heading(numbering: none)
#let field(name, value) = [#strong(name + ": ")#str(value)#parbreak()]
#let optional(value, fallback: "Not established") = if value == none { fallback } else { str(value) }
#let location(id) = if id == none { "No location" } else { locations.at(id).label.text }
#let work(id) = work-types.at(shifts.at(id).assignmentTypeId).label.text
#let endpoint(e) = {
  let base = e.actual + " (UTC offset " + str(e.resolved.offsetSeconds) + " seconds; instant " + e.resolved.instant + ")"
  if e.actual == e.resolved.local { base } else { base + "; adjusted from requested " + e.resolved.local }
}
#let interval(i) = endpoint(i.startsAt) + " → " + endpoint(i.endsAt)
#let work-facts(id) = {
  let s = shifts.at(id)
  work(id) + "; " + location(s.locationId) + "; " + interval(s.interval) + "; reporting date " + s.reportingDate
}

#text(size: 11pt)[Eutheto · Shared plan]
= #report.title.text
#field("Plan version", report.planVersion)
#field("Time zone", report.timeZone)
Verified feasible for the captured source. This is not a claim of optimality, fairness or coverage satisfaction. This immutable report does not follow later edits.
#if report.source.observedCurrentRevision != none and report.source.observedCurrentRevision != report.source.scenarioRevision [
  *Older accepted plan:* solved revision #report.source.scenarioRevision; current revision was #report.source.observedCurrentRevision when captured.
]

== Complete schedule
#field("Work instances", schedule.shifts.len())
#field("Assignments", schedule.assignments.len())
#field("People", schedule.people.len())
#if schedule.shifts.len() == 0 [This accepted plan has no included work instances.]
#for (index, shift) in schedule.shifts.enumerate() [
  === Work #str(index + 1): #work(shift.shiftId)
  #field("Work identity", shift.shiftId)
  #field("Assignment type identity", shift.assignmentTypeId)
  #field("Actual interval", interval(shift.interval))
  #field("Location", location(shift.locationId))
  #field("Location identity", optional(shift.locationId, fallback: "None"))
  #field("Reporting date", shift.reportingDate)
]

== Complete assignment census
Each entry preserves the assignment identity from the accepted result.
#if schedule.assignments.len() == 0 [No assignments in this accepted plan.]
#for (index, assignment) in schedule.assignments.enumerate() [
  === Assignment #str(index + 1)
  #field("Assignment identity", assignment.assignmentId)
  #field("Person", people.at(assignment.personId).label.text)
  #field("Person identity", assignment.personId)
  #field("Work identity", assignment.shiftId)
  #field("Work", work-facts(assignment.shiftId))
]

== Complete included people
Includes people without assignments.
#for person in schedule.people [
  #field("Person", person.label.text)
  #field("Person identity", person.personId)
]

#if schedule.staffing != none [
  == Staffing actuals
  Assigned headcounts only. Required thresholds, qualification conditions and coverage satisfaction are not disclosed.
  #for row in schedule.staffing [
    #field("Work", work-facts(row.shiftId))
    #field("Work identity", row.shiftId)
    #field("Assigned people", row.assignedCount)
  ]
]

#if schedule.changes != none [
  == Recorded changes
  Pair membership only: retaining a person/work pair does not establish unchanged times, labels or policy. Recorded manual and external starting plans are not authenticated accepted ancestors.
  #if schedule.changes.len() == 0 [No recorded starting-plan comparison in this source.]
  #for comparison in schedule.changes [
    === #(
      recordedOriginal: "Recorded original → current plan",
      recordedManual: "Recorded manual selection → current plan",
      externalStartingPlan: "External starting plan → current plan",
    ).at(comparison.basis)
    #if comparison.original != none {
      let o = comparison.original
      field("Original scenario", o.scenarioId)
      field("Original revision", o.scenarioRevision)
      field("Original result", o.solutionId)
      field("Original checksum", o.acceptedResultChecksum)
      field("Original admission", if o.admission == "freshlyReadmittedNative" { "Freshly re-admitted natively at capture" } else { "Native acceptance not established" })
    }
    #for row in comparison.rows [
      #field("Membership", row.membership)
      #field("Person identity", row.personId)
      #field("Work identity", row.shiftId)
      #if row.previous == none {
        field("Previous facts", "Not in starting pairs")
      } else if row.previous.availability == "unavailable" {
        field("Previous facts", "Historical display facts unavailable")
      } else {
        let f = row.previous.facts
        field("Previous person", f.personLabel.text)
        field("Previous work", f.assignmentType.label.text)
        field("Previous assignment type identity", f.assignmentType.entityId)
        field("Previous location", if f.location == none { "No location" } else { f.location.label.text })
        field("Previous location identity", if f.location == none { "None" } else { f.location.entityId })
        field("Previous actual interval", interval(f.interval))
        field("Previous reporting date", f.reportingDate)
      }
      #field("Current facts", if row.membership == "removed" { "Not in current pairs" } else { people.at(row.personId).label.text + "; " + work-facts(row.shiftId) })
    ]
  ]
]

#if schedule.fairnessActuals != none [
  == Configured workload actuals
  Actual quantities are not an equity assessment. Targets, deviations, weights and preference penalties are excluded. Each measurement retains its full population, including zeros.
  #if schedule.fairnessActuals.len() == 0 [No active configured measurements.]
  #for (index, group) in schedule.fairnessActuals.enumerate() [
    === Measurement #str(index + 1)
    #field("Measure", (assignmentCount: "Assignments", elapsedMinutes: "Elapsed minutes", scheduledMinutes: "Scheduled minutes").at(group.measure))
    #field("Membership", (reportingDate: "Reporting date", startInstant: "Start instant", intersection: "Interval intersection").at(group.membership))
    #field("Overlapping contributions", if group.overlap == "sum" { "Counted separately" } else { "Elapsed union" })
    #for id in group.people [#field("Population member", people.at(id).label.text + " (" + id + ")")]
    #if group.people.len() == 0 [Empty population.]
    #for id in group.qualifyingShifts [#field("Qualifying work", id + "; " + work-facts(id))]
    #if group.qualifyingShifts.len() == 0 [No qualifying work.]
    #if group.periods.len() == 0 [No measurement periods in this captured context.]
    #for period in group.periods [
      #field("Measurement interval", interval(period.interval))
      #for actual in period.actuals [
        #field("Person", people.at(actual.personId).label.text + " (" + actual.personId + ")")
        #field("Actual", if actual.actual.denominator == "1" { actual.actual.numerator } else { actual.actual.numerator + "/" + actual.actual.denominator })
      ]
    ]
  ]
]

== Privacy and disclosure
Stable identifiers and work patterns remain linkable. Generated aliases are pseudonyms, not anonymity.
#field("Person names", if report.privacy.includePersonNames { "Included" } else { "Excluded; generated aliases" })
#field("Authored plan, work and location labels", if report.privacy.includeAuthoredLabels { "Included; labels may contain private information" } else { "Excluded; generated labels" })
#field("Staffing counts", if report.privacy.includeStaffing { "Included; actuals only" } else { "Excluded" })
#field("Historical changes", if report.privacy.includeChanges { "Included" } else { "Excluded" })
#field("Configured workload actuals", if report.privacy.includeFairnessActuals { "Included; complete measurement populations" } else { "Excluded" })
Full addresses, source calendar titles, private notes, constraint details, rejected alternatives, diagnostics, objective details and preference assessments are excluded. Transportation is not available in this report. External links are excluded.

== About this plan
#field("Format", report.format)
#field("Profile", report.profile)
#field("Plan version", report.planVersion)
#field("Scenario", report.source.scenarioId)
#field("Solved revision", report.source.scenarioRevision)
#field("Result", report.source.solutionId)
#field("Current revision at capture", optional(report.source.observedCurrentRevision, fallback: "Not established for supplied file"))
#field("Source", report.source.origin)
#field("Verification", report.source.verification)
#field("Scenario horizon", report.horizon.start + " → " + report.horizon.end)
#field("Time zone", report.timeZone)
#field("Source locale", report.locale)
#field("Generated", report.generatedAt)
#field("Application", report.applicationVersion)
#field("Report schema", report.schemaVersion)
#field("Result schema", report.source.resultSchemaVersion)
#field("HTML renderer", report.renderer.version)
#field("Recorded solver", report.source.solver.backendId + "; " + optional(report.source.solver.recordedVersion, fallback: "Version unavailable"))
#field("Accepted-result checksum", report.source.acceptedResultChecksum)
#field("Verification checksum", report.source.verificationReportChecksum)
#field("Source-document hash", report.source.sourceDocumentHash)
#field("Renderer JavaScript SHA-256", report.renderer.javascriptSha256)
#field("Renderer CSS SHA-256", report.renderer.cssSha256)
Public hashes and recorded solver metadata are descriptive, not independent authentication of an imported report. Intervals are half-open: their end instant is excluded. Actual civil times and offsets are captured natively; reporting dates may use a different authored attribution.
