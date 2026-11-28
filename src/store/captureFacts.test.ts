import { describe, expect, it } from "vitest";
import { ended, running } from "../test/captureBackend";
import type { CaptureFact, CaptureId } from "../types";
import {
  type CaptureState,
  compareCaptureFacts,
  reduceCaptureState,
} from "./captureFacts";

/**
 * The renderer merge, exhaustively: for one start and for two consecutive
 * starts, every order in which the messages the backend sends can arrive
 * (acknowledgement, refusal, Running and Idle events, Stop reply, and a stale
 * snapshot a slower thread emitted earlier) ends in the state the backend is
 * in. Orders are generated, not chosen; impossible ones (a reply before its
 * command) are included, which only makes the check stronger.
 */

interface Scenario {
  name: string;
  /** The user's intents after issuing every command, in issue order. */
  startIntent: CaptureId;
  stopIntent: CaptureId | null;
  /** Each message is the facts it carries (a reply: its fact and snapshot). */
  messages: Record<string, CaptureFact[]>;
  /** Whether the backend ends with a running capture. */
  capturing: boolean;
}

const floor: CaptureId = { epoch: 1, seq: 0 };
const s1: CaptureId = { epoch: 1, seq: 1 };
const s2: CaptureId = { epoch: 1, seq: 2 };

const scenarios: Scenario[] = [
  {
    name: "one start that commits and keeps running",
    startIntent: s1,
    stopIntent: null,
    messages: {
      ack: [running(s1), running(s1)],
      runningEvent: [running(s1)],
      staleSnapshot: [ended(floor)],
    },
    capturing: true,
  },
  {
    name: "one start that commits, then loses its last source",
    startIntent: s1,
    stopIntent: null,
    messages: {
      ack: [running(s1), running(s1)],
      runningEvent: [running(s1)],
      idleEvent: [ended(s1)],
      staleSnapshot: [running(s1)],
    },
    capturing: false,
  },
  {
    name: "one start the backend refuses",
    startIntent: s1,
    stopIntent: null,
    messages: {
      refusal: [ended(s1), ended(floor)],
      staleSnapshot: [ended(floor)],
    },
    capturing: false,
  },
  {
    name: "one start that commits, then the user stops it",
    startIntent: s1,
    stopIntent: s1,
    messages: {
      ack: [running(s1), running(s1)],
      runningEvent: [running(s1)],
      idleEvent: [ended(s1)],
      stopReply: [ended(s1), ended(s1)],
      staleSnapshot: [running(s1)],
    },
    capturing: false,
  },
  {
    name: "a Stop that reaches the backend before its start",
    startIntent: s1,
    stopIntent: s1,
    messages: {
      stopReply: [ended(s1), ended(floor)],
      refusal: [ended(s1), ended(floor)],
      staleSnapshot: [ended(floor)],
    },
    capturing: false,
  },
  {
    name: "a stopped capture, then a replacement that keeps running",
    startIntent: s2,
    stopIntent: null,
    messages: {
      ack1: [running(s1), running(s1)],
      runningEvent1: [running(s1)],
      idleEvent1: [ended(s1)],
      stopReply1: [ended(s1), ended(s1)],
      ack2: [running(s2), running(s2)],
      runningEvent2: [running(s2)],
      staleSnapshot: [running(s1)],
    },
    capturing: true,
  },
  {
    name: "a stale Stop for the first capture answered while the replacement runs",
    startIntent: s2,
    stopIntent: null,
    messages: {
      ack1: [running(s1), running(s1)],
      idleEvent1: [ended(s1)],
      stopReply1: [ended(s1), running(s2)],
      ack2: [running(s2), running(s2)],
      runningEvent2: [running(s2)],
      staleSnapshot1: [running(s1)],
      staleSnapshot2: [ended(floor)],
    },
    capturing: true,
  },
  {
    name: "a stopped capture, then a replacement the backend refuses",
    startIntent: s2,
    stopIntent: null,
    messages: {
      ack1: [running(s1), running(s1)],
      runningEvent1: [running(s1)],
      idleEvent1: [ended(s1)],
      stopReply1: [ended(s1), ended(s1)],
      refusal2: [ended(s2), ended(s1)],
      staleSnapshot: [running(s1)],
    },
    capturing: false,
  },
  {
    name: "a refused start, then one that keeps running",
    startIntent: s2,
    stopIntent: null,
    messages: {
      refusal1: [ended(s1), ended(floor)],
      ack2: [running(s2), running(s2)],
      runningEvent2: [running(s2)],
      staleSnapshot: [ended(floor)],
    },
    capturing: true,
  },
  {
    name: "a capture lost on its own, then one the user stops",
    startIntent: s2,
    stopIntent: s2,
    messages: {
      ack1: [running(s1), running(s1)],
      runningEvent1: [running(s1)],
      idleEvent1: [ended(s1)],
      ack2: [running(s2), running(s2)],
      runningEvent2: [running(s2)],
      idleEvent2: [ended(s2)],
      stopReply2: [ended(s2), ended(s2)],
    },
    capturing: false,
  },
];

/** Every ordering of `items`, by Heap's algorithm. */
function* permutations<T>(items: readonly T[]): Generator<T[]> {
  const order = [...items];
  const counters = new Array(order.length).fill(0);
  yield [...order];
  let i = 1;
  while (i < order.length) {
    if (counters[i] < i) {
      const j = i % 2 === 0 ? 0 : counters[i];
      [order[i], order[j]] = [order[j], order[i]];
      yield [...order];
      counters[i] += 1;
      i = 1;
    } else {
      counters[i] = 0;
      i += 1;
    }
  }
}

const idle: CaptureState = {
  captureFact: null,
  captureStartIntent: null,
  captureStopIntent: null,
  isCapturing: false,
  captureStartTime: null,
};

describe("capture fact merge", () => {
  for (const scenario of scenarios) {
    it(`agrees with the backend in every arrival order: ${scenario.name}`, () => {
      const names = Object.keys(scenario.messages);
      const expectedFact = Object.values(scenario.messages)
        .flat()
        .reduce((newest, fact) =>
          compareCaptureFacts(fact, newest) > 0 ? fact : newest,
        );
      let orders = 0;
      for (const order of permutations(names)) {
        orders += 1;
        let state = reduceCaptureState(
          idle,
          {
            startIntent: scenario.startIntent,
            stopIntent: scenario.stopIntent,
          },
          0,
        );
        for (const [step, name] of order.entries()) {
          const before = state.captureFact;
          state = reduceCaptureState(
            state,
            { facts: scenario.messages[name] },
            step,
          );
          // The merged fact never moves backwards.
          if (before !== null && state.captureFact !== null) {
            expect(
              compareCaptureFacts(state.captureFact, before),
            ).toBeGreaterThanOrEqual(0);
          }
        }
        const label = `${scenario.name}: ${order.join(" → ")}`;
        expect(state.captureFact, label).toEqual(expectedFact);
        expect(state.isCapturing, label).toBe(scenario.capturing);
        expect(state.captureStarting, label).toBe(false);
        expect(state.captureStopping, label).toBe(false);
        expect(state.captureStartTime !== null, label).toBe(scenario.capturing);
      }
      // Every ordering was checked.
      let factorial = 1;
      for (let n = 2; n <= names.length; n += 1) factorial *= n;
      expect(orders).toBe(factorial);
    });
  }
});
