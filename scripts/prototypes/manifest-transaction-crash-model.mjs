#!/usr/bin/env bun

/**
 * THROWAWAY LOGIC PROTOTYPE — audio-graph-661f. Never import into production.
 *
 * Question: which physical persisted Session Artifact manifest transaction is
 * simplest while remaining crash-correct: a versioned atomic snapshot with
 * generation CAS, an append-only manifest log, or a log plus materialized view?
 *
 * This is a finite, in-memory, metadata-only model. It models named durability
 * barriers rather than real filesystem calls. All identifiers and diagnostics
 * are content-free. The executable compares all three candidates under the
 * same transaction, crash, fault, restart, retry, and concurrency scenarios.
 *
 * Run: bun scripts/prototypes/manifest-transaction-crash-model.mjs
 */

import assert from "node:assert/strict";

const FORMS = ["AtomicSnapshotCas", "AppendOnlyLog", "LogWithMaterializedView"];
const PHASES = ["Absent", "Prepared", "Completed"];
const TARGET_SOURCE_LENGTH = 60;
const INITIAL_SOURCE_LENGTH = 100;
const TX_A = Object.freeze({
  id: "tx-a",
  fingerprint: "fp-a",
  expectedGeneration: 0,
  writer: "writer-a",
});
const TX_B = Object.freeze({
  id: "tx-b",
  fingerprint: "fp-b",
  expectedGeneration: 0,
  writer: "writer-b",
});

const CRASH_CUTS = [
  "BeforePrepare",
  "AfterPrepare",
  "AfterQuarantinePublish",
  "AfterQuarantineNamespaceDurability",
  "AfterManifestAcceptance",
  "AfterSourceTruncate",
  "AfterSourceSync",
  "AfterCompletionPersistence",
  "PreAcknowledgement",
  "PostAcknowledgement",
];

const STAGES = [
  "None",
  "Prepare",
  "QuarantinePublish",
  "QuarantineNamespaceDurability",
  "ManifestAcceptance",
  "SourceTruncate",
  "SourceSync",
  "CompletionPersistence",
  "Acknowledgement",
  "Restart",
  "GenerationCheck",
  "Lock",
];

const OUTCOMES = [
  "Idle",
  "InProgress",
  "Accepted",
  "AlreadyCompleted",
  "Contended",
  "DurabilityIndeterminate",
  "GenerationConflict",
  "IdempotencyConflict",
  "IoFailedBeforeAcceptance",
  "NamespaceDurabilityUnsupported",
  "ReadyToAcknowledge",
  "RecoveryRequired",
];

const DIAGNOSTIC_CODES = [
  "GenerationConflict",
  "IdempotencyConflict",
  "ManifestTailQuarantined",
  "NamespaceBarrierIndeterminate",
  "SnapshotTempQuarantined",
  "VisibleMutationIndeterminate",
];

let transitionCount = 0;
let caseCount = 0;
let assertionCount = 0;
const invariantFamilies = new Set();
const observedStates = new Set();
const formEvidence = Object.fromEntries(
  FORMS.map((form) => [
    form,
    {
      crashCases: 0,
      faultCases: 0,
      concurrencyCases: 0,
      residualStates: new Set(),
      restartRepairs: 0,
      exactRetries: 0,
    },
  ]),
);

function invariant(condition, family, message, state = undefined) {
  invariantFamilies.add(family);
  assertionCount += 1;
  if (condition) return;
  const detail = state === undefined ? "" : `\nFull state:\n${JSON.stringify(state, null, 2)}`;
  throw new Error(`[${family}] ${message}${detail}`);
}

function guard(condition, code, state) {
  if (condition) return;
  throw new Error(`[${code}] illegal model transition\n${JSON.stringify(state, null, 2)}`);
}

function opaqueRef(value) {
  const input = String(value ?? "none");
  let hash = 0x811c9dc5;
  for (let index = 0; index < input.length; index += 1) {
    hash ^= input.charCodeAt(index);
    hash = Math.imul(hash, 0x01000193);
  }
  return `opaque:${(hash >>> 0).toString(16).padStart(8, "0")}`;
}

function manifestRecord(generation, phase, tx) {
  return {
    generation,
    schemaVersion: 1,
    phase,
    txId: tx.id,
    fingerprint: tx.fingerprint,
    quarantineRef: opaqueRef(`${tx.id}:quarantine`),
    sourceLength: phase === "Prepared" ? INITIAL_SOURCE_LENGTH : TARGET_SOURCE_LENGTH,
  };
}

function emptyHead() {
  return {
    generation: 0,
    schemaVersion: 1,
    phase: "Absent",
    txId: null,
    fingerprint: null,
    quarantineRef: null,
    sourceLength: INITIAL_SOURCE_LENGTH,
  };
}

function initialState(form, { namespaceSupported = true } = {}) {
  guard(FORMS.includes(form), "unknown-form", { form });
  return {
    model: "audio-graph-661f-manifest-transaction-v1",
    form,
    capability: {
      namespaceSupported,
      barrier: namespaceSupported ? "FileAndNamespace" : "FileOnly",
    },
    durable: {
      sourceLength: INITIAL_SOURCE_LENGTH,
      quarantine: {
        name: "Missing",
        quality: "Absent",
        bytesDurable: false,
        namespaceDurable: false,
        txId: null,
        fingerprint: null,
      },
      snapshot: {
        head: emptyHead(),
        replacementTemp: null,
        quarantinedTempRefs: [],
      },
      log: {
        records: [],
        tornTail: null,
        quarantinedTailRefs: [],
      },
      view: {
        head: emptyHead(),
        status: form === "LogWithMaterializedView" ? "Exact" : "Unused",
      },
    },
    runtime: {
      lockOwner: null,
      activeTx: null,
      sourceVisibleLength: INITIAL_SOURCE_LENGTH,
      acknowledgementDelivered: false,
      restartCount: 0,
    },
    result: {
      kind: "Idle",
      stage: "None",
      expectedGeneration: null,
      actualGeneration: null,
      txRef: null,
    },
    recovery: {
      required: false,
      residual: "CleanSourceFull",
      priorIndeterminateStage: null,
    },
    counters: {
      manifestAdvancements: 0,
      authorityWrites: 0,
      materializedViewWrites: 0,
      restartRepairs: 0,
    },
    diagnostics: [],
  };
}

function head(state) {
  if (state.form === "AtomicSnapshotCas") return state.durable.snapshot.head;
  return state.durable.log.records.at(-1) ?? emptyHead();
}

function recordsForTx(state, txId) {
  if (state.form === "AtomicSnapshotCas") {
    return state.durable.snapshot.head.txId === txId ? [state.durable.snapshot.head] : [];
  }
  return state.durable.log.records.filter((record) => record.txId === txId);
}

function completedRecord(state, tx) {
  return recordsForTx(state, tx.id).find(
    (record) => record.phase === "Completed" && record.fingerprint === tx.fingerprint,
  );
}

function preparedRecord(state, tx) {
  return recordsForTx(state, tx.id).find(
    (record) => record.phase === "Prepared" && record.fingerprint === tx.fingerprint,
  );
}

function exactTxFingerprint(state, tx) {
  const records = recordsForTx(state, tx.id);
  return records.length === 0 || records.every((record) => record.fingerprint === tx.fingerprint);
}

function addDiagnostic(state, code, stage, txId = null) {
  guard(DIAGNOSTIC_CODES.includes(code), "diagnostic-code", state);
  guard(STAGES.includes(stage), "diagnostic-stage", state);
  state.diagnostics.push({
    code,
    stage,
    form: state.form,
    ...(txId === null ? {} : { txRef: opaqueRef(txId) }),
  });
}

function setResult(state, kind, stage, tx = null, generations = {}) {
  guard(OUTCOMES.includes(kind), "outcome-kind", state);
  guard(STAGES.includes(stage), "outcome-stage", state);
  state.result = {
    kind,
    stage,
    expectedGeneration: generations.expected ?? null,
    actualGeneration: generations.actual ?? null,
    txRef: tx === null ? null : opaqueRef(tx.id),
  };
}

function residualState(state) {
  const logical = head(state);
  const quarantine = state.durable.quarantine;
  const source = state.durable.sourceLength;

  if (state.durable.log.tornTail !== null) return `TornManifestTail:${logical.phase}`;
  if (state.durable.snapshot.replacementTemp !== null) {
    return `SnapshotReplacementTemp:${logical.phase}`;
  }
  if (logical.phase === "Completed" && source === TARGET_SOURCE_LENGTH) {
    return "CompletedSourceTruncated";
  }
  if (logical.phase === "Prepared" && source === TARGET_SOURCE_LENGTH) {
    return "PreparedSourceTruncated";
  }
  if (logical.phase === "Prepared" && source === INITIAL_SOURCE_LENGTH) {
    return "PreparedSourceFull";
  }
  if (logical.phase === "Absent" && source === INITIAL_SOURCE_LENGTH) {
    if (quarantine.name === "Missing") return "CleanSourceFull";
    if (quarantine.name === "Temp" && quarantine.quality === "Partial") {
      return "PartialTempSourceFull";
    }
    if (quarantine.name === "Temp") return "TempOnlySourceFull";
    if (quarantine.name === "Final" && !quarantine.namespaceDurable) {
      return "PublishedNamespaceUncertainSourceFull";
    }
    if (quarantine.name === "Final" && quarantine.namespaceDurable) {
      return "QuarantineOnlySourceFull";
    }
  }
  return "InvalidResidual";
}

function diagnosticIsContentFree(diagnostic) {
  const keys = Object.keys(diagnostic).sort();
  const allowedKeys = ["code", "form", "stage", "txRef"];
  return (
    keys.every((key) => allowedKeys.includes(key)) &&
    DIAGNOSTIC_CODES.includes(diagnostic.code) &&
    FORMS.includes(diagnostic.form) &&
    STAGES.includes(diagnostic.stage) &&
    (diagnostic.txRef === undefined || /^opaque:[0-9a-f]{8}$/.test(diagnostic.txRef))
  );
}

function verifyState(state, label) {
  const logical = head(state);
  const quarantine = state.durable.quarantine;
  const source = state.durable.sourceLength;

  invariant(FORMS.includes(state.form), "known-physical-form", `${label}: unknown form`, state);
  invariant(PHASES.includes(logical.phase), "known-manifest-phase", `${label}: unknown phase`, state);
  invariant(
    Number.isSafeInteger(logical.generation) && logical.generation >= 0,
    "generation-is-monotonic-integer",
    `${label}: invalid generation`,
    state,
  );
  invariant(
    [INITIAL_SOURCE_LENGTH, TARGET_SOURCE_LENGTH].includes(source),
    "exact-source-residual",
    `${label}: unexpected durable source length`,
    state,
  );
  invariant(
    [INITIAL_SOURCE_LENGTH, TARGET_SOURCE_LENGTH].includes(state.runtime.sourceVisibleLength),
    "exact-visible-source-state",
    `${label}: unexpected visible source length`,
    state,
  );
  invariant(
    !(logical.phase !== "Absent" && !quarantine.namespaceDurable),
    "quarantine-durable-before-manifest",
    `${label}: manifest advanced before quarantine namespace durability`,
    state,
  );
  invariant(
    !(source === TARGET_SOURCE_LENGTH && logical.phase === "Absent"),
    "manifest-prepared-before-truncate",
    `${label}: source durable truncate lacks prepared manifest`,
    state,
  );
  invariant(
    !(
      state.runtime.sourceVisibleLength === TARGET_SOURCE_LENGTH &&
      (!quarantine.namespaceDurable || logical.phase === "Absent")
    ),
    "quarantine-durable-before-truncate",
    `${label}: visible source truncate outran quarantine or prepared manifest durability`,
    state,
  );
  invariant(
    !(logical.phase === "Completed" && source !== TARGET_SOURCE_LENGTH),
    "source-sync-before-completion",
    `${label}: completion outran source sync`,
    state,
  );
  invariant(
    !(state.runtime.acknowledgementDelivered && logical.phase !== "Completed"),
    "completion-before-acknowledgement",
    `${label}: acknowledgement outran completion`,
    state,
  );
  invariant(
    !(state.result.kind === "Accepted" && logical.phase !== "Completed"),
    "accepted-only-after-completion",
    `${label}: Accepted without durable completion`,
    state,
  );
  invariant(
    !(state.result.kind === "Accepted" && !quarantine.namespaceDurable),
    "accepted-requires-quarantine-namespace",
    `${label}: Accepted without quarantine namespace durability`,
    state,
  );
  invariant(
    !(state.result.kind === "Accepted" && source !== TARGET_SOURCE_LENGTH),
    "accepted-requires-source-sync",
    `${label}: Accepted without durable source truncate`,
    state,
  );
  invariant(
    !(state.result.kind === "Accepted" && !state.capability.namespaceSupported),
    "unsupported-namespace-never-accepted",
    `${label}: unsupported namespace reached Accepted`,
    state,
  );
  invariant(
    state.diagnostics.every(diagnosticIsContentFree),
    "content-free-diagnostics",
    `${label}: diagnostic copied unrestricted content`,
    state,
  );
  invariant(
    residualState(state) !== "InvalidResidual",
    "exact-residual-state",
    `${label}: state cannot be classified exactly`,
    state,
  );

  if (state.form === "AtomicSnapshotCas") {
    invariant(
      state.durable.log.records.length === 0 && state.durable.log.tornTail === null,
      "single-authority-form",
      `${label}: snapshot form used a log`,
      state,
    );
  } else {
    const generations = state.durable.log.records.map((record) => record.generation);
    invariant(
      generations.every((generation, index) => generation === index + 1),
      "log-generation-contiguity",
      `${label}: log generations are not contiguous`,
      state,
    );
    for (const txId of new Set(state.durable.log.records.map((record) => record.txId))) {
      const phases = state.durable.log.records
        .filter((record) => record.txId === txId)
        .map((record) => record.phase);
      invariant(
        new Set(phases).size === phases.length,
        "no-duplicate-manifest-advancement",
        `${label}: duplicate manifest phase for one transaction`,
        state,
      );
    }
  }

  if (state.form === "LogWithMaterializedView" && state.durable.view.status === "Exact") {
    invariant(
      JSON.stringify(state.durable.view.head) === JSON.stringify(logical),
      "materialized-view-is-never-authority",
      `${label}: exact view differs from canonical log`,
      state,
    );
  }
}

function replaceManifest(state, record, { updateView = true } = {}) {
  const current = head(state);
  guard(record.generation === current.generation + 1, "generation-gap", state);

  if (state.form === "AtomicSnapshotCas") {
    state.durable.snapshot.head = structuredClone(record);
  } else {
    state.durable.log.records.push(structuredClone(record));
    if (state.form === "LogWithMaterializedView") {
      if (updateView) {
        state.durable.view.head = structuredClone(record);
        state.durable.view.status = "Exact";
        state.counters.materializedViewWrites += 1;
      } else {
        state.durable.view.status = "Lagging";
      }
    }
  }
  state.counters.manifestAdvancements += 1;
  state.counters.authorityWrites += 1;
}

function candidateRecord(state, phase, tx) {
  return manifestRecord(head(state).generation + 1, phase, tx);
}

function sameQuarantineTransaction(state, tx) {
  const quarantine = state.durable.quarantine;
  return quarantine.txId === tx.id && quarantine.fingerprint === tx.fingerprint;
}

function applyTransition(state, action) {
  switch (action.type) {
    case "Prepare": {
      const tx = action.tx;
      if (!state.capability.namespaceSupported) {
        setResult(state, "NamespaceDurabilityUnsupported", "Prepare", tx);
        return;
      }
      if (state.runtime.lockOwner !== null && state.runtime.lockOwner !== tx.writer) {
        setResult(state, "Contended", "Lock", tx);
        return;
      }
      if (!exactTxFingerprint(state, tx)) {
        addDiagnostic(state, "IdempotencyConflict", "GenerationCheck", tx.id);
        setResult(state, "IdempotencyConflict", "GenerationCheck", tx);
        return;
      }
      if (completedRecord(state, tx) !== undefined) {
        setResult(state, "AlreadyCompleted", "GenerationCheck", tx);
        return;
      }

      const prepared = preparedRecord(state, tx);
      const current = head(state);
      if (prepared === undefined && current.generation !== tx.expectedGeneration) {
        addDiagnostic(state, "GenerationConflict", "GenerationCheck", tx.id);
        setResult(state, "GenerationConflict", "GenerationCheck", tx, {
          expected: tx.expectedGeneration,
          actual: current.generation,
        });
        return;
      }

      state.runtime.lockOwner = tx.writer;
      state.runtime.activeTx = structuredClone(tx);
      state.runtime.acknowledgementDelivered = false;

      if (prepared !== undefined) {
        setResult(state, "InProgress", "ManifestAcceptance", tx);
        return;
      }

      const quarantine = state.durable.quarantine;
      if (quarantine.name !== "Missing" && !sameQuarantineTransaction(state, tx)) {
        addDiagnostic(state, "IdempotencyConflict", "Prepare", tx.id);
        setResult(state, "IdempotencyConflict", "Prepare", tx);
        state.runtime.lockOwner = null;
        state.runtime.activeTx = null;
        return;
      }

      if (quarantine.name === "Missing" || quarantine.quality === "Partial") {
        state.durable.quarantine = {
          name: "Temp",
          quality: "Exact",
          bytesDurable: true,
          namespaceDurable: false,
          txId: tx.id,
          fingerprint: tx.fingerprint,
        };
      }
      setResult(state, "InProgress", "Prepare", tx);
      return;
    }

    case "PublishQuarantine": {
      const tx = state.runtime.activeTx;
      guard(tx !== null && state.runtime.lockOwner === tx.writer, "publish-without-lock", state);
      const quarantine = state.durable.quarantine;
      guard(
        quarantine.name === "Temp" && quarantine.quality === "Exact" && sameQuarantineTransaction(state, tx),
        "publish-without-exact-temp",
        state,
      );
      quarantine.name = "Final";
      quarantine.namespaceDurable = false;
      setResult(state, "InProgress", "QuarantinePublish", tx);
      return;
    }

    case "SyncQuarantineNamespace": {
      const tx = state.runtime.activeTx;
      guard(tx !== null && state.runtime.lockOwner === tx.writer, "sync-namespace-without-lock", state);
      const quarantine = state.durable.quarantine;
      guard(
        quarantine.name === "Final" && quarantine.quality === "Exact" && sameQuarantineTransaction(state, tx),
        "sync-namespace-without-final",
        state,
      );
      if (action.result === "UnsupportedAfterMutation") {
        addDiagnostic(state, "NamespaceBarrierIndeterminate", "QuarantineNamespaceDurability", tx.id);
        state.recovery.priorIndeterminateStage = "QuarantineNamespaceDurability";
        setResult(state, "DurabilityIndeterminate", "QuarantineNamespaceDurability", tx);
        return;
      }
      quarantine.namespaceDurable = true;
      setResult(state, "InProgress", "QuarantineNamespaceDurability", tx);
      return;
    }

    case "PersistPrepared": {
      const tx = state.runtime.activeTx;
      guard(tx !== null && state.runtime.lockOwner === tx.writer, "prepare-manifest-without-lock", state);
      guard(state.durable.quarantine.namespaceDurable, "manifest-before-quarantine-durable", state);
      const current = head(state);
      if (current.generation !== tx.expectedGeneration) {
        addDiagnostic(state, "GenerationConflict", "ManifestAcceptance", tx.id);
        setResult(state, "GenerationConflict", "ManifestAcceptance", tx, {
          expected: tx.expectedGeneration,
          actual: current.generation,
        });
        return;
      }
      replaceManifest(state, candidateRecord(state, "Prepared", tx));
      setResult(state, "InProgress", "ManifestAcceptance", tx);
      return;
    }

    case "TruncateSource": {
      const tx = state.runtime.activeTx;
      guard(tx !== null && state.runtime.lockOwner === tx.writer, "truncate-without-lock", state);
      const prepared = preparedRecord(state, tx);
      guard(prepared !== undefined, "truncate-without-prepared-manifest", state);
      state.runtime.sourceVisibleLength = TARGET_SOURCE_LENGTH;
      setResult(state, "InProgress", "SourceTruncate", tx);
      return;
    }

    case "SyncSource": {
      const tx = state.runtime.activeTx;
      guard(tx !== null && state.runtime.lockOwner === tx.writer, "source-sync-without-lock", state);
      guard(
        state.runtime.sourceVisibleLength === TARGET_SOURCE_LENGTH,
        "source-sync-before-visible-truncate",
        state,
      );
      state.durable.sourceLength = TARGET_SOURCE_LENGTH;
      setResult(state, "InProgress", "SourceSync", tx);
      return;
    }

    case "PersistCompleted": {
      const tx = state.runtime.activeTx;
      guard(tx !== null && state.runtime.lockOwner === tx.writer, "complete-without-lock", state);
      guard(state.durable.sourceLength === TARGET_SOURCE_LENGTH, "completion-before-source-sync", state);
      const current = head(state);
      const expectedPreparedGeneration = tx.expectedGeneration + 1;
      if (current.generation !== expectedPreparedGeneration) {
        addDiagnostic(state, "GenerationConflict", "CompletionPersistence", tx.id);
        setResult(state, "GenerationConflict", "CompletionPersistence", tx, {
          expected: expectedPreparedGeneration,
          actual: current.generation,
        });
        return;
      }
      guard(
        current.phase === "Prepared" &&
          current.txId === tx.id &&
          current.fingerprint === tx.fingerprint,
        "completion-without-exact-prepare",
        state,
      );
      replaceManifest(state, candidateRecord(state, "Completed", tx));
      setResult(state, "InProgress", "CompletionPersistence", tx);
      return;
    }

    case "ReadyToAcknowledge": {
      const tx = state.runtime.activeTx;
      guard(tx !== null && completedRecord(state, tx) !== undefined, "ready-before-completion", state);
      setResult(state, "ReadyToAcknowledge", "Acknowledgement", tx);
      return;
    }

    case "Acknowledge": {
      const tx = state.runtime.activeTx;
      guard(tx !== null && completedRecord(state, tx) !== undefined, "ack-before-completion", state);
      state.runtime.acknowledgementDelivered = true;
      state.runtime.lockOwner = null;
      state.runtime.activeTx = null;
      setResult(state, "Accepted", "Acknowledgement", tx);
      return;
    }

    case "Restart": {
      const priorIndeterminateStage = state.recovery.priorIndeterminateStage;
      const quarantine = state.durable.quarantine;
      if (!quarantine.namespaceDurable && action.quarantineObserved !== undefined) {
        const observation = action.quarantineObserved;
        guard(["Missing", "Temp", "Final"].includes(observation), "bad-quarantine-observation", state);
        quarantine.name = observation;
        quarantine.quality = observation === "Missing" ? "Absent" : quarantine.quality;
        quarantine.bytesDurable = observation !== "Missing" && quarantine.quality === "Exact";
        if (observation === "Missing") {
          quarantine.txId = null;
          quarantine.fingerprint = null;
        }
      }
      if (action.sourceObserved !== undefined) {
        guard(
          [INITIAL_SOURCE_LENGTH, TARGET_SOURCE_LENGTH].includes(action.sourceObserved),
          "bad-source-observation",
          state,
        );
        state.durable.sourceLength = action.sourceObserved;
      }
      state.runtime.sourceVisibleLength = state.durable.sourceLength;
      state.runtime.lockOwner = null;
      state.runtime.activeTx = null;
      state.runtime.acknowledgementDelivered = false;
      state.runtime.restartCount += 1;

      if (state.durable.log.tornTail !== null) {
        state.durable.log.quarantinedTailRefs.push(opaqueRef(state.durable.log.tornTail.txId));
        addDiagnostic(state, "ManifestTailQuarantined", "Restart", state.durable.log.tornTail.txId);
        state.durable.log.tornTail = null;
        state.counters.restartRepairs += 1;
      }
      if (state.durable.snapshot.replacementTemp !== null) {
        state.durable.snapshot.quarantinedTempRefs.push(
          opaqueRef(state.durable.snapshot.replacementTemp.txId),
        );
        addDiagnostic(
          state,
          "SnapshotTempQuarantined",
          "Restart",
          state.durable.snapshot.replacementTemp.txId,
        );
        state.durable.snapshot.replacementTemp = null;
        state.counters.restartRepairs += 1;
      }
      if (state.form === "LogWithMaterializedView") {
        const canonical = head(state);
        if (JSON.stringify(state.durable.view.head) !== JSON.stringify(canonical)) {
          state.counters.restartRepairs += 1;
        }
        state.durable.view.head = structuredClone(canonical);
        state.durable.view.status = "Exact";
      }
      state.recovery.required = residualState(state) !== "CompletedSourceTruncated";
      state.recovery.residual = residualState(state);
      state.recovery.priorIndeterminateStage = priorIndeterminateStage;
      setResult(state, "RecoveryRequired", "Restart", action.tx ?? null);
      return;
    }

    default:
      throw new Error(`unknown action type: ${action.type}`);
  }
}

function transition(input, action, label = action.type) {
  const state = structuredClone(input);
  transitionCount += 1;
  applyTransition(state, action);
  state.recovery.residual = residualState(state);
  state.recovery.required =
    state.recovery.residual !== "CompletedSourceTruncated" &&
    (state.runtime.restartCount > 0 || state.recovery.priorIndeterminateStage !== null);
  verifyState(state, label);
  observedStates.add(JSON.stringify(state));
  return state;
}

function runSuccessfulTransaction(form, { readyOnly = false } = {}) {
  let state = initialState(form);
  verifyState(state, "successful-initial");
  state = transition(state, { type: "Prepare", tx: TX_A });
  state = transition(state, { type: "PublishQuarantine" });
  state = transition(state, { type: "SyncQuarantineNamespace" });
  state = transition(state, { type: "PersistPrepared" });
  state = transition(state, { type: "TruncateSource" });
  state = transition(state, { type: "SyncSource" });
  state = transition(state, { type: "PersistCompleted" });
  state = transition(state, { type: "ReadyToAcknowledge" });
  if (!readyOnly) state = transition(state, { type: "Acknowledge" });
  return state;
}

function convergeExactRetry(input, tx = TX_A) {
  let state = transition(input, { type: "Prepare", tx }, "retry-prepare-or-resume");
  if (["AlreadyCompleted", "GenerationConflict", "IdempotencyConflict"].includes(state.result.kind)) {
    return state;
  }

  const quarantine = state.durable.quarantine;
  if (quarantine.name === "Temp") {
    state = transition(state, { type: "PublishQuarantine" }, "retry-publish");
  }
  if (state.durable.quarantine.name === "Final" && !state.durable.quarantine.namespaceDurable) {
    state = transition(state, { type: "SyncQuarantineNamespace" }, "retry-namespace-sync");
  }
  if (head(state).phase === "Absent") {
    state = transition(state, { type: "PersistPrepared" }, "retry-persist-prepare");
  }
  if (state.durable.sourceLength === INITIAL_SOURCE_LENGTH) {
    state = transition(state, { type: "TruncateSource" }, "retry-truncate");
    state = transition(state, { type: "SyncSource" }, "retry-source-sync");
  }
  if (head(state).phase === "Prepared") {
    state = transition(state, { type: "PersistCompleted" }, "retry-persist-completion");
  }
  state = transition(state, { type: "ReadyToAcknowledge" }, "retry-ready");
  state = transition(state, { type: "Acknowledge" }, "retry-ack");
  return state;
}

function stateForCrashCut(form, cut) {
  let state = initialState(form);
  const apply = (action) => {
    state = transition(state, action, `${cut}:${action.type}`);
  };
  if (cut === "BeforePrepare") return state;
  apply({ type: "Prepare", tx: TX_A });
  if (cut === "AfterPrepare") return state;
  apply({ type: "PublishQuarantine" });
  if (cut === "AfterQuarantinePublish") return state;
  apply({ type: "SyncQuarantineNamespace" });
  if (cut === "AfterQuarantineNamespaceDurability") return state;
  apply({ type: "PersistPrepared" });
  if (cut === "AfterManifestAcceptance") return state;
  apply({ type: "TruncateSource" });
  if (cut === "AfterSourceTruncate") return state;
  apply({ type: "SyncSource" });
  if (cut === "AfterSourceSync") return state;
  apply({ type: "PersistCompleted" });
  if (cut === "AfterCompletionPersistence") return state;
  apply({ type: "ReadyToAcknowledge" });
  if (cut === "PreAcknowledgement") return state;
  apply({ type: "Acknowledge" });
  guard(cut === "PostAcknowledgement", "unknown-crash-cut", { cut });
  return state;
}

function crashVariants(form, cut) {
  const base = [{}];
  if (cut === "AfterPrepare") {
    return [
      { quarantineObserved: "Missing" },
      { quarantineObserved: "Temp" },
    ];
  }
  if (cut === "AfterQuarantinePublish") {
    return [
      { quarantineObserved: "Missing" },
      { quarantineObserved: "Temp" },
      { quarantineObserved: "Final" },
    ];
  }
  if (cut === "AfterSourceTruncate") {
    return [
      { sourceObserved: INITIAL_SOURCE_LENGTH },
      { sourceObserved: TARGET_SOURCE_LENGTH },
    ];
  }
  if (
    form === "LogWithMaterializedView" &&
    ["AfterManifestAcceptance", "AfterCompletionPersistence", "PreAcknowledgement", "PostAcknowledgement"].includes(
      cut,
    )
  ) {
    return [{ viewStatus: "Exact" }, { viewStatus: "Lagging" }];
  }
  return base;
}

function expectedResidualAtCut(cut, variant) {
  if (cut === "BeforePrepare") return "CleanSourceFull";
  if (cut === "AfterPrepare") {
    return variant.quarantineObserved === "Missing" ? "CleanSourceFull" : "TempOnlySourceFull";
  }
  if (cut === "AfterQuarantinePublish") {
    if (variant.quarantineObserved === "Missing") return "CleanSourceFull";
    if (variant.quarantineObserved === "Temp") return "TempOnlySourceFull";
    return "PublishedNamespaceUncertainSourceFull";
  }
  if (cut === "AfterQuarantineNamespaceDurability") return "QuarantineOnlySourceFull";
  if (cut === "AfterManifestAcceptance") return "PreparedSourceFull";
  if (cut === "AfterSourceTruncate") {
    return variant.sourceObserved === TARGET_SOURCE_LENGTH
      ? "PreparedSourceTruncated"
      : "PreparedSourceFull";
  }
  if (cut === "AfterSourceSync") return "PreparedSourceTruncated";
  return "CompletedSourceTruncated";
}

function checkCrashMatrix() {
  let cases = 0;
  for (const form of FORMS) {
    for (const cut of CRASH_CUTS) {
      for (const variant of crashVariants(form, cut)) {
        let state = stateForCrashCut(form, cut);
        const preCrashGeneration = head(state).generation;
        const acknowledgementObserved = state.runtime.acknowledgementDelivered;
        if (variant.viewStatus === "Lagging") state.durable.view.status = "Lagging";
        state = transition(
          state,
          {
            type: "Restart",
            tx: TX_A,
            quarantineObserved: variant.quarantineObserved,
            sourceObserved: variant.sourceObserved,
          },
          `${form}:${cut}:restart`,
        );
        const expectedResidual = expectedResidualAtCut(cut, variant);
        invariant(
          state.recovery.residual === expectedResidual,
          "crash-cut-exact-residual",
          `${form}/${cut} expected ${expectedResidual}, got ${state.recovery.residual}`,
          state,
        );
        invariant(
          acknowledgementObserved === (cut === "PostAcknowledgement"),
          "pre-post-acknowledgement-cut",
          `${form}/${cut} acknowledgement observer mismatch`,
          state,
        );
        const beforeRetryAdvancements = state.counters.manifestAdvancements;
        state = convergeExactRetry(state);
        invariant(
          state.result.kind === (preCrashGeneration === 2 ? "AlreadyCompleted" : "Accepted"),
          "deterministic-exact-retry",
          `${form}/${cut} retry did not converge deterministically`,
          state,
        );
        invariant(
          head(state).phase === "Completed" &&
            state.durable.sourceLength === TARGET_SOURCE_LENGTH &&
            state.durable.quarantine.namespaceDurable,
          "crash-restart-converges",
          `${form}/${cut} did not converge to exact completed state`,
          state,
        );
        const expectedNewAdvancements = 2 - preCrashGeneration;
        invariant(
          state.counters.manifestAdvancements - beforeRetryAdvancements === expectedNewAdvancements,
          "no-duplicate-advancement-on-retry",
          `${form}/${cut} retry advanced manifest more than missing transitions`,
          state,
        );
        formEvidence[form].crashCases += 1;
        formEvidence[form].residualStates.add(expectedResidual);
        formEvidence[form].exactRetries += 1;
        caseCount += 1;
        cases += 1;
      }
    }
  }
  return cases;
}

function markIndeterminate(state, stage, tx = TX_A) {
  state.recovery.priorIndeterminateStage = stage;
  addDiagnostic(state, "VisibleMutationIndeterminate", stage, tx.id);
  setResult(state, "DurabilityIndeterminate", stage, tx);
}

function manifestFaultVariants(form) {
  if (form === "AtomicSnapshotCas") return ["OldWithTemp", "Exact"];
  if (form === "AppendOnlyLog") return ["Absent", "Torn", "Exact"];
  return ["AbsentViewLagging", "TornViewLagging", "ExactViewLagging", "ExactViewExact"];
}

function injectManifestFault(input, phase, variant) {
  const state = structuredClone(input);
  const tx = state.runtime.activeTx;
  guard(tx !== null, "manifest-fault-without-tx", state);
  const record = candidateRecord(state, phase, tx);

  if (state.form === "AtomicSnapshotCas") {
    if (variant === "OldWithTemp") {
      state.durable.snapshot.replacementTemp = structuredClone(record);
    } else {
      guard(variant === "Exact", "unknown-snapshot-fault", { variant });
      replaceManifest(state, record);
    }
  } else {
    if (variant.startsWith("Absent")) {
      if (state.form === "LogWithMaterializedView") state.durable.view.status = "Lagging";
    } else if (variant.startsWith("Torn")) {
      state.durable.log.tornTail = {
        txId: tx.id,
        fingerprintRef: opaqueRef(tx.fingerprint),
        intendedGeneration: record.generation,
        phase,
      };
      if (state.form === "LogWithMaterializedView") state.durable.view.status = "Lagging";
    } else {
      const updateView = variant === "ExactViewExact" || state.form === "AppendOnlyLog";
      replaceManifest(state, record, { updateView });
    }
  }
  markIndeterminate(state, phase === "Prepared" ? "ManifestAcceptance" : "CompletionPersistence");
  state.recovery.residual = residualState(state);
  verifyState(state, `inject-${phase}-${variant}`);
  observedStates.add(JSON.stringify(state));
  return state;
}

function faultBase(form, stage) {
  let state = initialState(form);
  if (stage === "Prepare") return state;
  state = transition(state, { type: "Prepare", tx: TX_A });
  if (stage === "QuarantinePublish") return state;
  state = transition(state, { type: "PublishQuarantine" });
  if (stage === "QuarantineNamespaceDurability") return state;
  state = transition(state, { type: "SyncQuarantineNamespace" });
  if (stage === "ManifestAcceptance") return state;
  state = transition(state, { type: "PersistPrepared" });
  if (stage === "SourceTruncate") return state;
  state = transition(state, { type: "TruncateSource" });
  if (stage === "SourceSync") return state;
  state = transition(state, { type: "SyncSource" });
  if (stage === "CompletionPersistence") return state;
  state = transition(state, { type: "PersistCompleted" });
  guard(stage === "Acknowledgement", "unknown-fault-stage", { stage });
  return state;
}

function faultVariants(form, stage) {
  if (stage === "Prepare") return ["NoMutation", "PartialTemp", "ExactTemp"];
  if (stage === "QuarantinePublish") return ["TempRemains", "FinalVisible"];
  if (stage === "QuarantineNamespaceDurability") return ["FinalUnbarriered"];
  if (stage === "ManifestAcceptance" || stage === "CompletionPersistence") {
    return manifestFaultVariants(form);
  }
  if (stage === "SourceTruncate" || stage === "SourceSync") return ["SourceFull", "SourceTruncated"];
  return ["AckNotObserved", "AckPossiblyObserved"];
}

function injectFault(input, stage, variant) {
  if (stage === "ManifestAcceptance") return injectManifestFault(input, "Prepared", variant);
  if (stage === "CompletionPersistence") return injectManifestFault(input, "Completed", variant);

  const state = structuredClone(input);
  const tx = state.runtime.activeTx ?? TX_A;
  if (stage === "Prepare" && variant === "NoMutation") {
    setResult(state, "IoFailedBeforeAcceptance", "Prepare", tx);
    state.recovery.residual = residualState(state);
    verifyState(state, `inject-${stage}-${variant}`);
    observedStates.add(JSON.stringify(state));
    return state;
  }
  if (stage === "Prepare") {
    if (variant === "PartialTemp") {
      state.durable.quarantine = {
        name: "Temp",
        quality: "Partial",
        bytesDurable: false,
        namespaceDurable: false,
        txId: tx.id,
        fingerprint: tx.fingerprint,
      };
    } else if (variant === "ExactTemp") {
      state.durable.quarantine = {
        name: "Temp",
        quality: "Exact",
        bytesDurable: true,
        namespaceDurable: false,
        txId: tx.id,
        fingerprint: tx.fingerprint,
      };
    }
  } else if (stage === "QuarantinePublish") {
    if (variant === "FinalVisible") state.durable.quarantine.name = "Final";
  } else if (stage === "QuarantineNamespaceDurability") {
    state.durable.quarantine.namespaceDurable = false;
  } else if (stage === "SourceTruncate" || stage === "SourceSync") {
    const length = variant === "SourceTruncated" ? TARGET_SOURCE_LENGTH : INITIAL_SOURCE_LENGTH;
    state.runtime.sourceVisibleLength = length;
    if (stage === "SourceSync") state.durable.sourceLength = length;
  } else if (stage === "Acknowledgement") {
    state.runtime.acknowledgementDelivered = variant === "AckPossiblyObserved";
  }
  markIndeterminate(state, stage, tx);
  state.recovery.residual = residualState(state);
  verifyState(state, `inject-${stage}-${variant}`);
  observedStates.add(JSON.stringify(state));
  return state;
}

function restartObservationForFault(stage, variant) {
  if (stage === "Prepare") {
    if (variant === "NoMutation") return { quarantineObserved: "Missing" };
    return { quarantineObserved: "Temp" };
  }
  if (stage === "QuarantinePublish") {
    return { quarantineObserved: variant === "FinalVisible" ? "Final" : "Temp" };
  }
  if (stage === "QuarantineNamespaceDurability") return { quarantineObserved: "Final" };
  if (stage === "SourceTruncate" || stage === "SourceSync") {
    return {
      sourceObserved: variant === "SourceTruncated" ? TARGET_SOURCE_LENGTH : INITIAL_SOURCE_LENGTH,
    };
  }
  return {};
}

function checkFaultMatrix() {
  const stages = [
    "Prepare",
    "QuarantinePublish",
    "QuarantineNamespaceDurability",
    "ManifestAcceptance",
    "SourceTruncate",
    "SourceSync",
    "CompletionPersistence",
    "Acknowledgement",
  ];
  let cases = 0;
  for (const form of FORMS) {
    for (const stage of stages) {
      for (const variant of faultVariants(form, stage)) {
        let state = faultBase(form, stage);
        const durableBeforeFault = JSON.stringify(state.durable);
        state = injectFault(state, stage, variant);
        const provenNoMutation = stage === "Prepare" && variant === "NoMutation";
        invariant(
          state.result.kind ===
            (provenNoMutation ? "IoFailedBeforeAcceptance" : "DurabilityIndeterminate"),
          provenNoMutation
            ? "pre-mutation-failure-is-io-before-acceptance"
            : "visible-failure-remains-uncertain",
          `${form}/${stage}/${variant} used the wrong failure boundary`,
          state,
        );
        if (provenNoMutation) {
          invariant(
            JSON.stringify(state.durable) === durableBeforeFault,
            "pre-mutation-failure-has-no-visible-state",
            `${form}/${stage}/${variant} mutated physical state`,
            state,
          );
        }
        const advancementBeforeRestart = state.counters.manifestAdvancements;
        const repairsBeforeRestart = state.counters.restartRepairs;
        state = transition(
          state,
          {
            type: "Restart",
            tx: TX_A,
            ...restartObservationForFault(stage, variant),
          },
          `${form}:${stage}:${variant}:restart`,
        );
        invariant(
          state.recovery.priorIndeterminateStage === (provenNoMutation ? null : stage),
          provenNoMutation
            ? "pre-mutation-failure-does-not-create-uncertainty"
            : "uncertainty-survives-restart",
          `${form}/${stage}/${variant} retained the wrong recovery boundary`,
          state,
        );
        const observedGeneration = head(state).generation;
        state = convergeExactRetry(state);
        invariant(
          ["Accepted", "AlreadyCompleted"].includes(state.result.kind),
          "fault-retry-converges",
          `${form}/${stage}/${variant} did not converge`,
          state,
        );
        invariant(
          state.counters.manifestAdvancements - advancementBeforeRestart <= 2 - observedGeneration,
          "fault-retry-no-duplicate-advancement",
          `${form}/${stage}/${variant} duplicated manifest advancement`,
          state,
        );
        formEvidence[form].faultCases += 1;
        formEvidence[form].residualStates.add(state.recovery.residual);
        formEvidence[form].restartRepairs +=
          state.counters.restartRepairs - repairsBeforeRestart;
        formEvidence[form].exactRetries += 1;
        caseCount += 1;
        cases += 1;
      }
    }
  }
  return cases;
}

function checkUnsupportedNamespaceAndLateDiscovery() {
  let cases = 0;
  for (const form of FORMS) {
    let state = initialState(form, { namespaceSupported: false });
    const before = JSON.stringify(state.durable);
    state = transition(state, { type: "Prepare", tx: TX_A }, `${form}:unsupported-preflight`);
    invariant(
      state.result.kind === "NamespaceDurabilityUnsupported",
      "unsupported-namespace-refuses-before-mutation",
      `${form}: unsupported namespace did not return typed refusal`,
      state,
    );
    invariant(
      JSON.stringify(state.durable) === before,
      "unsupported-namespace-has-no-mutation",
      `${form}: unsupported preflight mutated durable state`,
      state,
    );
    cases += 1;
    caseCount += 1;

    state = initialState(form);
    state = transition(state, { type: "Prepare", tx: TX_A });
    state = transition(state, { type: "PublishQuarantine" });
    state = transition(
      state,
      { type: "SyncQuarantineNamespace", result: "UnsupportedAfterMutation" },
      `${form}:late-unsupported`,
    );
    invariant(
      state.result.kind === "DurabilityIndeterminate",
      "late-unsupported-is-indeterminate",
      `${form}: late namespace discovery used a safe preflight refusal`,
      state,
    );
    invariant(
      head(state).phase === "Absent" && state.durable.sourceLength === INITIAL_SOURCE_LENGTH,
      "late-unsupported-never-accepts-or-truncates",
      `${form}: late unsupported path advanced logical state`,
      state,
    );
    cases += 1;
    caseCount += 1;
  }
  return cases;
}

function checkConcurrentWriters() {
  let cases = 0;
  for (const form of FORMS) {
    let state = initialState(form);
    state = transition(state, { type: "Prepare", tx: TX_A }, `${form}:writer-a-prepare`);
    const beforeContended = JSON.stringify(state.durable);
    state = transition(state, { type: "Prepare", tx: TX_B }, `${form}:writer-b-contended`);
    invariant(
      state.result.kind === "Contended" && JSON.stringify(state.durable) === beforeContended,
      "concurrent-writer-is-contended",
      `${form}: second cooperating writer mutated while lock held`,
      state,
    );

    state = transition(state, { type: "PublishQuarantine" });
    state = transition(state, { type: "SyncQuarantineNamespace" });
    state = transition(state, { type: "PersistPrepared" });
    state = transition(state, { type: "TruncateSource" });
    state = transition(state, { type: "SyncSource" });
    state = transition(state, { type: "PersistCompleted" });
    state = transition(state, { type: "ReadyToAcknowledge" });
    state = transition(state, { type: "Acknowledge" });
    const afterA = state.counters.manifestAdvancements;

    state = transition(state, { type: "Prepare", tx: TX_B }, `${form}:writer-b-stale-cas`);
    invariant(
      state.result.kind === "GenerationConflict" &&
        state.result.expectedGeneration === 0 &&
        state.result.actualGeneration === 2,
      "generation-conflict-is-explicit",
      `${form}: stale writer did not receive exact generation conflict`,
      state,
    );
    invariant(
      state.counters.manifestAdvancements === afterA,
      "generation-conflict-does-not-advance",
      `${form}: stale generation advanced manifest`,
      state,
    );

    const conflictingIdentity = { ...TX_A, fingerprint: "different-fingerprint", writer: "writer-c" };
    state = transition(
      state,
      { type: "Prepare", tx: conflictingIdentity },
      `${form}:idempotency-conflict`,
    );
    invariant(
      state.result.kind === "IdempotencyConflict" &&
        state.counters.manifestAdvancements === afterA,
      "same-id-different-fingerprint-refused",
      `${form}: conflicting exact-retry identity was accepted`,
      state,
    );

    state = transition(state, { type: "Prepare", tx: TX_A }, `${form}:exact-retry-completed`);
    invariant(
      state.result.kind === "AlreadyCompleted" &&
        state.counters.manifestAdvancements === afterA &&
        head(state).generation === 2,
      "completed-exact-retry-is-idempotent",
      `${form}: completed retry duplicated advancement`,
      state,
    );
    formEvidence[form].concurrencyCases += 4;
    formEvidence[form].exactRetries += 1;
    caseCount += 4;
    cases += 4;
  }
  return cases;
}

function checkCompletionGenerationRegression() {
  let cases = 0;
  for (const form of FORMS) {
    let state = initialState(form);
    state = transition(state, { type: "Prepare", tx: TX_A });
    state = transition(state, { type: "PublishQuarantine" });
    state = transition(state, { type: "SyncQuarantineNamespace" });
    state = transition(state, { type: "PersistPrepared" });
    state = transition(state, { type: "TruncateSource" });
    state = transition(state, { type: "SyncSource" });

    const exactPrepared = structuredClone(head(state));
    exactPrepared.generation = TX_A.expectedGeneration + 2;
    if (form === "AtomicSnapshotCas") {
      state.durable.snapshot.head = exactPrepared;
    } else {
      const prior = manifestRecord(1, "Prepared", {
        id: "tx-prior",
        fingerprint: "fp-prior",
      });
      state.durable.log.records = [prior, exactPrepared];
      if (form === "LogWithMaterializedView") {
        state.durable.view.head = structuredClone(exactPrepared);
        state.durable.view.status = "Exact";
        state.counters.materializedViewWrites = 2;
      }
    }
    state.counters.manifestAdvancements = 2;
    state.counters.authorityWrites = 2;

    const durableBeforeCompletion = JSON.stringify(state.durable);
    const countersBeforeCompletion = JSON.stringify(state.counters);
    state = transition(
      state,
      { type: "PersistCompleted" },
      `${form}:unexpected-prepared-generation`,
    );
    invariant(
      state.result.kind === "GenerationConflict" &&
        state.result.expectedGeneration === TX_A.expectedGeneration + 1 &&
        state.result.actualGeneration === TX_A.expectedGeneration + 2,
      "completion-requires-exact-prepared-generation",
      `${form}: completion accepted an unexpected same-transaction generation`,
      state,
    );
    invariant(
      JSON.stringify(state.durable) === durableBeforeCompletion &&
        JSON.stringify(state.counters) === countersBeforeCompletion,
      "completion-generation-conflict-does-not-mutate",
      `${form}: rejected completion changed physical state`,
      state,
    );
    caseCount += 1;
    cases += 1;
  }
  return cases;
}

function checkSuccessfulForms() {
  let cases = 0;
  for (const form of FORMS) {
    let state = runSuccessfulTransaction(form);
    invariant(
      state.result.kind === "Accepted" && head(state).generation === 2,
      "successful-form-accepts-once",
      `${form}: successful transaction did not reach generation 2`,
      state,
    );
    const advancements = state.counters.manifestAdvancements;
    state = transition(state, { type: "Prepare", tx: TX_A }, `${form}:successful-exact-retry`);
    invariant(
      state.result.kind === "AlreadyCompleted" &&
        state.counters.manifestAdvancements === advancements,
      "successful-exact-retry-does-not-advance",
      `${form}: exact successful retry appended/replaced again`,
      state,
    );
    formEvidence[form].exactRetries += 1;
    cases += 1;
    caseCount += 1;
  }
  return cases;
}

function physicalComparison() {
  const rows = [
    {
      form: "AtomicSnapshotCas",
      canonicalAuthorities: 1,
      eventStream: false,
      manifestAuthorityWrites: 2,
      auxiliaryViewWrites: 0,
      tornTailMode: false,
      replayRequired: false,
      namespaceReplacements: 2,
      restartSkewDomains: 1,
      adr0037Backflow: false,
    },
    {
      form: "AppendOnlyLog",
      canonicalAuthorities: 1,
      eventStream: true,
      manifestAuthorityWrites: 2,
      auxiliaryViewWrites: 0,
      tornTailMode: true,
      replayRequired: true,
      namespaceReplacements: 0,
      restartSkewDomains: 2,
      adr0037Backflow: true,
    },
    {
      form: "LogWithMaterializedView",
      canonicalAuthorities: 1,
      eventStream: true,
      manifestAuthorityWrites: 2,
      auxiliaryViewWrites: 2,
      tornTailMode: true,
      replayRequired: true,
      namespaceReplacements: 0,
      restartSkewDomains: 3,
      adr0037Backflow: true,
    },
  ];
  for (const row of rows) {
    row.safetyCases =
      formEvidence[row.form].crashCases +
      formEvidence[row.form].faultCases +
      formEvidence[row.form].concurrencyCases;
    row.complexityScore =
      row.canonicalAuthorities +
      row.manifestAuthorityWrites +
      row.auxiliaryViewWrites +
      row.namespaceReplacements +
      row.restartSkewDomains +
      Number(row.tornTailMode) +
      Number(row.replayRequired);
  }
  const selected = [...rows].sort(
    (left, right) => left.complexityScore - right.complexityScore,
  )[0];
  invariant(
    selected.form === "AtomicSnapshotCas",
    "model-selects-simplest-crash-correct-form",
    `unexpected selection: ${selected.form}`,
  );
  invariant(
    rows.every((row) => row.safetyCases > 0),
    "all-forms-compared-fairly",
    "one or more physical forms skipped the common matrix",
  );
  invariant(
    !selected.eventStream && !selected.adr0037Backflow,
    "selection-does-not-add-fifth-stream",
    "selected manifest form requires ADR-0037 backflow",
  );
  return { rows, selected };
}

function compactState(state) {
  return {
    form: state.form,
    capability: state.capability,
    durable: {
      sourceLength: state.durable.sourceLength,
      quarantine: state.durable.quarantine,
      manifestHead: head(state),
      snapshotReplacementTemp: state.durable.snapshot.replacementTemp,
      logTornTail: state.durable.log.tornTail,
      view: state.form === "LogWithMaterializedView" ? state.durable.view : "Unused",
    },
    runtime: state.runtime,
    result: state.result,
    recovery: state.recovery,
    counters: state.counters,
    diagnostics: state.diagnostics,
  };
}

function traceStep(label, state) {
  console.log(`${label}: ${JSON.stringify(compactState(state))}`);
}

function showRepresentativeTrace() {
  console.log("\nRepresentative full-state trace (selected snapshot form, crash after truncate):");
  let state = initialState("AtomicSnapshotCas");
  traceStep("0 initial", state);
  state = transition(state, { type: "Prepare", tx: TX_A });
  traceStep("1 prepare exact quarantine temp", state);
  state = transition(state, { type: "PublishQuarantine" });
  traceStep("2 quarantine publish visible", state);
  state = transition(state, { type: "SyncQuarantineNamespace" });
  traceStep("3 quarantine namespace durable", state);
  state = transition(state, { type: "PersistPrepared" });
  traceStep("4 manifest Prepared accepted by generation CAS", state);
  state = transition(state, { type: "TruncateSource" });
  traceStep("5 source truncate visible but not synchronized", state);
  state = transition(state, {
    type: "Restart",
    tx: TX_A,
    sourceObserved: TARGET_SOURCE_LENGTH,
  });
  traceStep("6 restart classifies exact PreparedSourceTruncated residual", state);
  state = convergeExactRetry(state);
  traceStep("7 exact retry persists completion then acknowledges", state);
  state = transition(state, { type: "Prepare", tx: TX_A });
  traceStep("8 repeated exact retry returns AlreadyCompleted without advancement", state);
}

console.log("THROWAWAY PROTOTYPE — no product imports, files, network, or persistence");
console.log(
  "Question: which persisted manifest form is simplest and crash-correct without silently adding a fifth canonical stream?",
);
console.log(
  "Assumptions: one stable cooperative lock; qualified namespace barriers; exact transaction fingerprints; metadata-only diagnostics.",
);

showRepresentativeTrace();

const counts = {
  completionGenerationRegressionCases: checkCompletionGenerationRegression(),
  successfulFormCases: checkSuccessfulForms(),
  crashCutCases: checkCrashMatrix(),
  visibleFailureCases: checkFaultMatrix(),
  namespaceRefusalCases: checkUnsupportedNamespaceAndLateDiscovery(),
  concurrentWriterCases: checkConcurrentWriters(),
};
const comparison = physicalComparison();

assert.equal(comparison.selected.form, "AtomicSnapshotCas");

console.log("\nExhaustive finite-model result:");
console.log(JSON.stringify({ forms: FORMS.length, crashCuts: CRASH_CUTS.length, ...counts }, null, 2));
console.log(`cases explored: ${caseCount}`);
console.log(`transitions evaluated: ${transitionCount}`);
console.log(`unique full states observed: ${observedStates.size}`);
console.log(`invariant assertions: ${assertionCount}`);
console.log(`invariant families passed (${invariantFamilies.size}):`);
for (const family of [...invariantFamilies].sort()) console.log(`- ${family}`);

console.log("\nPhysical-form comparison:");
for (const row of comparison.rows) console.log(JSON.stringify(row));
console.log(`\nSELECTED: ${comparison.selected.form}`);
console.log(
  "VERDICT: use one versioned atomic manifest snapshot under the stable coordination lock; compare the expected generation before each atomic replacement, synchronize the replacement and qualified parent namespace, and retain typed Prepared/Completed/residual state in the snapshot.",
);
console.log(
  "ADR-0037: no backflow is triggered because the selected manifest is state, not a new append-only canonical event stream. Selecting either log candidate would require STOP plus a registry ADR update.",
);
console.log(
  "PASS: all forms passed the common safety matrix; snapshot CAS was simplest, exact retry never duplicated advancement, unsupported namespace never Accepted, and content-free diagnostics held.",
);
