# resources/zcode — a local copy of the ZCode agent, when you keep one here

The app does **not** ship the ZCode agent: it lives in its own repository
([`Zetsu4i/zcode-agent-core`](https://github.com/Zetsu4i/zcode-agent-core)), which publishes
`zcode-agent-bundle.tar.gz` (the agent `zcode.cjs`, the provider catalogue
`config/zcode-builtin.json`, and the bridge `zai-bridge.cjs`). Sandboxes fetch that bundle
straight into the box at setup — nothing of it lands on this computer or inside the exe.

This folder is only a **convenience for local runs**: when Settings → AI → ZCode agent folder
(or `CRAFT_ANYTHING_ZCODE_DIR`) names a folder holding `zcode.cjs`, that copy runs on this
computer. Dropping the bundle's files here (`zcode.cjs`, `config/zcode-builtin.json`,
`zai-bridge.cjs`, and optionally `node/node.exe` and `resources/rg.exe`/`ugrep.exe`) makes this
folder work as that local copy for a build of the app beside it.

## How the app runs it

- Local: `node zcode.cjs app-server --stdio`, with `ZCODE_BUILTIN_PROVIDER_CONFIG_FILE`,
  `ZCODE_PERSONAL_PROVIDER_CONFIG_FILE` and `ZCODE_STORAGE_DIR` inside the app's own config dir,
  so an installed ZCode is never touched. Node 22.5+ is needed (the agent uses `node:sqlite`);
  a `node/node.exe` beside the copy wins over PATH.
- In the project's sandbox: the same, with everything fetched into the sandbox at setup — see
  `docs/zcode-agent.md` and `docs/e2b-sandboxes.md`.

## The "free built-in" provider

- If the machine has Z.ai credentials (`.z-ai-config`), the app starts `zai-bridge.cjs` beside
  the local copy and the agent's provider config points at it — no key asked for.
- Otherwise, save a Z.ai API key for ZCode in Settings → AI; the agent's provider config then
  points straight at `https://api.z.ai/api/paas/v4`.
