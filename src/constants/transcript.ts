/**
 * Shared transcript render-window size (audio-graph-3b3f).
 *
 * `LiveTranscript` mounts only the LAST `TRANSCRIPT_WINDOW_SIZE` segments for
 * performance (`segments.slice(-TRANSCRIPT_WINDOW_SIZE)`), and the After
 * `SeekTimeline` strip must cap its blocks to the SAME tail window so every
 * rendered block has a mounted seek target — a block outside the transcript's
 * window would click-seek to a segment that isn't in the DOM (a silent no-op).
 *
 * Both components consume this single constant so the two windows can never
 * drift apart again (PR #80 review: the strip originally rendered the FIRST
 * 200 while the transcript mounted the LAST 200).
 */
export const TRANSCRIPT_WINDOW_SIZE = 200;
