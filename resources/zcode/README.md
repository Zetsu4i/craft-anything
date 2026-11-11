# resources/zcode — the ZCode agent, as Craft Anything ships it

This folder is what the desktop app carries for the **ZCode (GLM built-in)** builder: the
person's own coding agent (`zcode.cjs`), run in the background and spoken to over its NDJSON
app-server protocol instead of a per-message CLI run.

| File | What it is | Committed? |
| --- | --- | --- |
| `zai-bridge.cjs` | The local bridge behind "GLM (built-in)": serves `POST /llm/v1/chat/completions` from the machine's `.z-ai-config` (working folder → home → `/etc`). No dependencies. | yes |
| `config/zcode-builtin.json` | The agent's provider catalogue; the app points `ZCODE_BUILTIN_PROVIDER_CONFIG_FILE` at it. | yes |
| `zcode.cjs` | The agent itself (Node ≥ 22.5). ~15 MB, fetched at build time so this repository stays light. | no — see below |
| `node/` | A Node 22 runtime for Windows, fetched at build time. | no — see below |
| `resources/rg`, `resources/ugrep`, `resources/bfs` | The agent's search binaries, fetched at build time (per platform). | no — see below |

## Where the fetched pieces come from

The build workflow (`.github/workflows/windows-build.yml`) fetches, in order:

1. `zcode.cjs` from `Zetsu4i/zcode-agent-core` (branch `main`): `extracted/zcode.cjs` — overwrite
   it by uploading a new version there; no change is needed on this side.
2. `node-v22*-win-x64` from nodejs.org, extracted into `node/`.
3. `rg` (ripgrep), `ugrep` and `bfs` Windows builds from their GitHub releases, into `resources/`.

For a **local** development run, either build the workflow once and copy its artifact, or drop
the files in by hand:

```sh
# any folder works; point CRAFT_ANYTHING_ZCODE_DIR at it if it isn't <exe>/resources/zcode
curl -o resources/zcode/zcode.cjs https://raw.githubusercontent.com/Zetsu4i/zcode-agent-core/main/extracted/zcode.cjs
```

## How the app runs it

- Local: `node zcode.cjs app-server --stdio`, with `ZCODE_BUILTIN_PROVIDER_CONFIG_FILE`,
  `ZCODE_PERSONAL_PROVIDER_CONFIG_FILE` and `ZCODE_STORAGE_DIR` inside the app's own config dir,
  so an installed ZCode is never touched.
- In the project's sandbox: the same, with the agent and its files uploaded to
  `/craft-anything/agent` at sandbox setup and Node 22 installed there if the template's is older.

## The "free built-in" provider

- If the machine has Z.ai credentials (`.z-ai-config`), the app starts `zai-bridge.cjs` and the
  agent's provider config points at it — no key asked for.
- Otherwise, save a Z.ai API key for ZCode in Settings → AI; the agent's provider config then
  points straight at `https://api.z.ai/api/paas/v4`.
