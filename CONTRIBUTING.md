# Contributing

Thanks for your interest in phi-agent. Code contributions, architecture discussions, documentation fixes, and bug reports are all welcome.

## Before you start

- Read the [README](README.md) or the [Chinese guide](README.zh-CN.md), plus the [design documents](docs/README.md). `docs/` is the single source of truth for design.
- Open an issue before starting a large feature or architectural change so the design can be aligned first.
- Do not report security issues publicly. Follow [SECURITY.md](SECURITY.md).

## Development setup

- Rust 1.85+ (MSVC toolchain)
- Bun (or Node) and the Tauri 2 platform dependencies
- Windows: Visual Studio Build Tools with the “Desktop development with C++” workload

```bash
bun install
bun run tauri dev
```

## Checks before submitting

```bash
bunx tsc --noEmit
bun run build
cd src-tauri && cargo test
```

Please make sure that:

- New behavior has tests; changes touching policy, sandboxing, budgets, or recovery especially need them.
- Frontend type checking and the production build pass.
- No keys, tokens, or local configuration are committed.

## Design constraints

Before changing code, check that it does not violate the [architecture principles](docs/00-overview/principles.md). In particular:

- The kernel must not contain domain vocabulary (`Session`, `Run`, `Tool`, ...). New features are plugins by default.
- Cross-boundary contracts are versioned; event schemas only grow.
- Optional modules must be removable; a missing required slot must refuse normal execution.
- When a security capability (policy, sandbox, approval) is missing, side-effecting actions are denied.

If a change involves a decision that is hard to reverse, counterintuitive without context, and has a real trade-off, add an ADR as well (see [docs/adr/README.md](docs/adr/README.md)).

## Style

- Rust: run `cargo fmt`; keep module boundaries clear and avoid direct dependencies between plugins.
- TypeScript / Solid: match the existing code; prefer small, revertible changes.
- Comments explain “why”, not “what”.

## Commits and pull requests

- One pull request solves one problem. Avoid unrelated refactors.
- Commit messages follow `type: summary` (`feat`, `fix`, `docs`, `refactor`, `test`, `chore`).
- Describe the motivation, behavior changes, and verification in the PR. Include screenshots for UI changes.
- Update affected documentation. When code and documentation disagree, decide which one is wrong and fix that side.

## Releases

Pushing a `v*` tag triggers CI to build and publish a GitHub Release. Pushes to `main` and manual CI runs build the same release artifacts and upload them as workflow artifacts (retained for 14 days).

The CI workflow:

1. Runs frontend and Rust checks.
2. Creates a draft release when the ref is a tag.
3. Builds Windows (NSIS), Linux, and macOS (universal) desktop bundles.
4. Builds an Android universal AAB and per-ABI APKs (release mode, R8 + resource shrinking).
5. Uploads everything to the GitHub Release (tags) or as workflow artifacts (branches), then publishes the draft after all platforms succeed.

The tag should match `version` in `src-tauri/tauri.conf.json`; Tauri derives the Android `versionName` and `versionCode` from it.

The Android project is pinned to compileSdk/targetSdk 36, build-tools 36.0.0, and NDK 27.2.12479018. The current Tauri template emits SDK 37, but `sdkmanager` does not yet publish an `android-37` package; verify package availability before bumping these pins.

Android release signing requires these repository secrets when publishing a tag:

| Secret | Description |
|---|---|
| `ANDROID_KEYSTORE_BASE64` | Base64 of the keystore file (`base64 -w0 release.keystore`) |
| `ANDROID_KEY_ALIAS` | Key alias |
| `ANDROID_KEY_PASSWORD` | Key password |
| `ANDROID_STORE_PASSWORD` | Keystore password |

Branch builds fall back to the debug keystore when the secrets are absent; tag builds fail without them. Desktop builds are unsigned until Windows code-signing and macOS notarization secrets are configured.

## License

By contributing, you agree to license your contribution under [GPL-3.0-only](LICENSE).
