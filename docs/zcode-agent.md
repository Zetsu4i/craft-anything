# The ZCode agent (GLM built-in)

**ZCode** is this fork's default coding agent. It runs in the background and is spoken to over
its own protocol instead of a per-message CLI run: one `app-server` process per conversation,
one JSON message per line, sessions that outlive single messages. Its builder tools (Bash,
Read, Write, Edit, and the rest) work on the project's folder wherever it runs — here, or in
the project's sandbox.

The agent is **not shipped with the app**. It lives in its own repository
([`Zetsu4i/zcode-agent-core`](https://github.com/Zetsu4i/zcode-agent-core)) and gets to where it
runs two ways:

- **In a sandbox (the default route):** the sandbox fetches the agent bundle straight into the
  box at setup — from the sandbox template that carries it, or from the agent's own release.
  The bundle never lands on the computer the app runs on, and nothing of it is inside the exe.
- **On this computer (optional):** anyone who keeps a copy of the agent (the fork's owner, a
  developer) points Settings → AI → **ZCode agent folder** at the folder holding `zcode.cjs`.
  Without a folder named and without the sandbox on, ZCode doesn't offer local runs — for
  everyone else it is a sandbox-only agent.

Its model is the **free built-in provider** ("GLM (built-in)"), served one of two ways:

1. **The bridge** (`zai-bridge.cjs`, from the bundle, started beside the agent): fed by Z.ai
   credentials — on this computer (`.z-ai-config` in the home folder or `/etc`), or copied into
   the sandbox at setup when "Z.ai credentials in the sandbox" is on. No key is asked for.
2. **A Z.ai API key** (Settings → AI → ZCode): the agent points straight at
   `https://api.z.ai/api/paas/v4` with the saved key, in the sandbox or out of it.

## Using it

Settings → AI → **Look for AI**: ZCode is offered first, whether or not a local copy exists
(the reason line says which). "Build with this" makes it the builder, exactly like Claude Code
or Codex. Its permission requests (writes, commands) come to the app as the same approval cards
the other builders use.

## How it's wired

- Local run: `node zcode.cjs app-server --stdio`, the node looked up beside the agent folder
  first, then `node` on PATH (22.5 or newer — the agent needs `node:sqlite`).
- The app generates the agent's personal provider config under its own config dir
  (`<config>/zcode/provider_config.json`), and keeps the agent's state there as well
  (`ZCODE_STORAGE_DIR`), so an installed ZCode is never touched.
- The conversation is `cli_agents::zcode::ZcodeConversation`: create → subscribe → send, events
  streamed (`text_delta`, tool calls, results), permission requests answered through the same
  approval policy as ACP agents, `session/stop` when you press stop. A saved session id is
  kept with the project and resumed with `session/resume` on the next run.
- The route decides the model the session asks for: `zai-glm` behind the bridge, `glm-4.6`
  direct to Z.ai with a saved key.

## In a sandbox

With E2B sandboxes on (see `docs/e2b-sandboxes.md`), the same agent runs *inside* the project's
sandbox: the bundle is fetched there at setup (`/craft-anything/agent`), Node 22 is installed
if the template's is older, the bridge starts there and is health-checked before the first
turn, and the protocol runs over the sandbox's process API instead of a local pipe. The model
route is decided at setup: credentials in the sandbox serve the bridge; otherwise the saved
Z.ai key goes direct; without either the turn says exactly what to set and stops.

The bundle download goes to the sandbox alone. When the agent repository is private, save a
token in Settings → AI → Sandbox → **ZCode repo token**: the app resolves the release's asset
through the API (GitHub's browser download URLs answer 404 to a bare token on private
repositories) and the sandbox fetches the bytes itself with that token.

## Troubleshooting

| What you see | Why | The fix |
| --- | --- | --- |
| "The ZCode agent isn't on this computer" | No local folder named, sandbox off | Point Settings → AI → ZCode agent folder at your copy, or turn on the sandbox. |
| "The sandbox couldn't fetch the ZCode agent" | Private repository and no/weak token | Settings → AI → Sandbox → ZCode repo token; check the URL too. |
| "The sandbox's GLM bridge didn't start" | The bridge inside the sandbox failed | Its log is `/craft-anything/agent/bridge.log` in the sandbox. |
| "ZCode has no way to reach its GLM model" | No key saved and no credentials to travel | Save a Z.ai key for ZCode, or turn on "Z.ai credentials in the sandbox". |
| "The ZCode agent's provider catalogue is missing" | The local folder has no `config/zcode-builtin.json` | Get the full bundle (or use the sandbox route, which needs no local copy). |
