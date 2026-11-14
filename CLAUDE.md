# Claude project instructions

Read `AGENT.md`. The short version:

1. All source code, comments and identifiers are English. Human-visible strings
   use `i18n::t` / `i18n::t_args` (Rust) or the matching frontend catalog;
   Chinese text lives only in the translation catalog.
2. Headless mode is mandatory. Exercise shared features through
   `phi-headless` or an integration test that composes the same `Node`.
3. Do not force local compilation. `cargo fmt` is fine; push a branch and
   inspect GitHub Actions with `gh run view --log-failed`.
4. Preserve the microkernel/plugin boundaries: no Tauri type in `node.rs` or
   domain/plugin code.
5. `README.md` / `README.zh-CN.md` are user-facing only (experience, features,
   safety, setup). Architecture and developer details belong in `AGENT.md` or
   `docs/`.

Useful commands:

```bash
cargo fmt --all
gh run list --branch <branch> --limit 5
gh run watch <run-id>
gh run view <run-id> --log-failed
```
