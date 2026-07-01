/**
 * Frontend Sentry scrubber (`scrubEvent`) — privacy chokepoint.
 *
 * Mirrors the backend `scrub_event_strips_secret_transcript_and_identity`
 * test: an event carrying a free-text message, a user, a breadcrumb, a
 * non-allowlisted tag, a bad-shape allowlisted tag value, and one good
 * allowlisted structured tag must come out the other side with ONLY the good
 * allowlisted structured field surviving — no PII, no free prose.
 */

import type { Event } from "@sentry/browser";
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
});
