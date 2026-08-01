# Product naming decision: AudioGraph and Aria

Date: 2026-07-31

Seed: `audio-graph-16fc`

Status: recommended

## Recommendation

Keep **AudioGraph** as the product, repository, engine, and stable technical identity for this release cycle. Do not rename the whole product to **A.R.I.A. — Adaptive Realtime Intelligent Assistant**.

If a warmer conversational identity is useful, use **Aria** only as the optional assistant persona or realtime-assist surface inside AudioGraph, such as “Ask Aria” or “Aria Live.” Treat that as display copy; preserve the bundle id, package names, filesystem roots, keychain service, and credential account namespace.

## Why

1. **The name is already crowded.** Opera ships an AI assistant named Aria. WAI-ARIA is a foundational web-accessibility standard. Current products also use ARIA for meeting and personal assistants, and independent projects already expand ARIA to phrases such as “Adaptive Reasoning Intelligence Assistant” and “Adaptive Real-time Intelligence Architecture.” A full rename would inherit substantial search, support, and brand ambiguity.

2. **“Realtime” misstates the primary product.** AudioGraph's MVP is durable local-first memory: capture, transcript, notes, temporal graph, Review, export, and deletion. Realtime speech-to-speech is a sibling mode, not the whole product. Naming the product around realtime assistance makes the secondary personality sound authoritative and hides the differentiated durable graph.

3. **AudioGraph describes the defensible core.** It is less personable, but it names the capture-to-temporal-graph architecture and does not promise autonomy the product does not yet provide. Product copy can make the benefit clearer without replacing the identity: “AudioGraph — private, durable memory for everything you hear.”

4. **A display alias is technically cheap; an identity rename is not.** Existing installations use `com.rsac.audiograph`, `audio-graph` filesystem roots, keychain service `audio-graph`, package/crate/binary names, and `provider:<key>` credential accounts. Renaming those during the credential-store migration would create a second migration axis and could orphan secrets. Stable technical identity must be explicitly decoupled from display branding.

## Staged naming surface

| Surface | Decision now | Later change rule |
| --- | --- | --- |
| Product/repository/engine | AudioGraph | Revisit only with distinct-name and legal clearance work |
| Assistant persona | Optional “Aria” | Display-only experiment; do not expand to A.R.I.A. in technical contracts |
| Realtime mode | “Aria Live” is possible | Must remain visibly a sibling to durable capture/review |
| Tauri product name/window copy | AudioGraph for now | May change independently after UX testing |
| Bundle id | `com.rsac.audiograph` | Preserve across display renames |
| Keychain service/account namespace | Stable AudioGraph namespace | Change only through dual-read, verified migration and rollback |
| Filesystem/config roots | Stable AudioGraph paths | Change only through versioned migration and packaged three-OS proof |
| Crate/package/binary/repository | AudioGraph | Separate implementation Seed if ever approved |

## Evidence checked

- W3C WAI-ARIA overview: https://www.w3.org/WAI/standards-guidelines/aria/
- Opera Aria product page: https://www.opera.com/features/aria
- ARIA, Adaptive Reasoning Intelligence Assistant: https://www.ariaproject.ai/
- ARIA Meeting Assistant: https://www.askaria.io/
- ARIA, Adaptive Real-time Intelligence Architecture: https://www.ariaos.app/

This is a product recommendation, not trademark clearance. Any future public rename needs a formal trademark, domain, package, and app-store search in the target markets.

## Follow-up trigger

Create a separate rename implementation Seed only if the user chooses a public display rename after reviewing this recommendation. Never fold technical-identity changes into the credential-service rebuild.
