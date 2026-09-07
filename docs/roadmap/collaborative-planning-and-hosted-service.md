<!-- SPDX-License-Identifier: Apache-2.0 -->

# Collaborative Planning and Hosted Service

## Status, authority, and scope

**Status: roadmap direction approved on 2026-09-07; collaboration and hosted-service implementation remain post-MVP.** This specification reconciles the reviewed *Eutheto: Collaborative Planning and Hosted-Service Feature Additions* proposal dated 2026-09-06. The source `PRODUCT-*` and `ACCEPT-*` identifiers are retained below. This is not a claim that the proposed services, records, deployments or acceptance scenarios are implemented.

The [roadmap index](README.md) and approved ADRs retain their authority. Phases **00–12 remain the public local-first MVP**. Finish Phase05, pause for the user's manual testing, and do not start Phase06 before that pause is resolved. This specification introduces no organization schema, account requirement, campaign UI, server, email provider, billing integration or universal entity framework into those phases. Existing phase/work-package IDs, exit gates and school, Branch-K assistant/voice and Phase14 transportation commitments remain intact.

[Phase13 Branch H](13-post-mvp-roadmap.md#branch-h--collaboration-and-service-mode) owns the collaboration workflow and service-security boundary. Branch N owns managed operation; Branch I owns additional enterprise adapters; Branch O owns portable/share enhancements. This detailed specification is not a second numbered phase sequence. A–F from the source proposal are superseded as delivery groupings by the bounded milestones below, without deleting their reconciled requirements.

The organizational product outcome is **collect → reconcile → solve → independently verify → explain → approve → publish → change → repair → republish**. A workflow that stops at intake is an intake prototype, not the complete pilot. Pricing, commercial packages and provider selection are outside this specification.

## Open software and deployment parity

Eutheto remains open source and free to use locally or self-host. Implemented official software capabilities—including official packs, organizational permissions, applicable identity integrations, audit and repair—must not require a commercial software entitlement. [ADR-003](../adr/003-license-and-contributions.md) continues to govern licensing; this product policy does not relabel third-party code or data, disclose customer data, or invent a different license.

There are no **commercially imposed** local/self-hosted feature or population caps. Documented technical compatibility, bounded-input, model-size, CPU, memory and storage limits remain mandatory. Managed admission/capacity and operating agreements are separate from pack semantics and local software availability. No deployment can promise unlimited resources, guaranteed optimality, legal compliance or institutional suitability without the applicable evidence.

Distinguish three promises:

1. **Semantic parity:** supported deployments use the same versioned domain meaning, compiler/verifier rules, authoritative scoring and accepted-result requirements. Different budgets, hardware and supported backends need not produce identical schedules or runtimes.
2. **Data portability:** supported planning data and results have a practical local-readable exit through the versioned open formats. Imported provenance is not destination authority.
3. **Capability availability and support:** open implementations are usable with their stated runtime prerequisites. Supported deployment/feature combinations have explicit evidence; source availability alone is not an operating guarantee.

| Mode | Required boundary |
|---|---|
| Standalone desktop/library/CLI | Core workflows need no vendor account, license check, hosted solver, telemetry or network. Do not add a permanent public server to the desktop installer merely for parity. |
| Self-hosted shared deployment | Open collaboration software with a documented, tested reference deployment, explicit identity/permissions, connected-provider configuration and operator responsibilities. Start with one supported topology. |
| Managed collaboration | The same official software capabilities operated with additional service admission, backups, monitoring, updates and support. Managed guarantees require their own operating evidence. |
| Hybrid customer worker | Deferred enrollment, connectivity, custody and synchronization work; a sleeping or disconnected worker is not continuously available capacity. |

Maintain a versioned deployment/capability matrix using supported, experimental, not applicable, unavailable and research states with prerequisites and evidence. Public portals and internet email require a reachable connected runtime; disconnected local use may remain manual. Do not introduce an offline campaign-exchange format merely to make every interface look identical.

## Reuse of existing authorities

Phase ownership below identifies contracts, not proof that all associated features are complete today.

| Concern | Authority to reuse | Collaboration addition |
|---|---|---|
| Scenario meaning and edits | Pack-owned records plus the existing typed, validated, revision-checked command/batch and undo contract; Phases01/02/05–09 | A submission/proposal is non-authoritative. Acceptance emits ordinary commands and records the accepted source/mapping decision atomically. No parallel constraint database. |
| Immutable inputs and execution | Existing scenario snapshots, run inputs, solver router/worker and terminal manifests; Phases01–04 and their domain integrations | Bind the authorized organization/job context outside the mathematical model. Do not duplicate snapshots or invent a second solve authority. |
| Feasibility and scoring | [ADR-007](../adr/007-independent-solution-verification.md), original-domain independent verification | Human approval adds governance; it cannot waive verification or turn an invalid candidate into an accepted solution. |
| Comparison and repair | Phase07/09 accepted-result comparison, locks, typed edits and pack-defined disruption policy | Requests reference a publication baseline and undergo current authorization, revision checks and revalidation. |
| Recipient outputs | Existing Result/Share Result and exact privacy-preview contracts | An immutable publication references an accepted result and its authorized recipient projection. It is not another assignment store. |
| History | [ADR-011](../adr/011-command-journal-and-undo.md) command journal, snapshots and explicit irreversible actions | Minimal privileged audit and durable notification handoff; no distributed event-sourcing prerequisite. |
| Portability | [ADR-018](../adr/018-public-scenario-representation.md) and [portable-data specification](portable-data-backup-and-sharing.md) | Separately versioned, scoped organization migration/recovery where required; no cloud-only mathematical format. |

`SubmissionRevision`, `ConstraintProposal`, `Approval` and `PublishedPlanVersion` are proposed collaboration concepts, not current schema declarations. A proposed `ConstraintRevision` references accepted pack meaning at a scenario revision; `CandidatePlan` may be a planner-facing reference to an existing verified result. Add a record only when the first producing/consuming workflow proves its need. Do not retrofit nullable tenant, campaign, approval or requested-severity fields into all current records.

## Identity, input authority, and policy

### Identity and access

A planning person/resource is not a login account. Use existing pack identities for the initial Workforce workflow. Organization, workspace, campaign, planning period, subject relationship, operation and field scope all participate in authorization; an organization-wide role name is not sufficient.

Account-to-subject and delegated-subject links require a defined granting authority, origin/trust level, correction, unlink and revocation behavior. Matching names, email addresses or imported UUIDs never establish identity or grant access. Changing a contact destination does not silently transfer a queued private disclosure to its new owner.

The same real-world person may have immutable versions of planning facts in multiple scenarios. A directory update cannot rewrite old inputs/results/publications. Later cross-pack linking uses explicit, tenant-scoped, approved mappings and preserves snapshot boundaries; it grants no implicit permissions, constraint transfer or coordinated solve. The [handoff → conflict detection → coordinated-solving research gate](13-post-mvp-roadmap.md#platform-research-gates) remains binding. No universal shared-person/resource model is introduced here.

Start with participant, planner and optionally separate approver/publisher permissions. Owner, administrator, viewer and machine roles are only as broad as the selected workflow requires. Separate organization ownership from sensitive-field access and operator infrastructure custody. Local households require neither accounts nor enterprise approval ceremonies.

### Intake and effective meaning

Start with one constrained, reusable Workforce intake template, not a universal survey/form engine. Typed questions declare supported pack mappings, scope, units, period/timezone, required/optional status and permitted treatment. Free text is a request for interpretation, never executable policy or an ignored field falsely presented as enforced. Unsupported semantics remain explicit review/unavailable states.

Keep these independent:

- requested treatment;
- source authority and approval state;
- accepted effective Required/Preference meaning;
- amendment authority, policy basis and overridability.

Retain **Required / Preference** and the existing versioned pack score policy. Phase07's Low/Normal/High/Very high preference labels are not replaced by a competing generic severity enum. Strict objective priorities use the established lexicographic contract, not arbitrary large coefficients. Required violations always reject a candidate; an authorized amendment changes the input and requires verification of a new candidate rather than silently relaxing a protected rule.

Distinguish reported factual unavailability, a leave request requiring approval and a preferred day off. Missing or unapproved information is not automatically availability. The campaign explicitly defines missing, partial, late and post-deadline responses, effective dates and participant revision rights; the interface states what was received, accepted, pending or superseded. Deadlines use a recorded timezone/DST policy and trusted receiving-time rule, not a participant-supplied clock. New participants enter a deliberate campaign/roster revision.

Bind reconciliation and automatic acceptance to exact submission, template, deterministic mapping/policy and campaign/roster revisions; subject/period scope; proposed effective values; and current accepting authority. Recheck those bindings at the ordinary atomic command commit. A changed mapping creates new reviewed proposals, not a reinterpretation of accepted history. Retry must not apply the same acceptance twice or detach provenance from its effects.

Equivalent inputs, split intervals, repeat submissions and overlapping campaigns must not amplify one person's objective influence. Define normalization, replacement/supersession and bounded per-subject aggregation in the selected pack policy. Do not count historical submission revisions as additional preference votes. Fairness retains explicit population, period, targets/weights, exclusions, units and raw tradeoffs; it is not a moral verdict or universal percentage.

### Invitations

Invitations are high-entropy, scoped, expiring and revocable capabilities. Store verifiers, not reusable plaintext tokens. Specify issued/redemption/expiry/reissue/revocation states and deliberately chosen one-time or repeat-use behavior. Accountless possession is not independently verified personal identity; stronger authentication is required where the action/data risk demands it.

An invitation grants only its defined campaign/subject actions, not general workspace access. Separately authorize submitting, revising and reading saved answers or historical data. Revocation, expiry and reissue affect derived sessions and already-open tabs; acceptance checks authority at commit. No automatic general-account linking follows redemption.

GET, email security scanning and link previewing must not submit, consume the only intended action, publish or imply acknowledgment. Exclude tokens from logs, analytics, referrers and ordinary exports. Do not use tracking pixels or ambiguous open events as participant response/receipt evidence. Define abuse/rate limits, origin/session protections and recovery before exposing public forms.

## Verified approval and publication

Use distinct vocabulary:

- **Backend candidate:** not yet trusted.
- **Verified solution:** independently accepted against an exact original input snapshot.
- **Approved plan:** authorized review of that exact verified result under an identified policy.
- **Published version:** immutable, authorized distribution of a recipient projection.

A local selected result or generated result capsule is not automatically an institutional approval/publication. Approval references the accepted result, input/scenario revision, verifier binding and policy version. Relevant candidate/input, authority, policy or disclosure changes require appropriate re-review. Historical decisions remain historical; they cannot authorize changed content. New working inputs do not silently edit or replace the last published version, and a material change can mark that version as needing review.

The publication operation checks current publisher authority, exact candidate/input and approval bindings, expected current-publication version and permitted distribution/projection. It atomically creates the immutable publication, advances the current pointer, and records minimal audit plus the durable notification handoff. Artifact preparation may precede that transaction, but uncommitted private staging must not become public. Do not promise a distributed transaction with an email provider or remote object store.

A retry with the same idempotency identity and semantic request returns the original publication; different content conflicts. Concurrent publishers cannot create ambiguous current state. Expanding disclosed fields requires authorization/review even if the assignments are unchanged. Publication is not ordinary Undo: correction, supersession or withdrawal is explicit and cannot erase already delivered files.

The first pilot includes one changed-availability request and planner-mediated repair/republication using the completed domain repair service. Validate qualifications, availability, coverage, rest and other applicable original rules, respect protected locks, and show a meaningful disruption diff. A swap/preflight result is revision-bound and revalidated at acceptance; matching job titles is insufficient. A broader participant swap marketplace, generalized change framework and additional integrations are later evidence-led work.

## Privacy, retention, and recovery

Collect scheduling consequences rather than unnecessary reasons. Separate contact details, raw responses/private explanations, accepted normalized facts, authority lineage and distributed assignments. Even normalized availability, relationship constraints, conflict membership, small-group metrics and repeated counterfactual results may be sensitive.

Authorize the requested query and permitted derived result before building a view or AI context. Initial participants receive curated personal/authorized views, not unrestricted diagnostic access to other subjects. Do not falsify mathematics to conceal private data; omit protected detail with honest limited-evidence wording. An authorized schedule inherently discloses assignments: state that residual disclosure instead of promising zero inference. Check encoded values, IDs/correlation, evidence, caches and embedded HTML/PDF data, not only visible prose.

| Data class | Required policy before production use |
|---|---|
| Contacts, raw answers, optional reasons and drafts | Purpose, subject access, permitted readers, retention/expiry, correction/deletion, export inclusion and handling in backups. No indefinite retention by accident. |
| Accepted normalized facts and immutable run inputs | Exact versions needed for retained verification and comparisons, compatibility window and the effect of deleting required facts. |
| Minimal decision/provenance/audit | Source revision identity, accepted interpretation, actor/policy origin and necessary timestamps; not a duplicate store of sensitive source content. Imported claims remain untrusted history. |
| Approvals and publications | Retained immutable identity/content or explicit tombstone/withdrawal, distribution history and the limits of offline-copy deletion. |
| Delivery, provider and integration data | Minimal safe status, bounded retries/logs, destination handling, expiry and exclusion of reusable secrets. |

Deleting raw material need not erase the accepted normalized interpretation or minimal decision lineage. Reinterpretation is possible only while the raw source is legitimately retained. If deletion removes facts required for historical re-verification, disclose that loss; do not fabricate reproducibility, provenance or an inverse. Undo/replay must not resurrect expired private answers. A local audit file is not tamper-proof against its machine owner; managed integrity assurances need an explicit design and evidence.

Keep four purposes distinct: local-readable planning interchange; recipient result sharing; authorized organization migration; deployment disaster recovery. The proposed `.eutheto` extension is not finalized by this document. New organization transfer/profile versions must declare sensitive categories and explicit destination identity/authority mapping; ordinary scenario archives never carry sessions, reusable invitations, provider secrets or automatic privilege grants. Checksums prove consistency, not the authority of an imported approval.

Restoration enters a safe recovery state before public/network operation. Reconcile current revocations and retention/deletion policy, invalidate or deliberately re-enroll old sessions/invites/workers/shares, and do not automatically replay pending jobs or sends. Restored historical publication state must not silently claim to supersede a newer distributed version. Test restores have no production delivery credentials and cannot contact participants. If an older backup cannot satisfy a later revocation/deletion guarantee, make recovery explicit before access resumes rather than claiming that snapshots alone solve it.

Offline capsules contain only the exact permitted Share Result payload and remain useful without a server. A downloaded file cannot be remotely revoked or automatically know a newer schedule exists. Hosted links may be access-controlled, revoked and expired; revoking a link does not recall downloaded copies. Hosted capacity limits must leave a practical supported export/retrieval path, with staged or alternate transfer when necessary rather than unlimited-ingestion promises.

## Notifications and integration boundaries

Use committed application events → current recipient/permission resolution → minimal durable intent/outbox → safe projection → configured delivery adapter → recorded outcome. This is a module boundary, not a microservice or event-sourcing mandate. Start with email and visible in-app workflow/status behavior required by the pilot. An in-app state view does not require a generalized inbox. Reminders, quiet hours and digest policies are implemented only to the extent the chosen pilot requires them.

Bind logical intent/idempotency and source object/version to the committed change. Recheck recipient relationship, destination, invitation/share validity and disclosure rights before dispatch and retries. Prefer minimal email plus an authorized portal link for private details. Suppress/coalesce obsolete reminders where appropriate; delayed version N must never be described as current after N+1 is published.

Distinguish queued, provider accepted, confirmed delivered where supported, failed/bounced, and explicit acknowledgment. Provider acceptance followed by a process crash has an ambiguous external outcome: use bounded retries, stable intent/delivery identities and visible reconciliation, not an exactly-once claim or uncontrolled resend loop.

SMS, push/chat adapters, generalized channel preference engines, Managed AI and hybrid workers remain deferred. A small channel boundary is sufficient preparation; do not build unused settings, adapters or charging structures. Select the first provider/configuration approach through the pilot decision gate, not this roadmap revision.

AI remains optional and operates through the existing bounded proposal/confirmation boundary. It cannot approve, publish, export, choose privacy/credentials, alter permissions or become solver/verifier authority. Preserve the already-approved Phase10 and Branch-K ownership; consumer subscription/OAuth support remains conditional on official suitable authorization. Opening/importing/restoring a scenario never activates a provider, probe, tunnel or telemetry.

Before any hosted credential/provider implementation, approve deployment-specific custody and egress decisions: who may enter/use/rotate/revoke tenant credentials, the appropriate secret facility, browser/session boundaries, allowed destinations and redirects/resolved addresses, and prevention of access to infrastructure metadata/internal services. Preserve desktop [ADR-010](../adr/010-local-state-and-credentials.md) and [ADR-014](../adr/014-provider-authentication.md); do not silently relax native-only desktop custody for a browser service. A laptop-local endpoint is not a cloud-local endpoint.

Use a separately authenticated service API, never exposed Tauri IPC. Worker jobs bind the authorized tenant, immutable snapshot, request and enrolled execution context; arbitrary payload URLs or claimed identities do not confer network authority. Solver workers receive only the required mathematical representation, not contacts, private reasons or provider credentials. Additional roster/calendar/provider integrations begin read-only when sufficient and require versioned mappings, provenance, disclosure, conflict policy and safe retry/removal; write-back is separately authorized.

## Delivery and ownership

Milestones below have independent evidence, not calendar promises. Read-only discovery may happen in advance, but no later production capability moves into the current MVP. Independent eligibility does not promise simultaneous staffing or silently reprioritize school, voice or transportation.

| Milestone | Entry and bounded outcome | Exit |
|---|---|---|
| **H0 — reuse and pilot charter** | Read-only mapping of existing authorities, chosen workflow/domain reviewer, deployment and support assumptions. Final implementation contracts reconcile with the completed Phase12 baseline. No speculative schema/framework work. | Named decision owners and evidence; supported input/authority/privacy policy; measurable go/stop criteria; relevant ADR work identified. No customer/scale/provider choice is presumed. |
| **H1 — complete collaboration pilot** | Completed Phase12, H0 and approved applicable service/data-integrity/security decisions. One supported Workforce workflow, simple organization/workspace arrangement, constrained template, minimum permissions, documented self-hostable reference deployment and connected notifications. | The entire collect→reconcile→solve→independently verify→explain→approve→publish→change→repair→republish loop, local export and representative restore pass with participant/planner accessibility and all applicable adversarial gates. No intake-only pilot claim. |
| **N1 — managed collaboration operation** | H1 open capabilities plus managed deployment, capacity, security and operating readiness. Hosting does not introduce exclusive official software features. | Restore-tested backups, controlled updates/recovery, isolation, health/incident responsibility, retention/deletion, delivery failure handling, supported workload and practical export/exit are evidenced before production data. |
| **H2 / I / N expansion** | Repeated-cycle pilot evidence and an explicitly approved need. | Broader campaigns/policies, organizations, SSO/directory/calendar adapters, dedicated deployments or institutional scale complete their own scoped contracts and operating gates. None becomes a paid software unlock. |
| **Demand-led investigations** | Specific need, feasible operating model, privacy/security review and approved scope. | Individually justified Managed AI, SMS/additional channels, hybrid execution, external APIs or cross-pack research; no release promise follows merely from listing them. |

H1 itself requires minimum production operations even when self-hosted or called a pilot: isolation and backend authorization, bounded/cancellable work, credential custody, monitored health, migration/update recovery, tested backups/restores, retention/deletion, mail failure handling, incident ownership and supported-use limits. N1 adds the managed operator's evidence and responsibilities; it does not postpone baseline safety until after H1 has accepted real data. Without those gates, use a clearly non-production prototype with synthetic data.

Within H1, order coherent work packages: service/identity and subject-scoped access; versioned intake/proposals and atomic acceptance; reuse of verified solve/compare/repair; approval/publication/recipient views and durable notification handoff; then the complete repeated-cycle/repair/export/restore acceptance. Security and publication correctness are present at first use, not a later replacement of a knowingly weak path.

H is the owner of applicable service controls, not a requirement that every remote service implement campaigns. Other H/N deployments and remote Branch-K integrations must satisfy their relevant identity, authorization, concurrency, quota, privacy and operating gates, but do not inherit the entire H1 product workflow. Static docs/update hosting retains its own applicable release/security boundary. Local Branch-K and Phase14 entry rules remain unchanged.

## Product decision traceability

These adopted dispositions preserve every source identifier; they do not claim implementation or change the local-MVP scope.

| Source ID | Reconciled requirement | Ownership |
|---|---|---|
| PRODUCT-01 | Open local/self-hosted official functionality; no commercial feature/population caps; retain technical safety/resource limits. | Index principles; all releases. |
| PRODUCT-02 | Standalone core workflows require no vendor account or cloud contact. | Existing Phases00–12; regression gate for H/N. |
| PRODUCT-03 | Managed convenience/capacity/operations stay outside pack and solver licensing semantics. | N1 and applicable hosted profiles. |
| PRODUCT-04 | Implemented official packs have no commercial entitlement check; compiled/sandboxed loading gates remain. | Existing pack/release contracts; H/N deployment evidence. |
| PRODUCT-05 | Open versioned planning portability and practical local exit; no proprietary cloud format. | Portable specification, H1/N1, Branch O. |
| PRODUCT-06 | Typed participant intake with reconciliation becomes a complete post-MVP workflow. | H1. |
| PRODUCT-07 | Requested treatment, accepting authority and effective model meaning are distinct. | H1 mapping policy; existing Required/Preference semantics. |
| PRODUCT-08 | Reuse provenance, verified explanations and immutable result identities; add exact governance/publication bindings. | Existing Phases01–04/07/09; H1 additions. |
| PRODUCT-09 | Minimize private reasons and apply privacy to normalized/derived data, exports and retention. | Existing privacy gates; H1/H/N/O extensions. |
| PRODUCT-10 | Optional user-configured AI proposes typed changes; independent verification remains authority. | Phase10/Branch K; applicable H/N controls. |
| PRODUCT-11 | Managed AI is demand-led investigation, not an initial organizational service. | Deferred Branch N/K investigation. |
| PRODUCT-12 | No initial SMS provider, enrollment, settings or release promise. | Deferred channel investigation. |
| PRODUCT-13 | Committed event/policy/delivery separation; implement only channels and shared concerns needed now. | H1 email/status; later scoped adapters. |
| PRODUCT-14 | Institutional scale is an evidence gate, not the first mandatory customer. | H0 validation; later I/N expansion. |
| PRODUCT-15 | Prefer configuration and reusable supported semantics; no customer forks or unrestricted custom logic. | H0/H1 product validation and pack rule discipline. |

## Acceptance traceability

**Ownership is not completion.** Preserve all source `ACCEPT-01`–`ACCEPT-22` identifiers. Existing local contracts need their own phase evidence; deployment-specific behavior needs fresh evidence for that release. None is marked passed by this roadmap adoption.

| Source ID | Reconciled passing behavior | Owning gate |
|---|---|---|
| ACCEPT-01 | Fresh standalone offline create/edit/solve/verify/inspect/export/reopen, no vendor account. | Phases11–12; H/N regression. |
| ACCEPT-02 | Supported self-hosted official pack functionality has no commercial unlock, with actual deployment evidence. | H1/N1; existing licensing/loading gates. |
| ACCEPT-03 | Each participant can access only the authorized form, subject, period and actions. | H1. |
| ACCEPT-04 | Impermissible requested hard treatment remains a request/review item or is rejected; never silently becomes Required. | H1. |
| ACCEPT-05 | Authorized automatic acceptance commits the exact validated interpretation and policy provenance once. | H1. |
| ACCEPT-06 | Conflicting Required rules yield truthful proven-infeasible versus limited/unknown state; no silent weakening or invented minimal core. | Existing domain/solver gates; H1. |
| ACCEPT-07 | Missing information and the configured assumption/default/review policy are explicit, not implied consent. | H1. |
| ACCEPT-08 | Revised submission supersedes explicitly, preserves minimal revision/decision lineage under retention policy and invalidates affected pending approvals; no mandatory indefinite raw-answer retention. | H1. |
| ACCEPT-09 | Comparison flags different inputs and incompatible score policies; raw unlike objectives are not a common quality scale. | Phase07/09 and applicable Phase10/13; H1 reuse. |
| ACCEPT-10 | AI disabled/unavailable leaves deterministic editing, verification, explanations and applicable publication usable. | Phase10–12; H1/N1. |
| ACCEPT-11 | Participant payloads and derived queries expose only authorized data/consequences; private reasons, relationships and diagnostic inference are not hidden merely by wording. | Existing Share Result privacy; H1/H/O derived access. |
| ACCEPT-12 | Changed publication/input baseline invalidates swap preflight; acceptance revalidates current authorized changes. | H1 planner-mediated change; expanded swaps H2. |
| ACCEPT-13 | Protected assignments remain protected; impossible repair requires authorized input/lock amendment or truthful no-result. | Phase07/09; H1 reuse. |
| ACCEPT-14 | Restart/ambiguous provider outcome cannot create an uncontrolled resend loop or duplicate logical publication; delivery status is truthful. | H1/N1. |
| ACCEPT-15 | Revocation/reissue/expiry removes invitation and derived-session authority, including old-tab read/write attempts. | H1. |
| ACCEPT-16 | Recipient offline capsule works without network and contains no hidden unauthorized source data; revocation/freshness limits are explicit. | Phase07/09/11–12; H1/O. |
| ACCEPT-17 | Supported hosted planning semantics/results export and open locally with compatibility reporting and no imported authority. | H1/N1/O; existing portable conformance. |
| ACCEPT-18 | Unsupported required pack/schema meaning is not silently discarded or presented as equivalent. | Existing portable/pack gates; H1/N1. |
| ACCEPT-19 | Imported identity/approval/provider claims require destination mapping/authorization and convey no reusable secrets, membership or privilege. | H1/N1 organization transfer and recovery profiles. |
| ACCEPT-20 | Hosted service limits give actionable status and preserve practical supported export/retrieval; local features remain unlocked. | N1. |
| ACCEPT-21 | SMS and Managed AI are accurately absent/deferred in the initial organizational capability/release inventory; ordinary workflows do not depend on them. | H1/N1 release scope. |
| ACCEPT-22 | Representative restore recovers compatible planning/revision/publication meaning while reconciling revocation/deletion and preventing automatic jobs/sends. | Existing local recovery gates; separate H1/N1 service recovery. |

## Additional adversarial acceptance

The H1/N1 evidence index must include these negative/transition scenarios, in addition to the complete golden workflow and applicable existing tests:

| ID | Required proof |
|---|---|
| COLLAB-ADV-01 | Same-tenant other-subject and cross-workspace/tenant ID substitution fails for reads, writes, search/count, evidence, jobs, exports and recipient resolution. Shared email never auto-merges or grants authority. |
| COLLAB-ADV-02 | Forwarded/scanned links, racing redemption, expiry/reissue and revoke-versus-submit cannot create broad access, false acknowledgment, duplicate active revisions or usable revoked sessions. |
| COLLAB-ADV-03 | Policy/template/mapping/roster changes during review invalidate stale interpretation; accepted commands and source lineage commit together or neither does. |
| COLLAB-ADV-04 | Equivalent/split/replayed submissions and overlapping campaigns cannot multiply preference influence; unsupported free-text meaning is never presented as enforced. |
| COLLAB-ADV-05 | False backend feasibility/objective, mismatched snapshot/result evidence and unverified candidates cannot be approved or published as verified. |
| COLLAB-ADV-06 | Concurrent publishing, stale approvals, revoked publishers and changed disclosure scope cannot advance the wrong current version; idempotent retry returns the same publication. |
| COLLAB-ADV-07 | Crash around publication commit preserves a consistent publication/audit/outbox set; rolled-back state sends nothing. Provider-accepted-but-unrecorded delivery remains bounded and honestly classified. |
| COLLAB-ADV-08 | Recipient/contact revocation or campaign closure while mail is queued prevents stale private disclosure; delayed publication N is not presented as current N+1. |
| COLLAB-ADV-09 | Protected-reason/relationship variations and repeated diagnostic queries do not reveal unauthorized data through text, small-group metrics, IDs/correlation, caches, embedded reports or AI context. Record permitted residual schedule disclosure. |
| COLLAB-ADV-10 | Expired raw private content does not reappear through undo, replay, export or restore; retained normalized facts/decision lineage remain coherent, with lost re-verification ability disclosed when necessary. |
| COLLAB-ADV-11 | Snapshot→revoke/delete→restore cannot silently resurrect sessions/invites/workers/shares or deleted data, dispatch old jobs/sends or claim stale publication state is current. Test restores never contact real recipients. |
| COLLAB-ADV-12 | Tenant-selected endpoint/redirect/internal address, malicious provider/submission text and imported configuration cannot expand network, AI, credential, permission or publication authority. |
| COLLAB-ADV-13 | Stale/cross-tenant/revoked-worker completions and cancellation/commit races cannot publish invalid/current-looking results or corrupt source inputs. |
| COLLAB-ADV-14 | Keyboard/screen-reader participant and planner flows, a usable supported mobile-browser participant surface, late/missing-response states and repeated-cycle operation pass without an engineer rewriting code/model semantics. No native mobile app is implied. |

## Pilot validation and decision gates

Before selecting H1 production scope, record a pilot charter with a named planning owner and domain reviewer; one recurring workflow and representative synthetic corpus; supported population/horizon/rule and deployment envelope; exact intake/authority/missing/late-response policies; participant authentication and privacy needs; supported output/change path; chosen email/configuration approach; retention and recovery responsibilities; and appropriate legal/operational review. A physician group is a candidate workflow, not a confirmed market or safety claim. Households, large hospitals/universities and other packs do not all become launch requirements.

Define baselines and go/stop thresholds before enrolling production participants: time to first useful setup, response completion/correction, reconciliation/manual transcription effort, valid-result and explanation usefulness, recurring operation without engineer changes, repair disruption, restore success and support effort. Choose values from representative evidence; this document invents no customer-demand, scale, latency or service guarantee. Local measurements do not require exporting sensitive planning data or adding telemetry.

Require ADR decisions for applicable service identity/tenancy/subject links and sessions; intake/approval/publication concurrency; deployment-specific state/credential/egress custody; and migration/retention/security recovery. Keep approved desktop/domain ADRs in force; explicitly document any genuine supersession rather than weakening them by implication. Technical design follows demonstrated workflow needs, not the source proposal's record/service inventory as a mandatory database layout.

## Implementation orchestration

The primary owns scope, phase order, authority/schema decisions, migration policy, integration and evidence. Use one coherent vertical pilot plan after H0; delegate only bounded independently executable slices against landed contracts. One integration owner serializes identity/publication/format and shared persistence changes. UI/UX specialization is appropriate for participant/planner accessibility and state design; security and independent correctness review are required before real-data deployment.

Do not create permanent microservices, queues, generic workflow engines, registries or provider frameworks merely because the logical responsibilities have names. Reuse the Rust application/command/verifier and worker boundaries; use a modular application and one supported topology until measured operating requirements justify more. Each milestone records exact artifact/deployment/version evidence, migrations, negative tests, operating procedures and limitations before its own release claim. No code or schema adjustment to the existing local MVP follows solely from adopting this roadmap direction.
