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
 * barriers, binds receipts to the exact Pending/recovery capability, tracks
 * each remote attempt by exact effect identity, and emits only closed-schema
 * diagnostics. Remote side effects cannot be inferred after a process loses an
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
const DIAGNOSTIC_CODES = [
  "AutomaticReissueDuplicateRisk",
  "DeletionFenceRaised",
  "DerivedCacheLagging",
  "DetachedResultRefused",
  "DetachedWriterRefused",
  "IdempotencyConflictRejected",
  "RemoteEffectNeedsDecision",
  "SessionEpochReplaced",
  "SessionLeaseReplaced",
  "TornTailRequiresTypedQuarantine",
];
const DIAGNOSTIC_REASONS = [
  "DeletionTerminalObserved",
  "DerivedCacheLag",
  "DuplicateCostEgressRisk",
  "EffectIdentityMismatch",
  "ExternalEffectUnknown",
  "IdempotencyConflict",
  "InvalidResultKind",
  "LaneMismatch",
  "RetiredOwner",
  "TypedQuarantineRequired",
];

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
    outstandingEffects: [],
    duplicateCostEgressRisk: false,
    admission: {
      eventId: `${lane.toLowerCase()}-event-${session.epoch}`,
      digest: `prototype-digest-${lane.toLowerCase()}-${session.epoch}`,
      attemptedSequence: 1,
      lastReceipt: "None",
      pendingBinding: null,
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
    pendingRemoteEffects: [],
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

function opaqueRef(value) {
  const serialized = typeof value === "string" ? value : JSON.stringify(value ?? null);
  const input = typeof serialized === "string" ? serialized : "unrepresentable";
  let hash = 0x811c9dc5;
  for (let index = 0; index < input.length; index += 1) {
    hash ^= input.charCodeAt(index);
    hash = Math.imul(hash, 0x01000193);
  }
  return `opaque:${(hash >>> 0).toString(16).padStart(8, "0")}`;
}

function addDiagnostic(state, code, metadata = {}) {
  modelGuard(
    DIAGNOSTIC_CODES.includes(code),
    "diagnostic-schema",
    "diagnostic code must be a closed enum",
    state,
  );
  const diagnostic = { code };
  if (LANES.includes(metadata.lane)) diagnostic.lane = metadata.lane;
  if (DIAGNOSTIC_REASONS.includes(metadata.reason)) diagnostic.reason = metadata.reason;
  for (const key of ["sessionEpoch", "lease", "sequence", "attempt"]) {
    if (Number.isSafeInteger(metadata[key]) && metadata[key] >= 0) {
      diagnostic[key] = metadata[key];
    }
  }
  if (metadata.jobId !== undefined) diagnostic.jobRef = opaqueRef(metadata.jobId);
  if (metadata.effect !== undefined) diagnostic.effectRef = opaqueRef(metadata.effect);
  state.diagnostics.count += 1;
  state.diagnostics.last = diagnostic;
}

function ownerMatches(state, owner) {
  if (state.session.lifecycle !== "Active" || owner === null) return false;
  return (
    owner.sessionKey === state.session.key &&
    owner.epoch === state.session.epoch &&
    owner.lease === state.session.lease
  );
}

function ownersEqual(left, right) {
  return (
    left !== null &&
    right !== null &&
    left.sessionKey === right.sessionKey &&
    left.epoch === right.epoch &&
    left.lease === right.lease &&
    left.jobId === right.jobId
  );
}

function makeEffectIdentity(laneName, owner, attempt) {
  return {
    lane: laneName,
    sessionKey: owner.sessionKey,
    sessionEpoch: owner.epoch,
    lease: owner.lease,
    jobId: owner.jobId,
    attempt,
    resultRef: `${laneName.toLowerCase()}-result-${owner.epoch}-${owner.lease}-${attempt}`,
    owner: structuredClone(owner),
  };
}

function effectsEqual(left, right) {
  return (
    left !== null &&
    right !== null &&
    left.lane === right.lane &&
    left.sessionKey === right.sessionKey &&
    left.sessionEpoch === right.sessionEpoch &&
    left.lease === right.lease &&
    left.jobId === right.jobId &&
    left.attempt === right.attempt &&
    left.resultRef === right.resultRef &&
    ownersEqual(left.owner, right.owner)
  );
}

function effectIsStructurallyValid(effect, laneName) {
  return (
    effect !== null &&
    effect.lane === laneName &&
    typeof effect.sessionKey === "string" &&
    Number.isSafeInteger(effect.sessionEpoch) &&
    effect.sessionEpoch >= 0 &&
    Number.isSafeInteger(effect.lease) &&
    effect.lease >= 0 &&
    typeof effect.jobId === "string" &&
    Number.isSafeInteger(effect.attempt) &&
    effect.attempt >= 1 &&
    typeof effect.resultRef === "string" &&
    effect.owner !== null &&
    effect.sessionKey === effect.owner.sessionKey &&
    effect.sessionEpoch === effect.owner.epoch &&
    effect.lease === effect.owner.lease &&
    effect.jobId === effect.owner.jobId
  );
}

function effectIndex(effects, candidate) {
  return effects.findIndex((effect) => effectsEqual(effect, candidate));
}

function admissionPrestate(lane) {
  return lane.admission.committed === null ? lane.admission.lastReceipt : "Committed";
}

function makeReceiptBinding(state, laneName, expectedPrestate = undefined) {
  const lane = state.lanes[laneName];
  return {
    lane: laneName,
    owner: structuredClone(lane.owner),
    sessionKey: state.session.key,
    sessionEpoch: state.session.epoch,
    lease: state.session.lease,
    eventId: lane.admission.eventId,
    digest: lane.admission.digest,
    expectedLifecycle: "Active",
    expectedPrestate: expectedPrestate ?? admissionPrestate(lane),
  };
}

function bindingsEqual(left, right) {
  return (
    left !== null &&
    right !== null &&
    left.lane === right.lane &&
    ownersEqual(left.owner, right.owner) &&
    left.sessionKey === right.sessionKey &&
    left.sessionEpoch === right.sessionEpoch &&
    left.lease === right.lease &&
    left.eventId === right.eventId &&
    left.digest === right.digest &&
    left.expectedLifecycle === right.expectedLifecycle &&
    left.expectedPrestate === right.expectedPrestate
  );
}

function validateReceiptBinding(state, laneName, binding, allowedPrestates) {
  const lane = state.lanes[laneName];
  const currentPrestate = admissionPrestate(lane);
  modelGuard(
    binding !== null && typeof binding === "object",
    "receipt-binding",
    "canonical receipt requires a binding",
    state,
  );
  modelGuard(
    LANES.includes(laneName) && binding.lane === laneName,
    "receipt-binding",
    "canonical receipt lane mismatch",
    state,
  );
  modelGuard(
    state.session.lifecycle === "Active" && binding.expectedLifecycle === "Active",
    "receipt-binding",
    "canonical receipt requires the Active lifecycle",
    state,
  );
  modelGuard(
    binding.sessionKey === state.session.key &&
      binding.sessionEpoch === state.session.epoch &&
      binding.lease === state.session.lease &&
      ownersEqual(binding.owner, lane.owner),
    "receipt-binding",
    "canonical receipt owner or Session generation mismatch",
    state,
  );
  modelGuard(
    binding.eventId === lane.admission.eventId && binding.digest === lane.admission.digest,
    "receipt-binding",
    "canonical receipt event identity mismatch",
    state,
  );
  modelGuard(
    allowedPrestates.includes(currentPrestate) &&
      binding.expectedPrestate === currentPrestate,
    "receipt-binding",
    "canonical receipt prestate mismatch",
    state,
  );
  if (currentPrestate === "Pending") {
    modelGuard(
      lane.admission.pendingBinding !== null &&
        bindingsEqual(binding, lane.admission.pendingBinding),
      "receipt-binding",
      "canonical receipt does not match the Pending admission",
      state,
    );
  }
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
    STREAM_KINDS.includes(proof?.streamKind) &&
    proof?.writeComplete === true &&
    proof.flushComplete === true &&
    proof.fileSynced === true &&
    (proof.streamKind === "Existing" ||
      (proof.streamKind === "New" && proof.directorySynced === true))
  );
}

function acceptCanonical(state, laneName, status, proof, binding, allowedPrestates) {
  const lane = state.lanes[laneName];
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
  validateReceiptBinding(state, laneName, binding, allowedPrestates);

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
  lane.admission.pendingBinding = null;
  lane.materializedSequence = candidate.sequence;
  lane.basisEligibleSequence = candidate.sequence;
  lane.derivedCache = "Current";
}

function finishDeletion(state) {
  state.session.lifecycle = "Deleted";
  state.session.artifacts = "Absent";
  state.session.pendingRemoteEffects = [];
  for (const laneName of LANES) {
    const lane = state.lanes[laneName];
    lane.owner = null;
    lane.scheduler = "Fenced";
    lane.queueDurable = false;
    lane.outstandingEffects = [];
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
      lane.outstandingEffects.push(
        makeEffectIdentity(action.lane, lane.owner, lane.remoteAttempt),
      );
      break;
    }
    case "RemoteResult": {
      const resultKindValid = RESULT_KINDS.includes(action.result);
      const laneEffectIndex = effectIndex(lane.outstandingEffects, action.effect);
      const exactCurrentEffect =
        resultKindValid &&
        ownerMatches(state, action.owner) &&
        ownersEqual(action.owner, lane.owner) &&
        action.effect?.lane === action.lane &&
        ownersEqual(action.owner, action.effect?.owner) &&
        laneEffectIndex >= 0;
      if (!exactCurrentEffect) {
        lane.refusedResults += 1;
        const pendingIndex = effectIndex(
          state.session.pendingRemoteEffects,
          action.effect,
        );
        const exactDeletingEffect =
          state.session.lifecycle === "Deleting" &&
          resultKindValid &&
          action.effect?.lane === action.lane &&
          ownersEqual(action.owner, action.effect?.owner) &&
          pendingIndex >= 0;
        if (exactDeletingEffect) {
          state.session.pendingRemoteEffects.splice(pendingIndex, 1);
          const deletingLaneEffect = effectIndex(lane.outstandingEffects, action.effect);
          if (deletingLaneEffect >= 0) lane.outstandingEffects.splice(deletingLaneEffect, 1);
        }
        const refusalReason = !resultKindValid
          ? "InvalidResultKind"
          : exactDeletingEffect
            ? "DeletionTerminalObserved"
            : action.effect?.lane !== action.lane
              ? "LaneMismatch"
              : !ownerMatches(state, action.owner)
                ? "RetiredOwner"
                : "EffectIdentityMismatch";
        addDiagnostic(state, "DetachedResultRefused", {
          lane: action.lane,
          reason: refusalReason,
          sessionEpoch: action.owner?.epoch,
          lease: action.owner?.lease,
          attempt: action.effect?.attempt,
          jobId: action.owner?.jobId,
          effect: action.effect,
        });
        break;
      }
      modelGuard(
        lane.scheduler === "RemoteInFlight",
        "scheduler-transition",
        "current result requires RemoteInFlight",
        state,
      );
      lane.outstandingEffects.splice(laneEffectIndex, 1);
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
      lane.admission.pendingBinding = makeReceiptBinding(state, action.lane, "Pending");
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
        acceptCanonical(
          state,
          action.lane,
          action.status,
          action.proof,
          action.binding,
          action.status === "Accepted" ? ["Pending"] : ["Committed"],
        );
      } else {
        validateReceiptBinding(state, action.lane, action.binding, ["Pending"]);
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
        acceptCanonical(
          state,
          action.lane,
          "AlreadyAccepted",
          fullDurabilityProof(action.streamKind),
          action.binding,
          ["Pending", "OutcomeUncertain"],
        );
      } else {
        validateReceiptBinding(
          state,
          action.lane,
          action.binding,
          ["Pending", "OutcomeUncertain"],
        );
        lane.admission.lastReceipt = "Rejected";
        if (action.diskOutcome === "TornTail") {
          addDiagnostic(state, "TornTailRequiresTypedQuarantine", {
            lane: action.lane,
            reason: "TypedQuarantineRequired",
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
          acceptCanonical(
            state,
            action.lane,
            "AlreadyAccepted",
            committed.proof,
            action.binding,
            ["Committed"],
          );
        } else {
          lane.admission.lastReceipt = "Rejected";
          addDiagnostic(state, "IdempotencyConflictRejected", {
            lane: action.lane,
            reason: "IdempotencyConflict",
            sequence: committed.sequence,
          });
        }
      } else {
        modelGuard(
          exact,
          "exact-retry",
          "uncommitted retry must match attempted bytes",
          state,
        );
        acceptCanonical(
          state,
          action.lane,
          "Accepted",
          fullDurabilityProof(action.streamKind),
          action.binding,
          ["Pending", "Rejected", "OutcomeUncertain"],
        );
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
        reason: "DerivedCacheLag",
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
        lane.outstandingEffects.push(
          makeEffectIdentity(action.lane, lane.owner, lane.remoteAttempt),
        );
        lane.duplicateCostEgressRisk = true;
        addDiagnostic(state, "AutomaticReissueDuplicateRisk", {
          lane: action.lane,
          reason: "DuplicateCostEgressRisk",
          sessionEpoch: state.session.epoch,
          lease: state.session.lease,
          jobId: lane.owner.jobId,
          effect: lane.outstandingEffects.at(-1),
        });
      } else {
        lane.scheduler = "AwaitingDecision";
        addDiagnostic(state, "RemoteEffectNeedsDecision", {
          lane: action.lane,
          reason: "ExternalEffectUnknown",
          sessionEpoch: state.session.epoch,
          lease: state.session.lease,
          jobId: lane.owner.jobId,
          effect: lane.outstandingEffects.at(-1),
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
      state.session.pendingRemoteEffects = [];
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
        reason: "RetiredOwner",
        sessionEpoch: action.owner?.epoch,
        lease: action.owner?.lease,
        jobId: action.owner?.jobId,
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
      state.session.pendingRemoteEffects = [];
      for (const laneName of LANES) {
        const deletingLane = state.lanes[laneName];
        if (deletingLane.owner !== null) {
          state.detachedOwners.push(deletingLane.owner);
        }
        state.session.pendingRemoteEffects.push(
          ...deletingLane.outstandingEffects.map((effect) => structuredClone(effect)),
        );
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
          state.session.pendingRemoteEffects.length === 0,
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
      invariant(
        lane.admission.pendingBinding === null,
        "pending-receipt-binding",
        `${where} retained a Pending binding after ${laneName} committed`,
        state,
      );
    }
    if (lane.admission.lastReceipt === "Pending") {
      invariant(
        lane.admission.pendingBinding !== null &&
          (state.session.lifecycle !== "Active" ||
            bindingsEqual(
              lane.admission.pendingBinding,
              makeReceiptBinding(state, laneName, "Pending"),
            )),
        "pending-receipt-binding",
        `${where} did not bind ${laneName} Pending to the current owner and prestate`,
        state,
      );
    }
    const effectKeys = lane.outstandingEffects.map((effect) => JSON.stringify(effect));
    invariant(
      lane.outstandingEffects.every((effect) =>
        effectIsStructurallyValid(effect, laneName),
      ) && new Set(effectKeys).size === effectKeys.length,
      "outstanding-effect-identity",
      `${where} retained a malformed or duplicate ${laneName} effect identity`,
      state,
    );
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

  const pendingEffectKeys = state.session.pendingRemoteEffects.map((effect) =>
    JSON.stringify(effect),
  );
  invariant(
    new Set(pendingEffectKeys).size === pendingEffectKeys.length &&
      state.session.pendingRemoteEffects.every(
        (effect) =>
          effectIsStructurallyValid(effect, effect.lane) &&
          effectIndex(state.lanes[effect.lane].outstandingEffects, effect) >= 0,
      ),
    "deletion-effect-identity",
    `${where} retained a malformed, duplicate, or unowned deletion effect`,
    state,
  );
  if (state.session.lifecycle === "Active") {
    invariant(
      state.session.pendingRemoteEffects.length === 0,
      "deletion-effect-identity",
      `${where} exposed deletion wait effects while Active`,
      state,
    );
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
      "reason",
      "sessionEpoch",
      "lease",
      "sequence",
      "attempt",
      "jobRef",
      "effectRef",
    ]);
    invariant(
      Object.keys(diagnostic).every((key) => allowedKeys.has(key)),
      "content-free-diagnostics",
      `${where} diagnostic used a content-bearing field`,
      state,
    );
    invariant(
      DIAGNOSTIC_CODES.includes(diagnostic.code) &&
        (diagnostic.lane === undefined || LANES.includes(diagnostic.lane)) &&
        (diagnostic.reason === undefined ||
          DIAGNOSTIC_REASONS.includes(diagnostic.reason)) &&
        ["sessionEpoch", "lease", "sequence", "attempt"].every(
          (key) =>
            diagnostic[key] === undefined ||
            (Number.isSafeInteger(diagnostic[key]) && diagnostic[key] >= 0),
        ) &&
        ["jobRef", "effectRef"].every(
          (key) =>
            diagnostic[key] === undefined ||
            /^opaque:[0-9a-f]{8}$/u.test(diagnostic[key]),
        ),
      "content-free-diagnostics",
      `${where} diagnostic violated its closed typed schema`,
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

function currentOutstandingEffect(state, laneName) {
  return structuredClone(state.lanes[laneName].outstandingEffects.at(-1));
}

function prepareAdmission(state, laneName) {
  let next = prepareRemote(state, laneName);
  next = reduce(next, {
    type: "RemoteResult",
    lane: laneName,
    owner: structuredClone(next.lanes[laneName].owner),
    effect: currentOutstandingEffect(next, laneName),
    result: "Success",
  });
  return reduce(next, { type: "BeginAdmission", lane: laneName });
}

function boundReceiptAction(state, laneName, status, streamKind = "New") {
  const lane = state.lanes[laneName];
  return {
    type: "Receipt",
    lane: laneName,
    status,
    proof: fullDurabilityProof(streamKind),
    binding: structuredClone(
      lane.admission.pendingBinding ?? makeReceiptBinding(state, laneName),
    ),
  };
}

function checkReceiptBindingRegressions() {
  const policy = {
    savedWording: "Saved",
    remoteReissue: "RequireDecision",
    deletion: "WaitForRemote",
  };
  const missedRefusals = [];

  let pending = prepareAdmission(initialState(policy), "Notes");
  const delayedReceipt = boundReceiptAction(pending, "Notes", "Accepted");
  const rotated = transition(pending, { type: "RotateSession" });
  try {
    transition(rotated, delayedReceipt);
    missedRefusals.push("delayed-pre-rotation-receipt");
  } catch (error) {
    assert.match(String(error), /\[receipt-binding\]/u);
  }

  pending = prepareAdmission(initialState(policy), "Notes");
  const deletingReceipt = boundReceiptAction(pending, "Notes", "Accepted");
  const deleting = transition(pending, { type: "BeginDelete" });
  try {
    transition(deleting, deletingReceipt);
    missedRefusals.push("accepted-during-deletion");
  } catch (error) {
    assert.match(String(error), /\[receipt-binding\]/u);
  }

  pending = prepareAdmission(initialState(policy), "Notes");
  const bogusStreamReceipt = boundReceiptAction(pending, "Notes", "Accepted");
  bogusStreamReceipt.proof = {
    ...fullDurabilityProof("New"),
    streamKind: "Bogus",
  };
  try {
    transition(pending, bogusStreamReceipt);
    missedRefusals.push("bogus-stream-kind");
  } catch (error) {
    assert.match(String(error), /\[accepted-requires-durability\]/u);
  }

  assert.deepEqual(
    missedRefusals,
    [],
    `receipt-boundary regressions accepted: ${missedRefusals.join(", ")}`,
  );
  invariantFamilies.add("receipt-binding-refusal");
  assertionCount += 3;
  return 3;
}

function checkReceiptForgeryMatrix() {
  const policy = {
    savedWording: "Saved",
    remoteReissue: "RequireDecision",
    deletion: "WaitForRemote",
  };
  const pending = prepareAdmission(initialState(policy), "Notes");
  const baseAction = boundReceiptAction(pending, "Notes", "Accepted");
  const mutations = [
    ["action-lane", (action) => (action.lane = "Graph")],
    ["binding-lane", (action) => (action.binding.lane = "Graph")],
    ["owner-session", (action) => (action.binding.owner.sessionKey = "session-forged")],
    ["owner-epoch", (action) => (action.binding.owner.epoch += 1)],
    ["owner-lease", (action) => (action.binding.owner.lease += 1)],
    ["owner-job", (action) => (action.binding.owner.jobId = "forged-job")],
    ["session-key", (action) => (action.binding.sessionKey = "session-forged")],
    ["session-epoch", (action) => (action.binding.sessionEpoch += 1)],
    ["lease", (action) => (action.binding.lease += 1)],
    ["event-id", (action) => (action.binding.eventId = "forged-event")],
    ["digest", (action) => (action.binding.digest = "forged-digest")],
    ["lifecycle", (action) => (action.binding.expectedLifecycle = "Deleting")],
    ["prestate", (action) => (action.binding.expectedPrestate = "None")],
  ];
  const missedRefusals = [];
  for (const [name, mutate] of mutations) {
    const action = structuredClone(baseAction);
    mutate(action);
    try {
      transition(pending, action);
      missedRefusals.push(name);
    } catch (error) {
      assert.match(String(error), /\[receipt-binding\]/u);
    }
  }

  const idle = initialState(policy);
  try {
    transition(idle, {
      type: "Receipt",
      lane: "Notes",
      status: "Accepted",
      proof: fullDurabilityProof("New"),
      binding: makeReceiptBinding(idle, "Notes"),
    });
    missedRefusals.push("idle-prestate");
  } catch (error) {
    assert.match(String(error), /\[receipt-binding\]/u);
  }

  assert.deepEqual(
    missedRefusals,
    [],
    `receipt-forgery matrix accepted: ${missedRefusals.join(", ")}`,
  );
  assertionCount += 14;
  return 14;
}

function prototypeEffectIdentity(state, laneName, owner = undefined, attempt = undefined) {
  const lane = state.lanes[laneName];
  const effectOwner = structuredClone(owner ?? lane.owner);
  const effectAttempt = attempt ?? lane.remoteAttempt;
  return makeEffectIdentity(laneName, effectOwner, effectAttempt);
}

function waitEffectCount(state) {
  return state.session.pendingRemoteEffects.length;
}

function checkWaitEffectIdentityRegressions() {
  const policy = {
    savedWording: "Saved",
    remoteReissue: "AutomaticAtRisk",
    deletion: "WaitForRemote",
  };
  const missedRefusals = [];

  let twoAttempt = prepareRemote(initialState(policy), "Notes");
  const firstEffect = prototypeEffectIdentity(twoAttempt, "Notes");
  twoAttempt = reduce(twoAttempt, { type: "Restart" });
  twoAttempt = reduce(twoAttempt, { type: "RecoverExternalUnknown", lane: "Notes" });
  const secondEffect = prototypeEffectIdentity(twoAttempt, "Notes");
  twoAttempt = transition(twoAttempt, { type: "BeginDelete" });
  if (waitEffectCount(twoAttempt) !== 2) missedRefusals.push("two-attempt-registration");

  const afterFirst = transition(twoAttempt, {
    type: "RemoteResult",
    lane: "Notes",
    owner: firstEffect.owner,
    effect: firstEffect,
    result: "Success",
  });
  if (waitEffectCount(afterFirst) !== 1) missedRefusals.push("exact-first-attempt");
  const afterFirstReplay = transition(afterFirst, {
    type: "RemoteResult",
    lane: "Notes",
    owner: firstEffect.owner,
    effect: firstEffect,
    result: "Failure",
  });
  if (waitEffectCount(afterFirstReplay) !== 1) {
    missedRefusals.push("replayed-first-attempt");
  }
  const afterSecond = transition(afterFirstReplay, {
    type: "RemoteResult",
    lane: "Notes",
    owner: secondEffect.owner,
    effect: secondEffect,
    result: "Failure",
  });
  if (waitEffectCount(afterSecond) !== 0) missedRefusals.push("exact-second-attempt");

  for (const [field, value] of [
    ["lane", "Graph"],
    ["sessionKey", "session-forged"],
    ["sessionEpoch", firstEffect.sessionEpoch + 1],
    ["lease", firstEffect.lease + 1],
    ["resultRef", "forged-result-ref"],
  ]) {
    const deleting = transition(prepareRemote(initialState(policy), "Notes"), {
      type: "BeginDelete",
    });
    const forgedEffect = { ...firstEffect, [field]: value };
    const refused = transition(deleting, {
      type: "RemoteResult",
      lane: "Notes",
      owner: firstEffect.owner,
      effect: forgedEffect,
      result: "Success",
    });
    if (waitEffectCount(refused) !== 1) missedRefusals.push(`mismatched-${field}`);
  }

  let invalidResult = prepareRemote(initialState(policy), "Notes");
  const invalidResultEffect = currentOutstandingEffect(invalidResult, "Notes");
  invalidResult = transition(invalidResult, { type: "BeginDelete" });
  invalidResult = transition(invalidResult, {
    type: "RemoteResult",
    lane: "Notes",
    owner: invalidResultEffect.owner,
    effect: invalidResultEffect,
    result: "ForgedTerminal",
  });
  if (waitEffectCount(invalidResult) !== 1) {
    missedRefusals.push("invalid-result-kind");
  }

  let crossLane = prepareRemote(initialState(policy), "Notes");
  const staleNotesEffect = prototypeEffectIdentity(crossLane, "Notes");
  crossLane = prepareRemote(crossLane, "Graph");
  crossLane = transition(crossLane, { type: "BeginDelete" });
  const crossLaneRefused = transition(crossLane, {
    type: "RemoteResult",
    lane: "Graph",
    owner: staleNotesEffect.owner,
    effect: staleNotesEffect,
    result: "Success",
  });
  if (waitEffectCount(crossLaneRefused) !== 2) {
    missedRefusals.push("cross-lane-stale-token");
  }

  assert.deepEqual(
    missedRefusals,
    [],
    `wait-effect regressions failed: ${missedRefusals.join(", ")}`,
  );
  invariantFamilies.add("wait-effect-exactness");
  assertionCount += 11;
  return 11;
}

function checkStructuralDiagnosticRegression() {
  const policy = {
    savedWording: "Saved",
    remoteReissue: "RequireDecision",
    deletion: "DiscardImmediately",
  };
  let state = prepareRemote(initialState(policy), "Notes");
  const originalEffect = currentOutstandingEffect(state, "Notes");
  const maliciousOwner = {
    ...structuredClone(originalEffect.owner),
    jobId: "transcript prompt bearer credential payload secret",
  };
  const maliciousEffect = {
    ...structuredClone(originalEffect),
    jobId: maliciousOwner.jobId,
    resultRef: "content audio secret result",
    owner: maliciousOwner,
  };
  state = transition(state, { type: "RotateSession" });
  const maliciousResult = transition(state, {
    type: "RemoteResult",
    lane: "Notes",
    owner: maliciousOwner,
    effect: maliciousEffect,
    result: "secret transcript result",
  });
  const maliciousWriter = transition(state, {
    type: "DetachedWrite",
    lane: "Notes",
    owner: maliciousOwner,
  });

  const violations = [];
  for (const [name, diagnostic] of [
    ["result", maliciousResult.diagnostics.last],
    ["writer", maliciousWriter.diagnostics.last],
  ]) {
    const keys = Object.keys(diagnostic);
    if (keys.includes("jobId") || keys.includes("status") || keys.includes("result")) {
      violations.push(`${name}-arbitrary-field`);
    }
    if (/transcript|prompt|bearer|credential|payload|secret|content|audio/iu.test(JSON.stringify(diagnostic))) {
      violations.push(`${name}-raw-value`);
    }
    for (const key of ["jobRef", "effectRef"]) {
      if (key in diagnostic && !/^opaque:[0-9a-f]{8}$/u.test(diagnostic[key])) {
        violations.push(`${name}-${key}-not-opaque`);
      }
    }
  }

  assert.deepEqual(
    violations,
    [],
    `diagnostic regressions leaked structure: ${violations.join(", ")}`,
  );
  invariantFamilies.add("structural-diagnostic-regression");
  assertionCount += 2;
  return 2;
}

function applyReceiptPath(state, laneName, status, streamKind = "New") {
  let next = prepareAdmission(state, laneName);
  if (status === "Pending") return next;
  if (status === "Accepted") {
    return reduce(next, boundReceiptAction(next, laneName, status, streamKind));
  }
  if (status === "AlreadyAccepted") {
    next = reduce(next, boundReceiptAction(next, laneName, "Accepted", streamKind));
    return reduce(next, {
      type: "Retry",
      lane: laneName,
      eventId: next.lanes[laneName].admission.eventId,
      digest: next.lanes[laneName].admission.digest,
      streamKind,
      binding: makeReceiptBinding(next, laneName),
    });
  }
  return reduce(next, boundReceiptAction(next, laneName, status, streamKind));
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
                state = reduce(
                  state,
                  boundReceiptAction(state, laneName, callerStatus, streamKind),
                );
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
                binding: makeReceiptBinding(state, laneName),
              });
            } else if (cut !== "BeforeEnqueue" || diskOutcome !== "Absent") {
              state = reduce(state, {
                type: "ReconcileCrash",
                lane: laneName,
                streamKind,
                diskOutcome,
                binding: makeReceiptBinding(state, laneName),
              });
            }

            if (state.lanes[laneName].admission.committed === null) {
              state = reduce(state, {
                type: "Retry",
                lane: laneName,
                eventId: state.lanes[laneName].admission.eventId,
                digest: state.lanes[laneName].admission.digest,
                streamKind,
                binding: makeReceiptBinding(state, laneName),
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
        binding: makeReceiptBinding(exact, laneName),
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
        binding: makeReceiptBinding(exact, laneName),
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
      const detachedEffect = currentOutstandingEffect(unknown, laneName);
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
        effect: detachedEffect,
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
        const oldEffects = Object.fromEntries(
          LANES.map((laneName) => [laneName, currentOutstandingEffect(rotating, laneName)]),
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
            effect: oldEffects[laneName],
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
        const deletingEffects = Object.fromEntries(
          LANES.map((laneName) => [laneName, currentOutstandingEffect(deleting, laneName)]),
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
              deleting.session.pendingRemoteEffects.length === 2,
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
            effect: deletingEffects[laneName],
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
              deleting.session.pendingRemoteEffects.length === 0,
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
            outstandingEffects: lane.outstandingEffects,
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
    effect: currentOutstandingEffect(state, "Notes"),
    result: "Success",
  });
  traceStep("3 remote result ready", state);
  state = reduce(state, { type: "BeginAdmission", lane: "Notes" });
  traceStep("4 canonical Pending", state);
  state = reduce(state, boundReceiptAction(state, "Notes", "OutcomeUncertain"));
  traceStep("5 crash after file sync but before new-file directory sync/ack", state);
  state = reduce(state, {
    type: "ReconcileCrash",
    lane: "Notes",
    streamKind: "New",
    diskOutcome: "DurableExact",
    binding: makeReceiptBinding(state, "Notes"),
  });
  traceStep("6 exact reopen/retry returns AlreadyAccepted", state);
  state = reduce(state, { type: "SnapshotFailure", lane: "Notes" });
  traceStep("7 derived cache lags without rolling back canonical state", state);
  const oldOwner = structuredClone(state.lanes.Notes.owner);
  const oldEffect = makeEffectIdentity("Notes", oldOwner, 1);
  state = reduce(state, { type: "RotateSession" });
  traceStep("8 epoch and lease replaced", state);
  state = reduce(state, {
    type: "RemoteResult",
    lane: "Notes",
    owner: oldOwner,
    effect: oldEffect,
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

const correctionRegressionCases =
  checkReceiptBindingRegressions() +
  checkReceiptForgeryMatrix() +
  checkWaitEffectIdentityRegressions() +
  checkStructuralDiagnosticRegression();
showRepresentativeTrace();

const counts = {
  correctionRegressionCases,
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
console.log(`cases explored: ${caseCount + correctionRegressionCases}`);
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
