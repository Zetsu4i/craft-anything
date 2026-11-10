# The ZCode agent (GLM built-in)

**ZCode** is a coding agent that ships with the app and runs in the background, spoken to over
its own protocol instead of a per-message CLI run: one `app-server` process per conversation,
one JSON message per line, sessions that outlive single messages. Its builder tools (Bash,
Read, Write, Edit, and the rest) work on the project's folder wherever it runs — here, or in
the project's sandbox.

Its model is the **free built-in provider** ("GLM (built-in)"), served one of two ways:

1. **The local bridge** (`zai-bridge.cjs`, started by the app when needed): on machines with
   Z.ai credentials (`.z-ai-config` in the home folder or `/etc`), the model is served through
   the machine's own account. No key is asked for.
2. **A Z.ai API key** (Settings → AI → ZCode): where there are no machine credentials, save a
   key and the agent points straight at `https://api.z.ai/api/paas/v4`.

## Using it

Settings → AI → add an agent app → **zcode**. "Build with this" makes it the builder, exactly
like Claude Code or Codex. Its permission requests (writes, commands) come to the app as the
same approval cards the other builders use.

## How it's wired

- The agent (`zcode.cjs`) and its provider catalogue (`config/zcode-builtin.json`) are bundled
  under `resources/zcode/` — see that folder's README for what's fetched at build time and why.
- A Node 22 runtime is bundled too, so the app doesn't depend on one being installed. A system
  Node is used if the bundle doesn't carry one.
- The app generates the agent's personal provider config under its own config dir
  (`<config>/zcode/provider_config.json`), and keeps the agent's state there as well
  (`ZCODE_STORAGE_DIR`), so an installed ZCode is never touched.
- The conversation is `cli_agents::zcode::ZcodeConversation`: create → subscribe → send, events
  streamed (`text_delta`, tool calls, results), permission requests answered through the same
  approval policy as ACP agents, `session/stop` when you press stop. A saved session id is
  kept with the project and resumed with `session/resume` on the next run.

## In a sandbox

With E2B sandboxes on (see `docs/e2b-sandboxes.md`), the same agent runs *inside* the project's
sandbox: its files are uploaded at setup (`/craft-anything/agent`), Node 22 is installed there
if the template's is older, and the protocol runs over the sandbox's process API instead of a
local pipe. The model is served by a bridge **inside the sandbox**, which needs one of:

- the machine's Z.ai credentials copied into the sandbox at setup
  (`[ai.sandbox] provide_zai_credentials = true` — they are written to `/etc/.z-ai-config`
  there and nowhere else), **or**
- nothing else today: in a sandbox there is no key prompt, so without the credentials copied
  the turn says exactly that and stops. (A BYOK key inside the sandbox is on the roadmap; the
  credentials route is the one the agent's own documentation describes.)

## Troubleshooting

| What you see | Why | The fix |
| --- | --- | --- |
| "ZCode has no way to reach its GLM model" | No key saved and no machine credentials | Save a Z.ai key in Settings → AI → ZCode, or add `~/.z-ai-config`. |
| "The ZCode agent isn't bundled" | `resources/zcode/zcode.cjs` missing (a source build without the fetch) | Build with the Windows workflow, or see `resources/zcode/README.md`. |
| "needs an E2B API key" | Sandbox on, key missing | Settings → AI → Cloud sandbox → key. |
| A turn says nothing and ends at once | The bridge found no credentials and no SDK | Check `curl localhost:3031/health` while a conversation runs, and that `.z-ai-config` or a saved key exists. |
