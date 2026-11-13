# This fork

`Zetsu4i/craft-anything` tracks `storytold/craft-anything` (upstream) and carries changes the
upstream doesn't have. Everything here is written to **fit the upstream's seams** — trait
implementations registered in one place, settings in the existing tables, no core rewrites — so
future upstream merges stay boring.

## What this fork adds

1. **E2B cloud sandboxes** — `docs/e2b-sandboxes.md`. A project builds in its own Linux box in
   the cloud (BYOK key in Settings), the work synced back after every turn, the sandbox paused
   and brought back on its own.
2. **The ZCode agent (the default builder)** — `docs/zcode-agent.md`. The person's own coding
   agent, run in the background over its app-server protocol with a free built-in GLM provider.
   Not shipped with the app: sandboxes fetch the bundle into the box at setup (a template
   carries it, or it comes from the agent repository's release), and a local run needs a folder
   the person points Settings → AI at.
3. **A sandbox template with everything** — `e2b-template/`. One Dockerfile that bakes the
   agent, Node 22, the tool workshop and the CLI tool server into an E2B template
   (`craft-zcode`), selected in Settings → AI → Sandbox → Template.
4. **A self-contained Windows build** — `.github/workflows/windows-build.yml` (portable exe
   package; the upstream release pipeline needs private secrets this fork doesn't have). The
   package carries no agent and no duplicate CLI: the app and its CLI helper, and that's it.
   Tag builds also publish the Linux CLI as a release asset sandboxes can fetch.

## Where each change lives (for merges)

| Area | Files | Notes |
| --- | --- | --- |
| E2B client | `crates/engines/src/e2b.rs` | New module, nothing upstream touched except one `pub mod e2b;` line. |
| Sandbox manager | `crates/ai/src/sandbox.rs` | New module; `pub mod sandbox;` in `ai/src/lib.rs`. |
| Settings | `crates/ai/src/config.rs` | `[ai.sandbox]` table on `AiConfig`, `sandbox` option on `BackendOptions` — additive, serde defaults, no migrations needed. |
| Spawn | `crates/ai/src/cli_agents/mod.rs` | `CliSpec.sandbox`; `run_cli`'s process building refactored into a local `RunHooks` struct so the sandbox runner and the local runner share the same event handling; the sync-down and pause after a turn. Upstream changes to `run_cli` will want a look at that block. |
| ZCode agent | `crates/ai/src/cli_agents/zcode.rs` | New adapter + conversation; registered in `adapters.rs` (`built_in`), `KNOWN`, and `BackendKind::suggested_models`. |
| Builder wiring | `crates/ai/src/rebuild/mod.rs` | `agent_spec` opens the project's sandbox when the backend wants one. |
| Keys | `crates/ai/src/keys.rs` | `SANDBOX_KEY` + `sandbox_key()`, `AGENT_TOKEN` + `agent_token()`; `AiHub` gained sandbox key/test/config methods, the agent-folder setting (`AiConfig.zcode_dir`) and its setters; ZCode may carry a key like an API backend. |
| Settings UI | `crates/ui-egui/src/settings_ui.rs` | "Cloud sandbox" panel in the AI tab; per-AI "Sandbox: …" buttons; ZCode's key row. |
| Sandbox template | `e2b-template/` | Dockerfile + build script + README; `e2b.toml` at the repository root. |
| Agent bundle release | `Zetsu4i/zcode-agent-core` | The private repository publishes `zcode-agent-bundle.tar.gz` (zcode.cjs, the catalogue, the bridge); sandboxes and the template fetch it. |
| CI | `.github/workflows/` | `windows-build.yml` is new; `ci.yml` is self-contained; the upstream-only pipelines are manual-only with a note. |

## Staying current with upstream

The fork sits on its own branch (`feature/e2b-sandbox-and-zcode`); `main` follows upstream.

```sh
git remote add upstream https://github.com/storytold/craft-anything.git   # once
git fetch upstream main
git checkout feature/e2b-sandbox-and-zcode
git merge upstream/main            # keep our changes; resolve conflicts, then commit
cargo check -p craft-anything-ai -p craft-anything-ui-egui && cargo test -p craft-anything-ai --lib
git push origin feature/e2b-sandbox-and-zcode
```

Our additions are new files (`zcode.rs`, `sandbox.rs`, `e2b.rs`, `e2b-template/`) plus small,
fenced edits in shared files — each carries its own doc comments, so conflicts stay readable
and ours usually win by keeping both sides. The settings and keys tables are additive (serde
defaults), so an upstream update never needs a settings migration.

## Merge rules of thumb

- Upstream refactors of `run_cli` / `agent_spec` / `settings_ui::ai` will touch the same
  regions; the fork's additions are fenced with their own doc comments ("The project's
  sandbox…" / "The ZCode agent…") so conflicts are readable.
- Upstream's `Settings`/`AiConfig` are additive here (new optional fields only), so newer
  upstream settings files load unchanged.
- The one vendored delta upstream already carries (`vendor/wgpu-hal`) is untouched.
