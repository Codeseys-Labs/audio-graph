#!/usr/bin/env bun

/**
 * THROWAWAY LOGIC PROTOTYPE — audio-graph-5e41. Never import into production.
 *
 * Question: can one receipt-bearing projection admission model keep Pending
 * work invisible until durable acceptance while exact retry, restart, Session
 * replacement, deletion, and independent Notes/Graph jobs remain safe?
 *
 * Assumptions: the model is finite, in-memory, metadata-only, and deliberately
 * smaller than the production filesystem/provider implementation. It treats a
 * newly created canonical stream as durable only after file and parent-directory
 * barriers. Remote side effects cannot be inferred after a process loses an
 * in-flight request. The three product policies remain parameters, not decisions.
 *
 * Run: bun scripts/prototype-session-projection-admission.mjs
 */

import assert from "node:assert/strict";

const LANES = ["Notes", "Graph"];
const RECEIPTS = [
  "Pending",
  "Accepted",
  "AlreadyAccepted",
  "Rejected",
  "OutcomeUncertain",
];
const CUTS = [
  "BeforeEnqueue",
  "AfterEnqueue",
  "AfterWrite",
  "AfterFlush",
  "AfterFileSync",
  "AfterDirectorySync",
  "AfterAck",
];
const STREAM_KINDS = ["Existing", "New"];
const RESULT_KINDS = ["Success", "Failure"];

const policyProfiles = [];
for (const savedWording of ["Saved", "Durably saved"]) {
  for (const remoteReissue of ["RequireDecision", "AutomaticAtRisk"]) {
    for (const deletion of ["DiscardImmediately", "WaitForRemote"]) {
      policyProfiles.push({ savedWording, remoteReissue, deletion });
    }
  }
}

let transitionCount = 0;
let caseCount = 0;
let assertionCount = 0;
const invariantFamilies = new Set();
const observedReceipts = new Set();
const observedStates = new Set();

function invariant(condition, family, message, state = undefined) {
  invariantFamilies.add(family);
  assertionCount += 1;
  if (condition) return;

  const detail = state === undefined ? "" : `\nFull state:\n${JSON.stringify(state, null, 2)}`;
  throw new Error(`[${family}] ${message}${detail}`);
}

function modelGuard(condition, code, message, state = undefined) {
  if (condition) return;
  const detail = state === undefined ? "" : `\nFull state:\n${JSON.stringify(state, null, 2)}`;
  throw new Error(`[${code}] ${message}${detail}`);
}

function makeOwner(lane, session) {
  return {
    sessionKey: session.key,
    epoch: session.epoch,
    lease: session.lease,
    jobId: `${lane.toLowerCase()}-job-${session.epoch}`,
  };
}

function makeLane(lane, session) {
  return {
    owner: makeOwner(lane, session),
    scheduler: "Idle",
    queueDurable: false,
    remoteAttempt: 0,
    duplicateCostEgressRisk: false,
    admission: {
      eventId: `${lane.toLowerCase()}-event-${session.epoch}`,
      digest: `prototype-digest-${lane.toLowerCase()}-${session.epoch}`,
      attemptedSequence: 1,
      lastReceipt: "None",
      committed: null,
    },
    materializedSequence: null,
    basisEligibleSequence: null,
    derivedCache: "Absent",
    refusedResults: 0,
    refusedWrites: 0,
  };
}

function initialState(policy) {
  const session = {
    key: "session-0",
    epoch: 0,
    lease: 0,
    lifecycle: "Active",
    deletionFence: 0,
    artifacts: "Present",
    pendingRemoteJobs: [],
  };

  return {
    model: "audio-graph-5e41-prototype-v1",
    policy: structuredClone(policy),
    session,
    lanes: Object.fromEntries(LANES.map((lane) => [lane, makeLane(lane, session)])),
    detachedOwners: [],
    diagnostics: {
      count: 0,
      last: null,
    },
  };
}

function addDiagnostic(state, code, metadata = {}) {
  state.diagnostics.count += 1;
  state.diagnostics.last = { code, ...metadata };
}

function ownerMatches(state, owner) {
  if (state.session.lifecycle !== "Active" || owner === null) return false;
  return (
    owner.sessionKey === state.session.key &&
    owner.epoch === state.session.epoch &&
    owner.lease === state.session.lease
  );
}

function fullDurabilityProof(streamKind) {
  return {
    streamKind,
    writeComplete: true,
    flushComplete: true,
    fileSynced: true,
    directorySynced: streamKind === "New",
  };
}

function isDurableProof(proof) {
  return (
    proof?.writeComplete === true &&
    proof.flushComplete === true &&
    proof.fileSynced === true &&
    (proof.streamKind === "Existing" || proof.directorySynced === true)
  );
}

function acceptCanonical(lane, status, proof) {
  modelGuard(
    status === "Accepted" || status === "AlreadyAccepted",
    "receipt-domain",
    `canonical acceptance cannot use ${status}`,
    lane,
  );
  modelGuard(
    isDurableProof(proof),
    "accepted-requires-durability",
    `${status} lacks its declared durability proof`,
    lane,
  );

  const candidate = {
    eventId: lane.admission.eventId,
    digest: lane.admission.digest,
    sequence: lane.admission.attemptedSequence,
    proof: structuredClone(proof),
  };

  if (lane.admission.committed !== null) {
    assert.deepEqual(lane.admission.committed, candidate);
  } else {
    lane.admission.committed = candidate;
  }

  lane.admission.lastReceipt = status;
  lane.materializedSequence = candidate.sequence;
  lane.basisEligibleSequence = candidate.sequence;
  lane.derivedCache = "Current";
}

function finishDeletion(state) {
  state.session.lifecycle = "Deleted";
  state.session.artifacts = "Absent";
  state.session.pendingRemoteJobs = [];
  for (const laneName of LANES) {
    const lane = state.lanes[laneName];
    lane.owner = null;
    lane.scheduler = "Fenced";
    lane.queueDurable = false;
    lane.admission.lastReceipt = "None";
    lane.admission.committed = null;
    lane.materializedSequence = null;
    lane.basisEligibleSequence = null;
    lane.derivedCache = "Absent";
  }
}

function transition(input, action) {
  const state = structuredClone(input);
  const lane = action.lane === undefined ? undefined : state.lanes[action.lane];

  switch (action.type) {
    case "DurableQueue": {
      modelGuard(
        lane.scheduler === "Idle",
        "scheduler-transition",
        "queue requires Idle",
        state,
      );
      lane.scheduler = "DurableQueued";
      lane.queueDurable = true;
      break;
    }
    case "DispatchRemote": {
      modelGuard(
        lane.scheduler === "DurableQueued",
        "scheduler-transition",
        "remote dispatch requires a durable queue record",
        state,
      );
      lane.scheduler = "RemoteInFlight";
      lane.remoteAttempt += 1;
      break;
    }
    case "RemoteResult": {
      if (!ownerMatches(state, action.owner) || lane.owner?.jobId !== action.owner.jobId) {
        lane.refusedResults += 1;
        const pendingIndex = state.session.pendingRemoteJobs.indexOf(action.owner.jobId);
        if (state.session.lifecycle === "Deleting" && pendingIndex >= 0) {
          state.session.pendingRemoteJobs.splice(pendingIndex, 1);
        }
        addDiagnostic(state, "DetachedResultRefused", {
          lane: action.lane,
          sessionEpoch: action.owner.epoch,
          lease: action.owner.lease,
          jobId: action.owner.jobId,
          status: action.result,
        });
        break;
      }
      modelGuard(
        lane.scheduler === "RemoteInFlight",
        "scheduler-transition",
        "current result requires RemoteInFlight",
        state,
      );
      lane.scheduler = action.result === "Success" ? "ResultReady" : "RemoteFailed";
      break;
    }
    case "BeginAdmission": {
      modelGuard(
        lane.scheduler === "ResultReady",
        "admission-transition",
        "admission requires a successful current result",
        state,
      );
      lane.admission.lastReceipt = "Pending";
      break;
    }
    case "Receipt": {
      modelGuard(
        RECEIPTS.includes(action.status),
        "receipt-domain",
        `unknown receipt ${action.status}`,
        state,
      );
      if (action.status === "Accepted" || action.status === "AlreadyAccepted") {
        acceptCanonical(lane, action.status, action.proof);
      } else {
        lane.admission.lastReceipt = action.status;
      }
      break;
    }
    case "RejectBeforeEnqueue": {
      lane.admission.lastReceipt = "Rejected";
      break;
    }
    case "ReconcileCrash": {
      if (action.diskOutcome === "DurableExact") {
        acceptCanonical(lane, "AlreadyAccepted", fullDurabilityProof(action.streamKind));
      } else {
        lane.admission.lastReceipt = "Rejected";
        if (action.diskOutcome === "TornTail") {
          addDiagnostic(state, "TornTailRequiresTypedQuarantine", {
            lane: action.lane,
            status: "Rejected",
          });
        }
      }
      break;
    }
    case "Retry": {
      const committed = lane.admission.committed;
      const exact =
        action.eventId === lane.admission.eventId && action.digest === lane.admission.digest;
      if (committed !== null) {
        if (
          exact &&
          action.eventId === committed.eventId &&
          action.digest === committed.digest
        ) {
          acceptCanonical(lane, "AlreadyAccepted", committed.proof);
        } else {
          lane.admission.lastReceipt = "Rejected";
          addDiagnostic(state, "IdempotencyConflictRejected", {
            lane: action.lane,
            sequence: committed.sequence,
            status: "Rejected",
          });
        }
      } else {
        modelGuard(
          exact,
          "exact-retry",
          "uncommitted retry must match attempted bytes",
          state,
        );
        acceptCanonical(lane, "Accepted", fullDurabilityProof(action.streamKind));
      }
      break;
    }
    case "SnapshotFailure": {
      modelGuard(
        lane.admission.committed !== null,
        "snapshot-authority",
        "a derived-cache failure only follows canonical acceptance",
        state,
      );
      lane.derivedCache = "LaggingRebuildable";
      addDiagnostic(state, "DerivedCacheLagging", {
        lane: action.lane,
        sequence: lane.admission.committed.sequence,
      });
      break;
    }
    case "Restart": {
      state.session.lease += 1;
      for (const laneName of LANES) {
        const restartedLane = state.lanes[laneName];
        if (restartedLane.owner !== null) {
          state.detachedOwners.push(restartedLane.owner);
        }
        if (restartedLane.scheduler === "RemoteInFlight") {
          restartedLane.scheduler = "ExternalEffectUnknown";
        }
        restartedLane.owner = makeOwner(laneName, state.session);
      }
      addDiagnostic(state, "SessionLeaseReplaced", {
        sessionEpoch: state.session.epoch,
        lease: state.session.lease,
      });
      break;
    }
    case "RecoverExternalUnknown": {
      modelGuard(
        lane.scheduler === "ExternalEffectUnknown",
        "scheduler-recovery",
        "remote recovery requires ExternalEffectUnknown",
        state,
      );
      if (state.policy.remoteReissue === "AutomaticAtRisk") {
        lane.scheduler = "RemoteInFlight";
        lane.remoteAttempt += 1;
        lane.duplicateCostEgressRisk = true;
        addDiagnostic(state, "AutomaticReissueDuplicateRisk", {
          lane: action.lane,
          sessionEpoch: state.session.epoch,
          lease: state.session.lease,
          jobId: lane.owner.jobId,
        });
      } else {
        lane.scheduler = "AwaitingDecision";
        addDiagnostic(state, "RemoteEffectNeedsDecision", {
          lane: action.lane,
          sessionEpoch: state.session.epoch,
          lease: state.session.lease,
          jobId: lane.owner.jobId,
        });
      }
      break;
    }
    case "RotateSession": {
      for (const laneName of LANES) {
        if (state.lanes[laneName].owner !== null) {
          state.detachedOwners.push(state.lanes[laneName].owner);
        }
      }
      state.session.key = `session-${state.session.epoch + 1}`;
      state.session.epoch += 1;
      state.session.lease += 1;
      state.session.lifecycle = "Active";
      state.session.artifacts = "Present";
      state.session.pendingRemoteJobs = [];
      state.lanes = Object.fromEntries(
        LANES.map((laneName) => [laneName, makeLane(laneName, state.session)]),
      );
      addDiagnostic(state, "SessionEpochReplaced", {
        sessionEpoch: state.session.epoch,
        lease: state.session.lease,
      });
      break;
    }
    case "DetachedWrite": {
      modelGuard(
        !ownerMatches(state, action.owner) || lane.owner?.jobId !== action.owner.jobId,
        "detached-writer-refusal",
        "DetachedWrite requires a retired owner token",
        state,
      );
      lane.refusedWrites += 1;
      addDiagnostic(state, "DetachedWriterRefused", {
        lane: action.lane,
        sessionEpoch: action.owner.epoch,
        lease: action.owner.lease,
        jobId: action.owner.jobId,
      });
      break;
    }
    case "BeginDelete": {
      modelGuard(
        state.session.lifecycle === "Active",
        "deletion-transition",
        "deletion begins only from Active",
        state,
      );
      state.session.lifecycle = "Deleting";
      state.session.deletionFence += 1;
      state.session.lease += 1;
      state.session.pendingRemoteJobs = [];
      for (const laneName of LANES) {
        const deletingLane = state.lanes[laneName];
        if (deletingLane.owner !== null) {
          state.detachedOwners.push(deletingLane.owner);
        }
        if (
          deletingLane.scheduler === "RemoteInFlight" ||
          deletingLane.scheduler === "ExternalEffectUnknown"
        ) {
          state.session.pendingRemoteJobs.push(deletingLane.owner.jobId);
        }
        deletingLane.owner = null;
        deletingLane.scheduler = "Fenced";
      }
      addDiagnostic(state, "DeletionFenceRaised", {
        sessionEpoch: state.session.epoch,
        lease: state.session.lease,
      });
      if (state.policy.deletion === "DiscardImmediately") finishDeletion(state);
      break;
    }
    case "CompleteDelete": {
      modelGuard(
        state.session.lifecycle === "Deleting" &&
          state.session.pendingRemoteJobs.length === 0,
        "deletion-transition",
        "wait deletion completes only after every remote terminal is observed",
        state,
      );
      finishDeletion(state);
      break;
    }
    default:
      throw new Error(`unknown prototype action: ${action.type}`);
  }

  return state;
}

function reduce(input, action) {
  transitionCount += 1;
  const state = transition(input, action);
  for (const laneName of LANES) {
    const status = state.lanes[laneName].admission.lastReceipt;
    if (RECEIPTS.includes(status)) observedReceipts.add(status);
  }
  verifyTransition(input, state, action);
  verifyState(state, `${action.type}:${action.lane ?? "Session"}`);
  return state;
}

function savedLabel(state, laneName) {
  const lane = state.lanes[laneName];
  if (lane.admission.committed !== null) return state.policy.savedWording;
  switch (lane.admission.lastReceipt) {
    case "Pending":
      return "Saving";
    case "OutcomeUncertain":
      return "Recovery required";
    case "Rejected":
      return "Not saved";
    default:
      return "Not started";
  }
}

function verifyTransition(before, after, action) {
  for (const laneName of LANES) {
    const beforeSequence = before.lanes[laneName].materializedSequence;
    const afterSequence = after.lanes[laneName].materializedSequence;
    if (beforeSequence !== afterSequence && afterSequence !== null) {
      invariant(
        ["Receipt", "ReconcileCrash", "Retry"].includes(action.type) &&
          ["Accepted", "AlreadyAccepted"].includes(
            after.lanes[laneName].admission.lastReceipt,
          ),
        "accepted-only-advancement",
        `${action.type} advanced ${laneName} without Accepted/AlreadyAccepted`,
        after,
      );
    }
  }

  if (action.type === "RemoteResult" && !ownerMatches(before, action.owner)) {
    for (const laneName of LANES) {
      assert.deepEqual(
        after.lanes[laneName].admission,
        before.lanes[laneName].admission,
        "detached result changed canonical admission",
      );
      assert.equal(
        after.lanes[laneName].materializedSequence,
        before.lanes[laneName].materializedSequence,
        "detached result changed materialized state",
      );
      assert.equal(
        after.lanes[laneName].basisEligibleSequence,
        before.lanes[laneName].basisEligibleSequence,
        "detached result changed basis eligibility",
      );
    }
    invariantFamilies.add("detached-result-refusal");
    assertionCount += 3 * LANES.length;
  }
}

function verifyState(state, where) {
  observedStates.add(JSON.stringify(state));

  for (const laneName of LANES) {
    const lane = state.lanes[laneName];
    const committed = lane.admission.committed;
    invariant(
      (committed === null &&
        lane.materializedSequence === null &&
        lane.basisEligibleSequence === null) ||
        (committed !== null &&
          lane.materializedSequence === committed.sequence &&
          lane.basisEligibleSequence === committed.sequence),
      "atomic-materialization-and-basis",
      `${where} split canonical, materialized, and basis state for ${laneName}`,
      state,
    );
    if (committed !== null) {
      invariant(
        isDurableProof(committed.proof),
        "accepted-requires-durability",
        `${where} retained a non-durable commit for ${laneName}`,
        state,
      );
      invariant(
        committed.sequence === lane.admission.attemptedSequence,
        "committed-sequence-stability",
        `${where} changed the attempted sequence for ${laneName}`,
        state,
      );
    }
    invariant(
      committed !== null || savedLabel(state, laneName) !== state.policy.savedWording,
      "saved-label-requires-commit",
      `${where} labeled uncommitted ${laneName} as saved`,
      state,
    );
    if (state.session.lifecycle === "Active" && lane.owner !== null) {
      invariant(
        ownerMatches(state, lane.owner),
        "active-owner-current",
        `${where} retained a stale active owner for ${laneName}`,
        state,
      );
    }
    if (lane.duplicateCostEgressRisk) {
      invariant(
        state.policy.remoteReissue === "AutomaticAtRisk" && lane.remoteAttempt >= 2,
        "automatic-reissue-risk-visible",
        `${where} hid or invented duplicate cost/egress risk`,
        state,
      );
    }
  }

  if (state.session.lifecycle === "Deleted") {
    invariant(
      state.session.artifacts === "Absent" &&
        LANES.every(
          (laneName) =>
            state.lanes[laneName].owner === null &&
            state.lanes[laneName].admission.committed === null,
        ),
      "deleted-state-empty-and-fenced",
      `${where} retained a writer or artifact after deletion`,
      state,
    );
  }

  const diagnostic = state.diagnostics.last;
  if (diagnostic !== null) {
    const allowedKeys = new Set([
      "code",
      "lane",
      "sessionEpoch",
      "lease",
      "jobId",
      "status",
      "sequence",
    ]);
    invariant(
      Object.keys(diagnostic).every((key) => allowedKeys.has(key)),
      "content-free-diagnostics",
      `${where} diagnostic used a content-bearing field`,
      state,
    );
    invariant(
      Object.values(diagnostic).every(
        (value) =>
          typeof value !== "string" ||
          !/(transcript|prompt|audio|secret|bearer|credential|payload|content)/iu.test(value),
      ),
      "content-free-diagnostics",
      `${where} diagnostic used a content-bearing value`,
      state,
    );
  }
}

function prepareRemote(state, laneName) {
  let next = reduce(state, { type: "DurableQueue", lane: laneName });
  next = reduce(next, { type: "DispatchRemote", lane: laneName });
  return next;
}

function prepareAdmission(state, laneName) {
  let next = prepareRemote(state, laneName);
  next = reduce(next, {
    type: "RemoteResult",
    lane: laneName,
    owner: structuredClone(next.lanes[laneName].owner),
    result: "Success",
  });
  return reduce(next, { type: "BeginAdmission", lane: laneName });
}

function applyReceiptPath(state, laneName, status, streamKind = "New") {
  let next = prepareAdmission(state, laneName);
  if (status === "Pending") return next;
  if (status === "Accepted") {
    return reduce(next, {
      type: "Receipt",
      lane: laneName,
      status,
      proof: fullDurabilityProof(streamKind),
    });
  }
  if (status === "AlreadyAccepted") {
    next = reduce(next, {
      type: "Receipt",
      lane: laneName,
      status: "Accepted",
      proof: fullDurabilityProof(streamKind),
    });
    return reduce(next, {
      type: "Retry",
      lane: laneName,
      eventId: next.lanes[laneName].admission.eventId,
      digest: next.lanes[laneName].admission.digest,
      streamKind,
    });
  }
  return reduce(next, { type: "Receipt", lane: laneName, status });
}

function diskOutcomes(streamKind, cut) {
  switch (cut) {
    case "BeforeEnqueue":
    case "AfterEnqueue":
      return ["Absent"];
    case "AfterWrite":
    case "AfterFlush":
      return ["Absent", "TornTail", "DurableExact"];
    case "AfterFileSync":
      return streamKind === "Existing" ? ["DurableExact"] : ["Absent", "DurableExact"];
    case "AfterDirectorySync":
    case "AfterAck":
      return ["DurableExact"];
    default:
      throw new Error(`unknown crash cut: ${cut}`);
  }
}

function callerReceiptAtCut(cut) {
  if (cut === "BeforeEnqueue") return "Rejected";
  if (cut === "AfterEnqueue") return "Pending";
  if (cut === "AfterAck") return "Accepted";
  return "OutcomeUncertain";
}

function checkAdmissionCrashMatrix() {
  let matrixCases = 0;
  for (const policy of policyProfiles) {
    for (const laneName of LANES) {
      for (const streamKind of STREAM_KINDS) {
        for (const cut of CUTS) {
          for (const diskOutcome of diskOutcomes(streamKind, cut)) {
            matrixCases += 1;
            caseCount += 1;
            let state = initialState(policy);
            verifyState(state, "crash-matrix-initial");
            if (cut === "BeforeEnqueue") {
              state = reduce(state, { type: "RejectBeforeEnqueue", lane: laneName });
            } else {
              state = prepareAdmission(state, laneName);
              const callerStatus = callerReceiptAtCut(cut);
              if (callerStatus !== "Pending") {
                state = reduce(state, {
                  type: "Receipt",
                  lane: laneName,
                  status: callerStatus,
                  proof:
                    callerStatus === "Accepted"
                      ? fullDurabilityProof(streamKind)
                      : undefined,
                });
              }
            }

            invariant(
              state.lanes[laneName].admission.lastReceipt === callerReceiptAtCut(cut),
              "crash-cut-receipt",
              `${streamKind}/${cut} exposed the wrong caller receipt`,
              state,
            );

            if (cut === "AfterAck") {
              state = reduce(state, {
                type: "Retry",
                lane: laneName,
                eventId: state.lanes[laneName].admission.eventId,
                digest: state.lanes[laneName].admission.digest,
                streamKind,
              });
            } else if (cut !== "BeforeEnqueue" || diskOutcome !== "Absent") {
              state = reduce(state, {
                type: "ReconcileCrash",
                lane: laneName,
                streamKind,
                diskOutcome,
              });
            }

            if (state.lanes[laneName].admission.committed === null) {
              state = reduce(state, {
                type: "Retry",
                lane: laneName,
                eventId: state.lanes[laneName].admission.eventId,
                digest: state.lanes[laneName].admission.digest,
                streamKind,
              });
            }

            invariant(
              state.lanes[laneName].admission.committed.sequence === 1,
              "committed-sequence-stability",
              `${streamKind}/${cut}/${diskOutcome} did not converge on sequence 1`,
              state,
            );
          }
        }
      }
    }
  }
  return matrixCases;
}

function checkReceiptAndRetryMatrix() {
  let matrixCases = 0;
  for (const policy of policyProfiles) {
    for (const laneName of LANES) {
      for (const status of RECEIPTS) {
        matrixCases += 1;
        caseCount += 1;
        const state = applyReceiptPath(initialState(policy), laneName, status);
        invariant(
          state.lanes[laneName].admission.lastReceipt === status,
          "receipt-domain",
          `receipt path did not end at ${status}`,
          state,
        );
      }

      let exact = applyReceiptPath(initialState(policy), laneName, "Accepted");
      const committedBefore = structuredClone(exact.lanes[laneName].admission.committed);
      exact = reduce(exact, {
        type: "Retry",
        lane: laneName,
        eventId: exact.lanes[laneName].admission.eventId,
        digest: exact.lanes[laneName].admission.digest,
        streamKind: "New",
      });
      assert.deepEqual(exact.lanes[laneName].admission.committed, committedBefore);
      invariantFamilies.add("exact-retry");
      assertionCount += 1;

      const conflicted = reduce(exact, {
        type: "Retry",
        lane: laneName,
        eventId: exact.lanes[laneName].admission.eventId,
        digest: "prototype-digest-conflict",
        streamKind: "New",
      });
      assert.deepEqual(conflicted.lanes[laneName].admission.committed, committedBefore);
      invariant(
        conflicted.lanes[laneName].admission.lastReceipt === "Rejected",
        "idempotency-conflict-refused",
        "mismatched retry changed or duplicated a commit",
        conflicted,
      );
    }
  }
  return matrixCases;
}

function checkSchedulerRestartMatrix() {
  let matrixCases = 0;
  for (const policy of policyProfiles) {
    for (const laneName of LANES) {
      matrixCases += 2;
      caseCount += 2;

      let queued = reduce(initialState(policy), { type: "DurableQueue", lane: laneName });
      queued = reduce(queued, { type: "Restart" });
      invariant(
        queued.lanes[laneName].scheduler === "DurableQueued" &&
          queued.lanes[laneName].remoteAttempt === 0,
        "durable-queue-recovery",
        "restart did not preserve a safe queued job",
        queued,
      );

      let unknown = reduce(queued, { type: "DispatchRemote", lane: laneName });
      const detachedOwner = structuredClone(unknown.lanes[laneName].owner);
      unknown = reduce(unknown, { type: "Restart" });
      invariant(
        unknown.lanes[laneName].scheduler === "ExternalEffectUnknown",
        "external-effect-unknown-recovery",
        "restart inferred an in-flight remote outcome",
        unknown,
      );
      const beforeLateResult = structuredClone(unknown.lanes[laneName].admission);
      unknown = reduce(unknown, {
        type: "RemoteResult",
        lane: laneName,
        owner: detachedOwner,
        result: "Success",
      });
      assert.deepEqual(unknown.lanes[laneName].admission, beforeLateResult);
      assertionCount += 1;
      invariantFamilies.add("detached-result-refusal");

      const recovered = reduce(unknown, {
        type: "RecoverExternalUnknown",
        lane: laneName,
      });
      if (policy.remoteReissue === "AutomaticAtRisk") {
        invariant(
          recovered.lanes[laneName].scheduler === "RemoteInFlight" &&
            recovered.lanes[laneName].duplicateCostEgressRisk,
          "automatic-reissue-risk-visible",
          "automatic reissue hid duplicate risk",
          recovered,
        );
      } else {
        invariant(
          recovered.lanes[laneName].scheduler === "AwaitingDecision" &&
            !recovered.lanes[laneName].duplicateCostEgressRisk,
          "manual-remote-reconciliation",
          "decision-required policy reissued remote work",
          recovered,
        );
      }
    }
  }
  return matrixCases;
}

function checkLaneIndependenceMatrix() {
  let matrixCases = 0;
  for (const policy of policyProfiles) {
    for (const notesStatus of RECEIPTS) {
      for (const graphStatus of RECEIPTS) {
        matrixCases += 1;
        caseCount += 1;
        let notesThenGraph = applyReceiptPath(
          initialState(policy),
          "Notes",
          notesStatus,
        );
        notesThenGraph = applyReceiptPath(notesThenGraph, "Graph", graphStatus);

        let graphThenNotes = applyReceiptPath(
          initialState(policy),
          "Graph",
          graphStatus,
        );
        graphThenNotes = applyReceiptPath(graphThenNotes, "Notes", notesStatus);

        assert.deepEqual(notesThenGraph.lanes, graphThenNotes.lanes);
        invariantFamilies.add("lane-order-independence");
        assertionCount += 1;
        verifyState(notesThenGraph, "lane-independence");
      }
    }
  }
  return matrixCases;
}

function checkRotationAndDeletionMatrix() {
  let matrixCases = 0;
  for (const policy of policyProfiles) {
    for (const notesResult of RESULT_KINDS) {
      for (const graphResult of RESULT_KINDS) {
        matrixCases += 2;
        caseCount += 2;

        let rotating = prepareRemote(initialState(policy), "Notes");
        rotating = prepareRemote(rotating, "Graph");
        const oldOwners = Object.fromEntries(
          LANES.map((laneName) => [laneName, structuredClone(rotating.lanes[laneName].owner)]),
        );
        rotating = reduce(rotating, { type: "RotateSession" });
        const cleanRotatedLanes = structuredClone(rotating.lanes);
        for (const [laneName, result] of [
          ["Notes", notesResult],
          ["Graph", graphResult],
        ]) {
          rotating = reduce(rotating, {
            type: "RemoteResult",
            lane: laneName,
            owner: oldOwners[laneName],
            result,
          });
          rotating = reduce(rotating, {
            type: "DetachedWrite",
            lane: laneName,
            owner: oldOwners[laneName],
          });
        }
        for (const laneName of LANES) {
          assert.deepEqual(
            rotating.lanes[laneName].admission,
            cleanRotatedLanes[laneName].admission,
          );
          invariant(
            rotating.lanes[laneName].refusedResults === 1 &&
              rotating.lanes[laneName].refusedWrites === 1,
            "rotation-fence",
            `rotation did not refuse detached ${laneName} activity`,
            rotating,
          );
        }

        let deleting = prepareRemote(initialState(policy), "Notes");
        deleting = prepareRemote(deleting, "Graph");
        const deletingOwners = Object.fromEntries(
          LANES.map((laneName) => [laneName, structuredClone(deleting.lanes[laneName].owner)]),
        );
        deleting = reduce(deleting, { type: "BeginDelete" });

        if (policy.deletion === "DiscardImmediately") {
          invariant(
            deleting.session.lifecycle === "Deleted" &&
              deleting.session.artifacts === "Absent",
            "immediate-delete-policy",
            "discard policy waited for remote work",
            deleting,
          );
        } else {
          invariant(
            deleting.session.lifecycle === "Deleting" &&
              deleting.session.artifacts === "Present" &&
              deleting.session.pendingRemoteJobs.length === 2,
            "wait-delete-policy",
            "wait policy removed artifacts before remote terminals",
            deleting,
          );
        }

        for (const [laneName, result] of [
          ["Notes", notesResult],
          ["Graph", graphResult],
        ]) {
          deleting = reduce(deleting, {
            type: "RemoteResult",
            lane: laneName,
            owner: deletingOwners[laneName],
            result,
          });
          deleting = reduce(deleting, {
            type: "DetachedWrite",
            lane: laneName,
            owner: deletingOwners[laneName],
          });
        }
        if (policy.deletion === "WaitForRemote") {
          invariant(
            deleting.session.lifecycle === "Deleting" &&
              deleting.session.pendingRemoteJobs.length === 0,
            "wait-delete-policy",
            "wait policy did not observe all remote terminals",
            deleting,
          );
          deleting = reduce(deleting, { type: "CompleteDelete" });
        }
        invariant(
          deleting.session.lifecycle === "Deleted" &&
            deleting.session.artifacts === "Absent",
          "deletion-fence",
          "deletion did not converge on an artifact-free fenced state",
          deleting,
        );
      }
    }
  }
  return matrixCases;
}

function compactState(state) {
  return {
    policy: state.policy,
    session: state.session,
    lanes: Object.fromEntries(
      LANES.map((laneName) => {
        const lane = state.lanes[laneName];
        return [
          laneName,
          {
            owner: lane.owner,
            scheduler: lane.scheduler,
            queueDurable: lane.queueDurable,
            remoteAttempt: lane.remoteAttempt,
            duplicateCostEgressRisk: lane.duplicateCostEgressRisk,
            admission: lane.admission,
            materializedSequence: lane.materializedSequence,
            basisEligibleSequence: lane.basisEligibleSequence,
            derivedCache: lane.derivedCache,
            savedLabel: savedLabel(state, laneName),
            refusedResults: lane.refusedResults,
            refusedWrites: lane.refusedWrites,
          },
        ];
      }),
    ),
    detachedOwners: state.detachedOwners,
    diagnostics: state.diagnostics,
  };
}

function traceStep(label, state) {
  console.log(`${label}: ${JSON.stringify(compactState(state))}`);
}

function showRepresentativeTrace() {
  console.log("\nRepresentative full-state trace (new stream, uncertain acknowledgement):");
  const policy = {
    savedWording: "Saved",
    remoteReissue: "RequireDecision",
    deletion: "DiscardImmediately",
  };
  let state = initialState(policy);
  verifyState(state, "trace-initial");
  traceStep("0 initial", state);
  state = reduce(state, { type: "DurableQueue", lane: "Notes" });
  traceStep("1 durable scheduler enqueue", state);
  state = reduce(state, { type: "DispatchRemote", lane: "Notes" });
  traceStep("2 remote dispatched", state);
  state = reduce(state, {
    type: "RemoteResult",
    lane: "Notes",
    owner: structuredClone(state.lanes.Notes.owner),
    result: "Success",
  });
  traceStep("3 remote result ready", state);
  state = reduce(state, { type: "BeginAdmission", lane: "Notes" });
  traceStep("4 canonical Pending", state);
  state = reduce(state, {
    type: "Receipt",
    lane: "Notes",
    status: "OutcomeUncertain",
  });
  traceStep("5 crash after file sync but before new-file directory sync/ack", state);
  state = reduce(state, {
    type: "ReconcileCrash",
    lane: "Notes",
    streamKind: "New",
    diskOutcome: "DurableExact",
  });
  traceStep("6 exact reopen/retry returns AlreadyAccepted", state);
  state = reduce(state, { type: "SnapshotFailure", lane: "Notes" });
  traceStep("7 derived cache lags without rolling back canonical state", state);
  const oldOwner = structuredClone(state.lanes.Notes.owner);
  state = reduce(state, { type: "RotateSession" });
  traceStep("8 epoch and lease replaced", state);
  state = reduce(state, {
    type: "RemoteResult",
    lane: "Notes",
    owner: oldOwner,
    result: "Failure",
  });
  traceStep("9 late failure refused", state);
  state = reduce(state, {
    type: "DetachedWrite",
    lane: "Notes",
    owner: oldOwner,
  });
  traceStep("10 detached writer refused", state);
}

console.log("THROWAWAY PROTOTYPE — no production imports, files, network, or persistence");
console.log(
  "Question: can durable admission and Session fencing prevent premature visibility, duplicate sequence, and post-rotation/delete resurrection?",
);
console.log(
  "Assumptions: finite metadata-only model; new files need file + directory barriers; unknown remote effect stays unknown; policies are parameters.",
);

showRepresentativeTrace();

const counts = {
  admissionCrashCases: checkAdmissionCrashMatrix(),
  receiptCases: checkReceiptAndRetryMatrix(),
  schedulerRestartCases: checkSchedulerRestartMatrix(),
  laneIndependenceCases: checkLaneIndependenceMatrix(),
  rotationAndDeletionCases: checkRotationAndDeletionMatrix(),
};

assert.deepEqual([...observedReceipts].sort(), [...RECEIPTS].sort());
invariantFamilies.add("all-receipts-observed");
assertionCount += 1;

console.log("\nExhaustive finite-model result:");
console.log(JSON.stringify({ policyProfiles: policyProfiles.length, ...counts }, null, 2));
console.log(`cases explored: ${caseCount}`);
console.log(`transitions evaluated: ${transitionCount}`);
console.log(`unique full states observed: ${observedStates.size}`);
console.log(`invariant assertions: ${assertionCount}`);
console.log(`invariant families passed (${invariantFamilies.size}):`);
for (const family of [...invariantFamilies].sort()) console.log(`- ${family}`);

console.log("\nPolicy decision table (all eight combinations passed safety invariants):");
console.log(
  "- Saved wording: `Saved` or `Durably saved`; recommend reserving the chosen label strictly for Accepted/AlreadyAccepted.",
);
console.log(
  "- Unknown remote effect: require a decision or auto-reissue with an explicit duplicate cost/egress flag; recommend no automatic reissue without provider idempotency proof.",
);
console.log(
  "- Deletion: fence then discard immediately, or fence then wait while refusing results/writes; recommend immediate discard for the default.",
);
console.log("Human acceptance of these three recommendations remains required.");
console.log("\nPASS: finite model exhausted; every invariant held; diagnostics remained content-free.");
