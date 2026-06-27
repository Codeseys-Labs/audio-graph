# ADR-0019: Credential And Config Storage Migration

## Status

proposed

## Context

AudioGraph already separates most secrets from non-secret settings:

- `config.yaml` is the canonical settings file.
- legacy `settings.json` is imported when a canonical config is absent.
- inline provider secrets are redacted before settings are written.
- `credentials.yaml` stores provider API keys behind Rust commands.
- Settings can load credential presence and provider readiness without sending
  stored plaintext keys to React.

That shape is directionally correct for the product vision: React configures
providers, while Rust owns long-lived provider connections, health checks, model
catalog calls, and secret use.

The current backend is still too file-specific to be the final credential
system. `credentials.yaml` is plaintext on disk, several docs and strings name it
as the primary source of truth, AWS credential refresh re-reads it directly, and
the YAML parser is `serde_yaml`, whose upstream repository is archived. Moving to
OS-native secret storage must preserve existing users, saved-key readiness, model
discovery, headless CI, and local development workflows.

Current external facts shaping this decision:

- `keyring` 4.1 exposes a v1 interface for setting, reading, and deleting
  secrets on macOS Keychain Services, Windows Credential Manager, and *nix Secret
  Service.
- Tauri Stronghold stores secrets through the IOTA Stronghold engine, but its
  Tauri plugin is vault/password oriented and exposes JavaScript bindings in the
  documented flow.
- `serde_yaml` was archived by its owner on 2024-03-25.
- `figment` can compose typed configuration from multiple providers and tracks
  provenance for errors.
- `serde-saphyr` offers Serde YAML parsing/serialization with configurable parser
  options and location-aware errors.
- `secrecy` can make secret access explicit, block accidental Debug leakage, and
  zeroize wrapped values on drop.

## Decision Drivers

- Provider credentials must not be written to `config.yaml`, logs, screenshots,
  Seeds, or frontend state.
- Settings must show saved credential presence and use saved keys for health and
  model catalog checks without asking users to re-enter the key.
- Windows, macOS, and Linux must have first-class behavior, not a Windows-only
  happy path.
- Headless CI and local developer environments must remain deterministic and
  must not require OS credential prompts.
- Existing `credentials.yaml` users need staged migration, recovery, and rollback
  without losing keys.
- The provider registry, backend allowlist, readiness probes, and frontend source
  labels must move together so provider additions do not create one-off secret
  paths.
- Parser replacement must preserve the current settings schema, defaults, legacy
  import behavior, corrupt-config fallback, and redaction guarantees.

## Considered Options

### Option A - Keep `credentials.yaml` As The Primary Store

Keep the current store and strengthen owner-only file writes, redaction tests,
and docs.

This is useful as a compatibility and development fallback, but it does not meet
the security expectation for a polished desktop app. It also leaves every future
provider wired to plaintext storage by default.

### Option B - Use OS Keychain Through A Backend Credential Facade

Introduce a Rust-owned credential facade, make OS keychain storage the default
backend, and keep YAML as an import/fallback backend.

The default production backend should use OS-native storage:

- macOS: Keychain Services.
- Windows: Windows Credential Manager.
- Linux desktop: Secret Service where available.
- Linux headless/dev/CI: explicit non-production fallback backend.

The facade owns read, write, delete, presence, source reporting, diagnosis,
migration, and test injection. Commands and provider clients use the facade
instead of reading `credentials.yaml` directly.

### Option C - Use Tauri Stronghold As The Primary Store

Adopt Stronghold as the default credential backend.

Stronghold is viable if the product later needs an app-managed encrypted vault,
sync/export semantics, or a user-facing vault password. It is not the best first
default because AudioGraph already keeps secret use in Rust commands and wants
native desktop credential stores with minimal new UX. Stronghold also adds a
vault-unlock concept that has to be designed carefully to avoid prompting users
before routine provider checks.

### Option D - Encrypt `credentials.yaml` With An App Key

Keep a YAML file but encrypt secret values with an application-managed key.

This only moves the problem to key management. Without OS credential storage or a
user-managed vault password, the encryption key must live somewhere equivalent
to the plaintext file. This option is rejected for the primary path.

### Option E - Replace YAML Parsing Opportunistically

Swap `serde_yaml` for a new parser in place.

This is too risky without fixtures. `config.yaml` has product semantics beyond
syntax: defaults, older field tolerance, corrupt-file fallback, redaction, and
legacy `settings.json` import. Parser migration must happen behind a codec
boundary with compatibility tests first.

## Decision Outcome

Adopt Option B as the target architecture, with a codec boundary for Option E.

AudioGraph will introduce:

- `CredentialBackend`: a Rust trait or equivalent facade for provider secret
  storage.
- `CredentialSource`: stable, non-secret source labels such as `os_keychain`,
  `file_fallback`, `imported_file`, `missing`, and `error`.
- `YamlCredentialBackend`: a compatibility backend around the current
  `credentials.yaml` logic.
- `KeychainCredentialBackend`: an OS-native backend using the `keyring`
  ecosystem or direct `keyring-core` stores where we need finer control.
- `CredentialMigration`: idempotent import from existing `credentials.yaml` into
  the keychain backend, with explicit fallback, recovery, migrated-key, and
  deleted-key states.
- `ConfigCodec`: a small settings parser/serializer boundary so `serde_yaml`
  can be replaced only after compatibility fixtures pass.

Production desktop builds should prefer the OS-native backend. CI, tests, and
developer overrides may opt into in-memory or YAML file fallback. The app must
not silently write plaintext credentials after a successful production migration
unless the user explicitly chooses an export/dev fallback action.

The first migration wave keeps `credentials.yaml` as a manual recovery/import
artifact, but writes non-secret migration state for keys that have been imported
to keychain or explicitly deleted. Automatic YAML fallback must filter those
tracked keys so legacy plaintext values cannot resurrect a deleted credential or
silently override a newer keychain value if the OS store becomes unavailable.

Stronghold remains a future backend candidate, not the default.

## Consequences

Positive:

- Secrets move to OS-native storage for normal desktop users.
- Settings can keep the current saved-key UX while exposing safer source labels.
- Provider additions get one credential contract rather than scattered storage
  branches.
- CI and local tests stay deterministic through injectable backends.
- Parser replacement can be tested without changing user-visible settings
  semantics.

Negative:

- Linux Secret Service availability varies across desktops and headless runners.
- macOS and Windows credential stores can have user/session-specific behavior
  that must be handled in tests and docs.
- AWS refresh, provider readiness, model catalogs, command tests, and frontend
  source labels all need coordinated updates.
- Migration must avoid losing or duplicating user credentials, so it cannot be a
  destructive one-shot rewrite.

## Implementation Outline

1. Add a backend-owned credential facade.
   - Define methods for `get`, `set`, `delete`, `presence`, `diagnose`, and
     `import_from_file`.
   - Keep the provider key allowlist in one backend-owned module.
   - Make returned presence non-secret and serializable over IPC.

2. Wrap the existing YAML store first.
   - Implement `YamlCredentialBackend` using the current parser and owner-only
     write behavior.
   - Move direct callers such as settings hydration, provider readiness,
     credential commands, and AWS refresh onto the facade before adding keychain
     storage.
   - Preserve existing tests as facade tests.

3. Add the OS keychain backend.
   - Use service names and account names that are stable across app upgrades,
     for example service `audio-graph` and account
     `provider:<credential-key>`.
   - Prefer `keyring` v1 only if its automatic store selection is enough; use
     `keyring-core` plus explicit stores if the app needs sharper Linux
     fallback control.
   - Classify store errors into user-actionable states without logging secret
     values.

4. Migrate existing `credentials.yaml` users.
   - Read keychain first.
   - If a key is missing and YAML contains it, import that key.
   - Record non-secret migration state for imported, keychain-owned, and
     explicitly deleted keys.
   - Keep the legacy YAML file intact in the first migration wave, but ignore
     tracked keys during automatic YAML fallback unless the user explicitly
     forces the file backend.
   - Reject YAML mutations when the existing file is malformed so recovery data
     is not overwritten by a partial store.
   - Offer a later explicit cleanup/export action after import has been
     verified.

5. Preserve saved-key health and model discovery.
   - `load_credential_presence_cmd`, `get_provider_readiness_cmd`,
     provider-specific model catalog commands, and `diagnose_credentials` must
     use the credential facade.
   - React receives only presence, source, readiness, timestamps, model catalogs,
     and sanitized error text.
   - Settings should auto-refresh readiness when opened and after credential
     save/delete operations.

6. Preserve headless CI and development.
   - Unit tests use in-memory or temp-file backends.
   - CI jobs that do not provide provider secrets must assert missing/unchecked
     states without prompting OS keychains.
   - Provider-backed smoke tests are opt-in via explicit CI secrets and env
     gates.

7. Add a config codec compatibility harness.
   - Capture fixtures for current `config.yaml`, older `settings.json`, redacted
     provider settings, corrupt YAML fallback, unknown/older fields, and
     defaulted values.
   - Compare current `serde_yaml` behavior with candidate codecs before
     replacing it.
   - Candidate parser paths include `serde-saphyr` for YAML compatibility and
     `figment` if provenance/layered providers become useful for app settings.

   The initial harness keeps runtime parsing on `serde_yaml` behind
   `ConfigCodec` and uses `serde-saphyr` as a dev-test candidate. The contract
   is semantic, not byte-for-byte YAML preservation:

   - current `config.yaml`, legacy `settings.json`, missing fields, and
     tolerated unknown fields must deserialize to the same redacted settings
     value under the current and candidate codecs.
   - corrupt YAML and unknown provider enum tags must fail under both codecs.
   - writeback is a known-schema rewrite; comments, ordering authored by hand,
     and unknown future fields are not preserved today.
   - parser failures must leave the recoverable file on disk rather than
     importing stale legacy settings or overwriting recovery data.
   - serialized settings must remain free of inline credential values and
     credential-bearing fields; secrets stay in the credential backend.

   `config-rs` is useful if app settings later need layered defaults,
   environment overrides, or richer provenance, but it is not a drop-in
   replacement for user settings writeback because it reads and merges sources
   rather than preserving and editing the original YAML document.

8. Update UX copy and docs.
   - Replace user-facing "credentials.yaml is the store" language with "saved
     credentials" or source-aware wording.
   - Keep `credentials.yaml` docs under import/export/dev fallback sections.
   - Add recovery copy for keychain unavailable, malformed import file, and
     fallback mode.

## Rollback

Rollback is the YAML backend plus non-destructive migration.

If the keychain backend fails on a platform or release channel, builds can switch
to the YAML fallback backend without losing already imported credentials. Since
the first migration wave does not delete `credentials.yaml`, users retain a
manual recovery path. If a parser replacement causes incompatibility, keep
`serde_yaml` behind `ConfigCodec` until the failing fixture is understood.

## Acceptance Criteria

- Windows Credential Manager, macOS Keychain, Linux Secret Service, and headless
  fallback behavior are covered by code and docs.
- No provider credential is persisted to `config.yaml`, logs, Seeds, screenshots,
  or frontend state.
- Saved-key health checks and model catalog loading work without key re-entry.
- AWS refresh no longer reads `credentials.yaml` directly.
- `credentials.yaml` import is idempotent and non-destructive.
- Frontend credential source labels no longer assume YAML is the only store.
- Parser migration fixtures prove current settings behavior before replacing
  `serde_yaml`.

## References

- `src-tauri/src/credentials/mod.rs`
- `src-tauri/src/settings/mod.rs`
- `src-tauri/src/commands.rs`
- `src-tauri/src/aws_util/mod.rs`
- `src/components/SettingsPage.tsx`
- `src/components/ProviderReadinessPanel.tsx`
- `src/components/ExpressSetup.tsx`
- `https://docs.rs/keyring/latest/keyring/`
- `https://docs.rs/keyring/latest/keyring/v1/index.html`
- `https://v2.tauri.app/plugin/stronghold/`
- `https://github.com/dtolnay/serde-yaml`
- `https://docs.rs/figment/latest/figment/`
- `https://docs.rs/serde-saphyr/latest/serde_saphyr/`
- `https://docs.rs/secrecy/latest/secrecy/`
