# This fork

`Zetsu4i/craft-anything` tracks `storytold/craft-anything` (upstream) and carries changes the
upstream doesn't have. Everything here is written to **fit the upstream's seams** — trait
implementations registered in one place, settings in the existing tables, no core rewrites — so
future upstream merges stay boring.

## What this fork adds

1. **E2B cloud sandboxes** — `docs/e2b-sandboxes.md`. A project builds in its own Linux box in
   the cloud (BYOK key in Settings), the work synced back after every turn, the sandbox paused
   and brought back on its own.
2. **The ZCode agent** — `docs/zcode-agent.md`. A bundled coding agent with a built-in GLM
   provider, run in the background over its app-server protocol.
3. **A self-contained Windows build** — `.github/workflows/windows-build.yml` (portable exe
   package; the upstream release pipeline needs private secrets this fork doesn't have).

## Where each change lives (for merges)

| Area | Files | Notes |
| --- | --- | --- |
| E2B client | `crates/engines/src/e2b.rs` | New module, nothing upstream touched except one `pub mod e2b;` line. |
| Sandbox manager | `crates/ai/src/sandbox.rs` | New module; `pub mod sandbox;` in `ai/src/lib.rs`. |
| Settings | `crates/ai/src/config.rs` | `[ai.sandbox]` table on `AiConfig`, `sandbox` option on `BackendOptions` — additive, serde defaults, no migrations needed. |
| Spawn | `crates/ai/src/cli_agents/mod.rs` | `CliSpec.sandbox`; `run_cli`'s process building refactored into a local `RunHooks` struct so the sandbox runner and the local runner share the same event handling; the sync-down and pause after a turn. Upstream changes to `run_cli` will want a look at that block. |
| ZCode agent | `crates/ai/src/cli_agents/zcode.rs` | New adapter + conversation; registered in `adapters.rs` (`built_in`), `KNOWN`, and `BackendKind::suggested_models`. |
| Builder wiring | `crates/ai/src/rebuild/mod.rs` | `agent_spec` opens the project's sandbox when the backend wants one. |
| Keys | `crates/ai/src/keys.rs` | `SANDBOX_KEY` + `sandbox_key()`; `AiHub` gained sandbox key/test/config methods and ZCode may carry a key like an API backend. |
| Settings UI | `crates/ui-egui/src/settings_ui.rs` | "Cloud sandbox" panel in the AI tab; per-AI "Sandbox: …" buttons; ZCode's key row. |
| Bundled agent | `resources/zcode/` | `zcode.cjs` and the search binaries are fetched at build time; the bridge and the provider catalogue are committed. |
| CI | `.github/workflows/` | `windows-build.yml` is new; `ci.yml` is self-contained; the upstream-only pipelines are manual-only with a note. |

## Merge rules of thumb

- Upstream refactors of `run_cli` / `agent_spec` / `settings_ui::ai` will touch the same
  regions; the fork's additions are fenced with their own doc comments ("The project's
  sandbox…" / "The ZCode agent…") so conflicts are readable.
- Upstream's `Settings`/`AiConfig` are additive here (new optional fields only), so newer
  upstream settings files load unchanged.
- The one vendored delta upstream already carries (`vendor/wgpu-hal`) is untouched.
