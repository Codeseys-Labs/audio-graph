import { beforeEach, describe, expect, it } from "vitest";
import { useAudioGraphStore } from "./index";

/**
 * Slice-level unit tests for `answerDrafts.ts` (audio-graph-83cc T4) —
 * mirrors `shellNav.test.ts`'s convention of exercising the slice through
 * the real, wired-up `useAudioGraphStore` rather than constructing the
 * slice creator in isolation (this repo has no multi-store slice harness).
 */

describe("store: answerDrafts slice", () => {
  beforeEach(() => {
    useAudioGraphStore.setState({ answerDrafts: {}, composerError: null });
  });

  it("starts with an empty draft map and no composer error", () => {
    const s = useAudioGraphStore.getState();
    expect(s.answerDrafts).toEqual({});
    expect(s.composerError).toBeNull();
  });

  it("setAnswerDraft writes a draft keyed by card id without disturbing other cards' drafts", () => {
    const { setAnswerDraft } = useAudioGraphStore.getState();
    setAnswerDraft("card-a", {
      status: "streaming",
      text: "",
      requestId: "r1",
    });
    setAnswerDraft("card-b", {
      status: "failed",
      text: "boom",
      requestId: null,
    });

    const drafts = useAudioGraphStore.getState().answerDrafts;
    expect(drafts["card-a"]).toEqual({
      status: "streaming",
      text: "",
      requestId: "r1",
    });
    expect(drafts["card-b"]).toEqual({
      status: "failed",
      text: "boom",
      requestId: null,
    });
  });

  it("appendAnswerDraftDelta appends onto the existing draft's text", () => {
    const { setAnswerDraft, appendAnswerDraftDelta } =
      useAudioGraphStore.getState();
    setAnswerDraft("card-a", {
      status: "streaming",
      text: "Hel",
      requestId: "r1",
    });
    appendAnswerDraftDelta("card-a", "r1", "lo");
    appendAnswerDraftDelta("card-a", "r1", " world");

    expect(useAudioGraphStore.getState().answerDrafts["card-a"]?.text).toBe(
      "Hello world",
    );
  });

  it("appendAnswerDraftDelta is a no-op when the requestId does not match the draft's currently-armed id (stale delta guard)", () => {
    const { setAnswerDraft, appendAnswerDraftDelta } =
      useAudioGraphStore.getState();
    setAnswerDraft("card-a", {
      status: "streaming",
      text: "keep",
      requestId: "r1",
    });
    appendAnswerDraftDelta("card-a", "stale-r0", "should-not-appear");

    expect(useAudioGraphStore.getState().answerDrafts["card-a"]?.text).toBe(
      "keep",
    );
  });

  it("appendAnswerDraftDelta is a no-op when no draft exists yet for that card id", () => {
    const { appendAnswerDraftDelta } = useAudioGraphStore.getState();
    appendAnswerDraftDelta("never-set", "r1", "text");
    expect(
      useAudioGraphStore.getState().answerDrafts["never-set"],
    ).toBeUndefined();
  });

  it("clearAnswerDraft removes only the named card's draft", () => {
    const { setAnswerDraft, clearAnswerDraft } = useAudioGraphStore.getState();
    setAnswerDraft("card-a", {
      status: "streaming",
      text: "",
      requestId: "r1",
    });
    setAnswerDraft("card-b", {
      status: "streaming",
      text: "",
      requestId: "r2",
    });
    clearAnswerDraft("card-a");

    const drafts = useAudioGraphStore.getState().answerDrafts;
    expect(drafts["card-a"]).toBeUndefined();
    expect(drafts["card-b"]).toBeDefined();
  });

  it("clearAnswerDraft on an id with no draft is a harmless no-op", () => {
    const { clearAnswerDraft } = useAudioGraphStore.getState();
    expect(() => clearAnswerDraft("nothing-here")).not.toThrow();
    expect(useAudioGraphStore.getState().answerDrafts).toEqual({});
  });

  it("setComposerError sets and clears the single composer-level error slot", () => {
    const { setComposerError } = useAudioGraphStore.getState();
    setComposerError("dispatch failed");
    expect(useAudioGraphStore.getState().composerError).toBe("dispatch failed");
    setComposerError(null);
    expect(useAudioGraphStore.getState().composerError).toBeNull();
  });
});
