# v0.1.0-alpha.4 Release Plan

- Date: 2026-10-23
- Target: `v0.1.0-alpha.4` Windows x64 unsigned NSIS pre-release.
- Release notes draft: [`docs/releases/v0.1.0-alpha.4.md`](../releases/v0.1.0-alpha.4.md)
- Feature baseline: Project Registry, Activity Timeline, multi-Harness capability boundaries, and plugin host v0.1.
- Current candidate: alpha.4 versions are synchronized; runtime/source candidate is `2c78952` (later changes are docs/tests only). Integration order: PR #14/#15/#17 into the plugin-host branch, PR #13 into main, then release PR #16. PR #17 passed Linux CI in run 39955830641 attempt 2. The Windows x64 NSIS build, checksum, all local gates, and isolated database cold-start smoke are complete. Final integration and publication follow the checklist below; verify their resulting SHAs through GitHub metadata.
- Smoke check: saved no-bundle `harness-hub-smoke.exe` variant used an isolated identifier/hidden-window Tauri config override against the release code and frontend. It exited 0 after confirming a new SQLite database; the original production executable was restored and its hash matched the saved copy. This is not installer installation or interactive GUI acceptance.

## Release contents

1. Project registration resolves Git roots and deduplicates repeated registrations.
2. Activity Timeline presents usage and managed-session activity.
3. Harness support reflects the implemented adapter capabilities and known machine readiness limits. CodeBuddy remains detection-only; Gemini CLI and OpenCode GUI/auth limitations remain explicit.
4. Plugin host v0.1 offers backend IPC for plugin discovery/refresh, usage import, and skill retrieval; it has no UI and its permission decisions are observe-only.
5. Plugin hardening includes a real 64 KiB stderr ceiling, usage plugin directory/manifest identity validation before spawn, and degraded status after plugin call failures. Local acceptance passed; the final merge commits are resolved through the linked PR metadata.

## Version synchronization

The release candidate has synchronized all version sources:

- `package.json`
- `src-tauri/Cargo.toml`
- `src-tauri/Cargo.lock` (regenerate through Cargo; do not hand-edit)
- `src-tauri/tauri.conf.json`

All three manifests report `0.1.0-alpha.4`. Cargo regenerated `Cargo.lock`; review confirmed the only lockfile change is the root `harness-hub` package version from `0.0.0` to `0.1.0-alpha.4`, with no dependency drift. The version sync is included in the release candidate. Record the final release commit after PR integration.

## Required checks

The gates below were run against the alpha.4 release candidate. Preserve the actual outputs and exit codes in this plan and the release notes. All local gates passed. PR/CI integration remains in progress; this record does not claim a new Linux CI pass.

```text
pnpm format:check
pnpm rust:fmt:check
pnpm lint
pnpm typecheck
pnpm test
pnpm rust:test
pnpm python:test
pnpm build
pnpm rust:clippy
```

The repository's `AGENTS.md` defines completion as passing all nine checks. Do not infer a pass from a prior commit, earlier E2E record, or remote CI run against a different SHA.

### Gate results — alpha.4 candidate

| Gate                  | Result / actual output                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                               |
| --------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `pnpm format:check`   | Passed — `All matched files use Prettier code style!`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                |
| `pnpm rust:fmt:check` | Passed — `cargo fmt --all -- --check`, exit 0, no diagnostics.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                       |
| `pnpm lint`           | Passed — 0 errors, 4 existing `react-refresh/only-export-components` warnings.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                       |
| `pnpm typecheck`      | Passed — `tsc --noEmit`, exit 0.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                     |
| `pnpm test`           | Passed — 9 files / 179 tests.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                        |
| `pnpm rust:test`      | Passed — exit 0; lib 438 passed; 28 test-result records including main and doc tests; 26 executable test binaries including lib; `real_plugins` 20 passed; all integration suites 0 failed. Full output: `test-results/2026-10-23-alpha4/rust-test-final.log`. Earlier runs exposed the OpenCode S2 second-Ctrl+C race and a Linux PR #13 input-closed assertion that was too narrow; both test-only fixes are documented in [`2026-10-23-lifecycle-test-races.md`](2026-10-23-lifecycle-test-races.md) and PR #17. Linux CI retry passed (PR #17, run 39955830641 attempt 2); its first attempt was cancelled while installing system dependencies. |
| `pnpm python:test`    | Passed — `Ran 12 tests ... OK`.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                      |
| `pnpm build`          | Passed — Vite transformed 1,909 modules; built in 2.37s.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                             |
| `pnpm rust:clippy`    | Passed — `Finished dev profile` in 15.35s; `-D warnings`.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                            |

## Windows packaging and checksum

1. Build the Windows x64 NSIS installer from the version-synchronized release candidate using the repository's Tauri release workflow/configuration.
2. Confirm the produced installer's architecture and NSIS format from build output/artifact metadata. Record the exact filename and size.
3. Run the built application executable with an isolated config override that changes the Tauri identifier to `dev.harnesshub.alpha4.smoke.20261023` and hides the window. This gives the smoke build a fresh app-data/database identity without relying on `APPDATA` (Windows Known Folder resolution does not honor that environment override). Confirm the no-bundle variant starts and initializes a new SQLite database from a cold start. Record the executable, override, database path, and actual output. This is a database cold-start smoke only, not installer installation or interactive GUI acceptance; it does not overwrite an existing app identity or registry entries.
4. Generate `SHA256SUMS` for the exact installer artifact, then independently recompute the hash and confirm it matches the file contents.
5. After the Rust suite and PR/CI requirements are resolved, attach both files to the GitHub Release. Do not publish a checksum that was not verified against the final artifact.

### Packaging and smoke results

| Item                                                            | Result / evidence                                                                                                                                                                                                                                                                                                                                                                                                                                                                            |
| --------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| First NSIS build attempt                                        | Release compilation finished in 5m11s; bundling failed because downloading the official NSIS archive timed out (`timeout: global`). No installer was produced by this attempt.                                                                                                                                                                                                                                                                                                               |
| Retry toolchain recovery                                        | Downloaded official NSIS 3.11 and Tauri utils 0.5.3; verified against official tauri-cli v2.11.5 source SHA-1 values before caching.                                                                                                                                                                                                                                                                                                                                                         |
| Successful release build                                        | `tauri build --bundles nsis --ci`; release compile finished in 1m21s; target x64; NSIS bundler produced the installer successfully. Full output: `test-results/2026-10-23-alpha4/tauri-build-retry.log`.                                                                                                                                                                                                                                                                                     |
| Installer filename, architecture, and size                      | Tauri bundle output: `Harness Hub_0.1.0-alpha.4_x64-setup.exe`; final GitHub asset name: `Harness-Hub_0.1.0-alpha.4_x64-setup.exe`; Windows x64 NSIS, unsigned, 3,644,933 bytes. Same installer bytes and SHA-256. Original output preserved at `test-results/2026-10-23-alpha4/artifacts/`.                                                                                                                                                                                                 |
| Isolated app-data executable launch / DB cold-start smoke check | Passed — saved `harness-hub-smoke.exe` no-bundle variant with identifier `dev.harnesshub.alpha4.smoke.20261023`; `SMOKE_ALIVE pid=51464`; fresh Roaming app-data DB reported `schemaVersion=9, harnesses=5, sessions=0, plugins=0`; `SMOKE_RESULT exit=0`. Production app data was untouched; formal executable restored and SHA matched saved copy. This is not installer installation or interactive GUI acceptance. Full output: `test-results/2026-10-23-alpha4/smoke-verification.log`. |
| SHA-256 generation and independent verification                 | Passed — `32a26c7419597fbaed3b1942b5fff6839ba72ae74f6c1b4cc9f293c7e398024c`; PowerShell and Python independently matched `artifacts/SHA256SUMS`. Final manifest entry uses the no-space GitHub asset name.                                                                                                                                                                                                                                                                                   |
| Final artifact links                                            | Expected after publication: [installer](https://github.com/tangyixiao/HarnessHub/releases/download/v0.1.0-alpha.4/Harness-Hub_0.1.0-alpha.4_x64-setup.exe), [SHA256SUMS](https://github.com/tangyixiao/HarnessHub/releases/download/v0.1.0-alpha.4/SHA256SUMS). Local files are preserved under `test-results/2026-10-23-alpha4/artifacts/`.                                                                                                                                                 |

## PR, CI, and publication

1. Confirm PR #17 CI passes, then close out PR #13 and the merged PR #14/#15 hardening changes on the feature branch. Confirm CI is green for the exact release candidate commit.
2. Confirm release PR #16 includes the version synchronization and final evidence. Merge only after all required checks are accepted.
3. Create tag `v0.1.0-alpha.4` from the verified release commit, push the tag, and confirm its target SHA.
4. Create a GitHub Release with the exact title `Harness Hub v0.1.0-alpha.4`, mark it explicitly as a **pre-release**, include the completed release notes, and attach the verified NSIS installer plus `SHA256SUMS`.
5. Re-open the published Release and verify pre-release status, tag target, two artifact attachments, and visible checksum. Record the Release URL below.

Publication follows PR #13 integration and release PR #16 merge, with successful required checks. Local Windows build, checksum, all nine gates, and isolated database smoke are complete. The linked release and asset URLs are expected targets; verify GitHub metadata confirms the release and assets before reporting publication.

## Release references

| Evidence                                | Value                                                                                                                                                      |
| --------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Release PR                              | PR #16                                                                                                                                                     |
| Runtime/source candidate                | `2c78952` (subsequent changes are docs/tests only)                                                                                                         |
| Tag target                              | `v0.1.0-alpha.4` (symbolic tag; verify target SHA against GitHub release metadata)                                                                         |
| GitHub Release URL / pre-release status | [Release URL](https://github.com/tangyixiao/HarnessHub/releases/tag/v0.1.0-alpha.4); verify `draft=false` and `prerelease=true` in the published metadata. |
| Installer artifact URL                  | [Expected target](https://github.com/tangyixiao/HarnessHub/releases/download/v0.1.0-alpha.4/Harness-Hub_0.1.0-alpha.4_x64-setup.exe).                      |
| `SHA256SUMS` artifact URL               | [Expected target](https://github.com/tangyixiao/HarnessHub/releases/download/v0.1.0-alpha.4/SHA256SUMS).                                                   |
