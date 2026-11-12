# Working on phi-agent

This file is for coding agents and contributors. It records the rules that are
easy to violate by accident.

## Non-negotiable rules

1. **Source code is English.** Identifiers, comments, log keys and commit
   messages are English. Do not introduce Chinese string literals in Rust,
   TypeScript or tests.
2. **Every human-visible string goes through i18n.** In Rust use
   `phi_i18n::t("English message")` or
   `phi_i18n::t_args("Hello {name}", &[("name", name)])` (a crate may
   `use phi_i18n as i18n;`). The English text is the message id;
   `src-tauri/crates/phi-i18n/src/lib.rs` is the only file that contains
   translations. Add the Chinese translation there when you add a message.
   Later UI work should follow the same rule (English source, translated
   catalog).
3. **Headless mode is mandatory.** Every shared feature must be usable and
   tested without the desktop UI. `phi-headless` is the reference shell:
   `cargo run -p phi-headless -- <command>`.
   A feature is not done until it has been exercised through headless mode or
   through an integration test that composes the same `Node`.
4. **Do not force a local build.** Some development machines only have a
   partial Rust toolchain (for example, no MSVC linker). Do not spend time
   fighting the local linker. Push a branch and read GitHub Actions with `gh`:

   ```bash
   gh run list --branch <branch> --limit 5
   gh run watch <run-id>
   gh run view <run-id> --log-failed
   ```

   `cargo fmt` is safe locally. `cargo check`/`cargo test` may not be.
5. **README is user-facing only.** `README.md` and `README.zh-CN.md` describe
   what a person can do and experience: capabilities, safety promises,
   requirements, quick start, FAQ and roadmap. Keep the two language versions
   in sync, and make every claim true for the shipped desktop UI. Do not put
   architecture, plugin/slot/kernel internals, headless CLI usage, wire
   protocols, environment variables, data paths, exit codes or CI details in
   the READMEs; those belong here or in `docs/`.

## Layout

The Rust code is a workspace under `src-tauri/`. Crates exist to enforce the
dependency direction at compile time:

- `crates/phi-kernel/` — mechanisms only (event bus, lifecycle, registry,
  mechanism ports, atomic file replacement). No domain vocabulary.
- `crates/phi-domain/` — messages, domain events and session read models.
- `crates/phi-contracts/` — ports, typed service handles, provider profiles,
  stream events, extension-slot names.
- `crates/phi-plugins/` — built-in implementations: model gateway, protocol
  adapters, tools, policy, approvals, session projection, sandbox.
- `crates/phi-adapters/` — filesystem journal/blob/config, system clock/ids,
  JSONL sinks, data-directory lock and cross-process control files.
- `crates/phi-i18n/` — language selection and the translation catalog.
- `crates/phi-node/` — shell-independent composition. Desktop and headless
  both call `compose` and use the same `Node`.
- `crates/phi-headless/` — headless shell. Durable events and live model deltas
  are JSONL on stdout; logs and summaries are on stderr.
- `src-tauri/src/` — desktop shell only (Tauri commands, sinks, entry point).
  No Tauri type may appear outside this crate.
- `src/` — SolidJS desktop UI.

The intended dependency graph is `kernel → domain → contracts → plugins →
adapters → node → shells`. Never add a dependency from a lower crate to a
higher one.

## Headless workflow

```bash
# One-shot run (mock model when no provider is configured)
phi-headless --data-dir .phi --workspace . --message "list the workspace"

# Pick a provider and a language
phi-headless --lang zh-CN run --provider prov_xxx --message "hello"
```

Exit codes: `0` success, `3` aborted, `4` failed, `5` awaiting
reconciliation, `2` usage/configuration error.

Only one writable node may use a data directory at a time; `compose` takes an
exclusive lock (`<data-dir>/phi.lock`). Read-only commands (`sessions show`,
`status`, provider reads) project the journal without locking. `approve` and
`cancel` hand a command file to the running process through
`<data-dir>/control/`, because a separate CLI invocation cannot reach another
process's memory.

Providers are durable profiles in `<data-dir>/providers.json`. Credentials are
references (`env:NAME`, `literal:VALUE`, `keychain:NAME`). Read APIs only ever
return a redacted hint. The keychain adapter is not implemented yet and fails
honestly.

## Model gateway

Three wire protocols are supported: `anthropic_messages`, `openai_chat` and
`openai_responses`, each with SSE streaming. Streaming deltas are transient and
go to the shell sink; only the final assistant message is journaled.

## Intelligent UI

The desktop UI can render OpenUI Lang (the generative-UI format used by
[open-intelligent-ui](https://github.com/thesysdev/open-intelligent-ui)) through
`@openuidev/browser-bundle`, which provides the React renderer and the built-in
OpenUI chat component library. UI runs use a dedicated system prompt
(`src-tauri/assets/openui-chat-system-prompt.txt`); the model's entire answer is
OpenUI Lang. Keep the prompt regenerated with `bun run generate:openui` when
the library version changes.

## Tests and CI

- Rust unit tests live next to the code; integration tests live in
  `src-tauri/crates/phi-headless/tests/`.
- Headless smoke tests use `env!("CARGO_BIN_EXE_phi-headless")`.
- CI runs `cargo fmt --all -- --check`, frontend typecheck/build, the full
  workspace test suite on Windows, and a Linux job for every crate except the
  desktop shell (`cargo test --workspace --exclude phi-agent`).
