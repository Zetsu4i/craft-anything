# Building in the cloud: E2B sandboxes (BYOK)

A project can build in its **own Linux sandbox in the cloud** instead of on this computer. The
builder's coding agent runs *inside* the sandbox with all of its usual tools — Bash, file edits,
installs, builds — exactly as if it were local, while everything it makes is copied back to the
project on this computer after every turn. Checks, previews and your own edits keep working the
way they always did.

The service is [E2B](https://e2b.dev), with **your own API key** (BYOK). Craft Anything is not
involved in your account beyond the calls it makes with the key you give it.

## Turning it on

1. **Settings → AI → Cloud sandbox → E2B API key**: paste your key from the E2B dashboard
   (or set `E2B_API_KEY` in the environment). "Test" checks it without starting anything.
2. **Sandbox: On**. From then on, every AI whose *Sandbox* button reads "as set below" builds
   in the cloud.
3. Per-AI choice (Settings → AI → each agent app): **Sandbox: always / never / as set below**.
   Claude Code and ZCode are the builders we run there today.

## What each project gets

- **One sandbox per project.** Its id is written beside the project's manifest
  (`sandbox.e2b.json`), so the next build — even after restarting the app — goes back to the
  same box.
- **The project is synced up** before a turn (only what changed travels; `node_modules`,
  `.git`, build caches and friends never do) and **synced down** after it, so the agent's work
  lands in the project on this computer. Files it deleted there are deleted here too.
- **The sandbox is put away when it isn't needed**: paused right after a turn (the setting
  "Between turns → Pause"), and paused by E2B itself after the idle timeout ("Put away after").
  A paused sandbox costs nothing; the next turn **brings it back** with its files exactly as
  they were. Turn **Restore off** (`auto_pause = false`) if you'd rather it was killed at the
  timeout and every project started fresh — nothing carries over then.
- **The app's tool server is there**: a Linux build of `craft-anything-cli` — carried by the
  template, uploaded from beside the app when the package has one, or fetched by the sandbox
  from this repository's release at setup — so the agent's approvals, questions and checklists
  work in the sandbox as usual.
- **The ZCode agent is set up there too** (when a ZCode builder runs): its bundle is fetched
  straight into the box, Node 22 is ensured, and the GLM bridge is health-checked — see
  `docs/zcode-agent.md`. Nothing of the agent lands on this computer.

## What stays where

| | On this computer | In the sandbox |
| --- | --- | --- |
| Project manifest, checks, evidence | yes (synced back) | synced copy |
| `app/` (what the agent builds) | yes (synced back) | where the agent works |
| `node_modules`, build caches | never (rebuild there) | created by the agent |
| The agent's own files (its sign-in, sessions) | — | in the sandbox, kept across pauses |

## Settings reference (`[ai.sandbox]` in `config.toml`)

| Setting | Default | What it does |
| --- | --- | --- |
| `enabled` | `false` | Builders with "as set below" build in the cloud. |
| `provider` | `"e2b"` | The sandbox service (the one there is today). |
| `template` | `"base"` | What a new sandbox starts from. The fork's own template (`craft-zcode`, built from `e2b-template/`) carries the agent, Node 22 and the whole tool workshop; `base` is E2B's plain Debian image and sets itself up at first turn. |
| `zcode_agent_url` | *(the agent repository's release)* | Where a sandbox fetches the ZCode agent bundle from, when its template doesn't carry it. |
| `timeout_secs` | `1800` | How long an unused sandbox stays up before E2B pauses it. |
| `auto_pause` | `true` | Pause (snapshot) at the timeout instead of killing it. |
| `pause_after_turn` | `true` | Pause right after every builder turn. |
| `ignore` | `[]` | Extra names that never travel (`.gitignore`-style: `secrets.json` matches everywhere, `app/secrets.json` exactly). |

The key itself lives in the system keychain (service `ai.storyteller.craft-anything`, entry
`sandbox:e2b`), with a 0600 fallback file when there is no keychain. It is never logged and
never written to a project file. The ZCode repo token (entry `sandbox:zcode-token`) is kept the
same way and is used by the sandbox alone.

## The template with everything inside

`e2b-template/` in this repository builds a sandbox template that boots fully set up: the ZCode
agent, its Node 22 runtime and bridge SDK, the CLI tool server, and the whole workshop of tools
(Java, Rust, Godot, adb, JADX, Apktool, Chrome, Frida, GDB, LLDB, mitmproxy, binwalk, unblob,
ilspycmd, tshark, objdump, Rizin, rtl_433, Vineflower, ripgrep, ugrep). Build it once
(`bash e2b-template/build.sh` — see its README), then put the template id in Settings → AI →
Sandbox → **Template**.
