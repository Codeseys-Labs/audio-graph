/**
 * Frontend Sentry scrubber (`scrubEvent`) — privacy chokepoint.
 *
 * Mirrors the backend `scrub_event_strips_secret_transcript_and_identity`
 * test: an event carrying a free-text message, a user, a breadcrumb, a
 * non-allowlisted tag, a bad-shape allowlisted tag value, and one good
 * allowlisted structured tag must come out the other side with ONLY the good
 * allowlisted structured field surviving — no PII, no free prose.
 */

import type { Event, StackFrame } from "@sentry/browser";
import { describe, expect, it } from "vitest";
import { scrubEvent } from "./sentry";

describe("scrubEvent (frontend beforeSend)", () => {
  it("keeps only allowlisted structured tags and strips all PII / free text", () => {
    const SECRET = "sk-supersecret-transcript-and-key-1234567890";

    const event: Event = {
      message: `raw transcript containing ${SECRET}`,
      user: {
        id: "user-42",
        email: "person@example.com",
        ip_address: "1.2.3.4",
      },
      request: { url: "https://internal/api", headers: { cookie: SECRET } },
      breadcrumbs: [{ category: "console", message: `logged ${SECRET}` }],
      extra: { note: `free-form ${SECRET}` },
      exception: {
        values: [{ type: "TypeError", value: `boom ${SECRET}` }],
      },
      tags: {
        // Non-allowlisted key: must be dropped entirely.
        api_key: SECRET,
        // Allowlisted key but bad-shape value (free prose): must be dropped.
        component: "the user typed this whole sentence with spaces",
        // Allowlisted key + valid id: the ONLY thing that should survive.
        category: "frontend",
      },
    };

    const scrubbed = scrubEvent(event);

    // Event is never dropped, only its unsafe contents.
    expect(scrubbed).not.toBeNull();
    if (scrubbed === null) throw new Error("event was dropped");

    // Free text / PII fields are gone.
    expect(scrubbed.message).toBeUndefined();
    expect(scrubbed.user).toBeUndefined();
    expect(scrubbed.request).toBeUndefined();
    expect(scrubbed.breadcrumbs).toBeUndefined();
    expect(scrubbed.extra).toBeUndefined();

    // Exception type kept for triage; its free-prose value stripped.
    expect(scrubbed.exception?.values?.[0]?.type).toBe("TypeError");
    expect(scrubbed.exception?.values?.[0]?.value).toBeUndefined();

    // Only the good allowlisted structured tag survives.
    expect(scrubbed.tags).toEqual({ category: "frontend" });
    expect(scrubbed.tags).not.toHaveProperty("api_key");
    expect(scrubbed.tags).not.toHaveProperty("component");

    // The secret appears nowhere in the serialised survivor.
    expect(JSON.stringify(scrubbed)).not.toContain(SECRET);
  });

  it("drops an allowlisted tag whose value is a non-string", () => {
    const event: Event = {
      tags: {
        // Numbers/booleans are valid Sentry tag Primitives but not our id shape.
        category: 123 as unknown as string,
        surface: "invoke",
      },
    };

    const scrubbed = scrubEvent(event);
    expect(scrubbed?.tags).toEqual({ surface: "invoke" });
  });

  it("yields an empty tag map when no allowlisted tags are present", () => {
    const event: Event = { tags: { unrelated: "x" } };
    const scrubbed = scrubEvent(event);
    expect(scrubbed?.tags).toEqual({});
  });

  // Load-bearing privacy gate, mirroring the backend
  // `scrub_event_strips_secret_transcript_and_identity`: an event carrying a
  // fake secret + fake transcript planted into EVERY free-text / source /
  // identity-bearing field must come out the other side of `scrubEvent` with
  // the secret and transcript GONE everywhere, identity nulled, frames
  // basenamed with vars/source cleared, only safe contexts kept, and the good
  // allowlisted tags (INCLUDING `event.name`) surviving. This test must FAIL if
  // the scrubber regresses on any field.
  it("scrubs a fully-planted event: no free text, path, source, var, or identity survives; allowlisted tags (incl. event.name) survive", () => {
    const SECRET = "sk-test-supersecret-credential-1234567890";
    const TRANSCRIPT = "patient said their social security number aloud";
    const ABS_PATH = "/Users/alice/secret-project/src/x.ts";
    const THREAD_PATH = "/home/alice/audio/worker.ts";
    const TOP_PATH = "/home/alice/work/top.ts";

    const event: Event = {
      // Top-level free prose.
      message: `boom: token=${SECRET} transcript="${TRANSCRIPT}"`,
      // Identity / network metadata.
      server_name: "alices-macbook.local",
      user: {
        id: "user-42",
        email: "alice@example.com",
        ip_address: "203.0.113.7",
      },
      request: { url: "https://internal/api", headers: { cookie: SECRET } },
      // Structured log entry: interpolated message + positional params.
      logentry: {
        message: `logentry: ${SECRET} / ${TRANSCRIPT}`,
        params: [SECRET, TRANSCRIPT],
      },
      // Free-text identifiers.
      transaction: `txn ${SECRET} ${TRANSCRIPT}`,
      // A custom fingerprint encoding prose.
      fingerprint: [`${SECRET}-${TRANSCRIPT}`],
      // Breadcrumb + extra that could carry private data.
      breadcrumbs: [
        {
          category: "console",
          message: `user typed ${SECRET} / ${TRANSCRIPT}`,
        },
      ],
      extra: { transcript: TRANSCRIPT, api_key: SECRET },
      // Exception with a fully-populated private frame. captureException always
      // attaches a stacktrace; abs_path embeds a username, filename is a full
      // path, and context/source/vars carry the secret + transcript.
      exception: {
        values: [
          {
            type: "RuntimeError",
            value: `failed with ${SECRET}; heard: ${TRANSCRIPT}`,
            stacktrace: {
              frames: [
                {
                  abs_path: ABS_PATH,
                  filename: ABS_PATH,
                  context_line: `let x = "${SECRET}"; // ${TRANSCRIPT}`,
                  pre_context: [`// ${TRANSCRIPT}`],
                  post_context: [`// ${SECRET}`],
                  vars: { heard: TRANSCRIPT, key: SECRET },
                },
              ],
            },
          },
        ],
      },
      // Thread stacktrace frame the exception loop does NOT reach.
      threads: {
        values: [
          {
            stacktrace: {
              frames: [
                {
                  abs_path: THREAD_PATH,
                  filename: THREAD_PATH,
                  context_line: `emit(${SECRET}, ${TRANSCRIPT})`,
                  vars: { buf: TRANSCRIPT, token: SECRET },
                },
              ],
            },
          },
        ],
      },
      // Contexts: a safe one (kept) + a non-allowlisted one carrying prose
      // (dropped).
      contexts: {
        os: { name: "macOS", version: "14.4" },
        // Non-allowlisted context with planted prose — must be dropped whole.
        app: { app_name: `${SECRET}`, note: TRANSCRIPT },
      },
      tags: {
        // Non-allowlisted key: dropped entirely.
        api_key: SECRET,
        // Allowlisted key but bad-shape (free prose) value: dropped.
        component: `the user typed ${TRANSCRIPT}`,
        // Good allowlisted structured tags: these SURVIVE.
        "event.name": "asr.stream.error",
        category: "frontend",
        surface: "invoke",
      },
    };

    // Attach the deprecated top-level `stacktrace` via a widened cast: the
    // browser `Event` type omits this legacy field, but the wire format permits
    // it and the scrubber must still basename + clear its frames.
    (event as { stacktrace?: { frames: StackFrame[] } }).stacktrace = {
      frames: [
        {
          abs_path: TOP_PATH,
          filename: TOP_PATH,
          context_line: `top ${SECRET} ${TRANSCRIPT}`,
        },
      ],
    };

    const scrubbed = scrubEvent(event);
    expect(scrubbed).not.toBeNull();
    if (scrubbed === null) throw new Error("event was dropped");

    // Serialize the WHOLE event and assert nothing leaks anywhere.
    const json = JSON.stringify(scrubbed);
    expect(json).not.toContain(SECRET);
    expect(json).not.toContain(TRANSCRIPT);
    expect(json).not.toContain("alice@example.com");
    expect(json).not.toContain("203.0.113.7");
    expect(json).not.toContain("alices-macbook.local");
    // Absolute directory segments must be gone (only the basename may remain).
    expect(json).not.toContain("/Users/alice");
    expect(json).not.toContain("/home/alice");
    expect(json).not.toContain("secret-project");

    // Identity / free-prose fields nulled.
    expect(scrubbed.message).toBeUndefined();
    expect(scrubbed.user).toBeUndefined();
    expect(scrubbed.request).toBeUndefined();
    expect(scrubbed.breadcrumbs).toBeUndefined();
    expect(scrubbed.extra).toBeUndefined();
    expect(scrubbed.logentry).toBeUndefined();
    expect(scrubbed.transaction).toBeUndefined();
    expect(scrubbed.server_name).toBeUndefined();
    expect(scrubbed.fingerprint).toBeUndefined();

    // Exception type kept for triage; value stripped; frame basenamed + cleared.
    const excValue = scrubbed.exception?.values?.[0];
    expect(excValue?.type).toBe("RuntimeError");
    expect(excValue?.value).toBeUndefined();
    const excFrame = excValue?.stacktrace?.frames?.[0];
    expect(excFrame?.abs_path).toBe("x.ts");
    expect(excFrame?.filename).toBe("x.ts");
    expect(excFrame?.context_line).toBeUndefined();
    expect(excFrame?.pre_context).toBeUndefined();
    expect(excFrame?.post_context).toBeUndefined();
    expect(excFrame?.vars).toBeUndefined();

    // Top-level (deprecated) stacktrace frame basenamed + source cleared.
    const topFrame = (scrubbed as { stacktrace?: { frames?: StackFrame[] } })
      .stacktrace?.frames?.[0];
    expect(topFrame?.abs_path).toBe("top.ts");
    expect(topFrame?.filename).toBe("top.ts");
    expect(topFrame?.context_line).toBeUndefined();

    // Thread stacktrace frame basenamed + cleared.
    const threadFrame = scrubbed.threads?.values?.[0]?.stacktrace?.frames?.[0];
    expect(threadFrame?.abs_path).toBe("worker.ts");
    expect(threadFrame?.filename).toBe("worker.ts");
    expect(threadFrame?.context_line).toBeUndefined();
    expect(threadFrame?.vars).toBeUndefined();

    // Only the safe context survives; the prose-bearing one is gone.
    expect(scrubbed.contexts).toEqual({
      os: { name: "macOS", version: "14.4" },
    });
    expect(scrubbed.contexts).not.toHaveProperty("app");

    // Only the good allowlisted structured tags survive — INCLUDING event.name.
    expect(scrubbed.tags).toEqual({
      "event.name": "asr.stream.error",
      category: "frontend",
      surface: "invoke",
    });
    expect(scrubbed.tags?.["event.name"]).toBe("asr.stream.error");
    expect(scrubbed.tags).not.toHaveProperty("api_key");
    expect(scrubbed.tags).not.toHaveProperty("component");
  });

  it("drops event.name when its value fails the id-shape check", () => {
    const event: Event = {
      tags: {
        // Free prose with spaces — fails ^[a-z0-9._:-]{1,48}$.
        "event.name": "not a valid id with spaces",
        category: "frontend",
      },
    };
    const scrubbed = scrubEvent(event);
    expect(scrubbed?.tags).toEqual({ category: "frontend" });
    expect(scrubbed?.tags).not.toHaveProperty("event.name");
  });
});
