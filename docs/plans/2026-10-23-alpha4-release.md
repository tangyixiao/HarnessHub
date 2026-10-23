# v0.1.0-alpha.4 Release Plan

- Date: 2026-10-23
- Target: `v0.1.0-alpha.4` Windows x64 unsigned NSIS pre-release.
- Release notes draft: [`docs/releases/v0.1.0-alpha.4.md`](../releases/v0.1.0-alpha.4.md)
- Feature baseline: Project Registry, Activity Timeline, multi-Harness capability boundaries, and plugin host v0.1.
- Current code baseline: `feat/plugins-host` / PR #13 plus the plugin-host hardening work. The hardening and PR integration are still in progress; no installer has been built and no GitHub Release exists yet.
- Scope: documentation and release procedure only. Version synchronization and release publication remain future steps in the release workflow.

## Release contents

1. Project registration resolves Git roots and deduplicates repeated registrations.
2. Activity Timeline presents usage and managed-session activity.
3. Harness support reflects the implemented adapter capabilities and known machine readiness limits. CodeBuddy remains detection-only; Gemini CLI and OpenCode GUI/auth limitations remain explicit.
4. Plugin host v0.1 offers backend IPC for plugin discovery/refresh, usage import, and skill retrieval; it has no UI and its permission decisions are observe-only.
5. Plugin hardening targets a real 64 KiB stderr ceiling, usage plugin directory/manifest identity validation before spawn, and degraded status after plugin call failures. Confirm these fixes are merged into the release candidate before describing them as shipped.

## Version synchronization

After the release PR and code candidate are selected, update and review all version sources together:

- `package.json`
- `src-tauri/Cargo.toml`
- `src-tauri/Cargo.lock` (regenerate through Cargo; do not hand-edit)
- `src-tauri/tauri.conf.json`

Set each project version to `0.1.0-alpha.4`, regenerate the lockfile using the repository's supported Cargo command, and inspect the diff to ensure the intended package versions are synchronized. Update this plan and the release notes with the actual PR/commit and command output. Do not tag until the version-sync change is included in the release candidate.

## Required checks

Run on the release candidate and paste the real outputs and exit codes into the verification tables in this plan and the release notes. Any failing or unavailable gate means the release candidate is not fully verified; record the exact failure and its environment limitation.

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

### Gate results — pending

| Gate                  | Result / actual output |
| --------------------- | ---------------------- |
| `pnpm format:check`   | 待执行                 |
| `pnpm rust:fmt:check` | 待执行                 |
| `pnpm lint`           | 待执行                 |
| `pnpm typecheck`      | 待执行                 |
| `pnpm test`           | 待执行                 |
| `pnpm rust:test`      | 待执行                 |
| `pnpm python:test`    | 待执行                 |
| `pnpm build`          | 待执行                 |
| `pnpm rust:clippy`    | 待执行                 |

## Windows packaging and checksum

1. Build the Windows x64 NSIS installer from the verified release candidate using the repository's Tauri release workflow/configuration.
2. Confirm the produced installer's architecture and NSIS format from build output/artifact metadata. Record the exact filename and size.
3. Run the built application executable from the release build (for example, `src-tauri/target/release/harness-hub.exe`) under a temporary Windows test profile with a fresh, isolated Roaming AppData directory. Confirm the app starts, opens/initializes its SQLite database from a cold start, WebView2 is available, and the reported version is `0.1.0-alpha.4`. Record the exact executable, test profile/isolation method, database path, and output. This smoke check does not require installing the NSIS package, so it cannot overwrite an existing installation or its registry entries.
4. Generate `SHA256SUMS` for the exact installer artifact, then independently recompute the hash and confirm it matches the file contents.
5. Attach both files to the GitHub Release. Do not add placeholder files or publish a checksum that was not verified against the final artifact.

### Packaging results — pending

| Item                                                            | Result / evidence |
| --------------------------------------------------------------- | ----------------- |
| Build command and actual output                                 | 待执行            |
| Installer filename, architecture, and size                      | 待构建            |
| Isolated app-data executable launch / DB cold-start smoke check | 待执行            |
| SHA-256 generation and independent verification                 | 待执行            |
| Final artifact links                                            | 待上传            |

## PR, CI, and publication

1. Complete the hardening changes and integrate them with plugin-host PR #13 according to the main release workstream. Review the final diff and confirm CI is green for the exact release candidate commit.
2. Synchronize versions and update this plan/release notes in the scoped release PR. Confirm the PR is merged before creating the tag.
3. Create tag `v0.1.0-alpha.4` from the verified release commit, push the tag, and confirm its target SHA.
4. Create a GitHub Release with the exact title `Harness Hub v0.1.0-alpha.4`, mark it explicitly as a **pre-release**, include the completed release notes, and attach the verified NSIS installer plus `SHA256SUMS`.
5. Re-open the published Release and verify pre-release status, tag target, two artifact attachments, and visible checksum. Record the Release URL below.

Publication is an explicit final action after the preceding evidence is complete. Do not describe a draft, local build, or tag as a published Release.

## Final release record — pending

| Evidence                                | Value        |
| --------------------------------------- | ------------ |
| Release PR                              | 待填写       |
| Verified release commit                 | 待填写       |
| Tag target SHA                          | 待填写       |
| GitHub Release URL / pre-release status | 待发布后填写 |
| Installer artifact URL                  | 待发布后填写 |
| `SHA256SUMS` artifact URL               | 待发布后填写 |
