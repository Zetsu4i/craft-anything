# e2b-template — the sandbox template with everything inside

A sandbox from this template boots **fully set up**: the ZCode agent, its Node 22 runtime and
bridge SDK, Craft Anything's CLI tool server, and the whole workshop of build/analysis tools.
Nothing is downloaded at first turn, and nothing the agent needs is missing.

## The tools it carries

| Purpose | Tools |
| --- | --- |
| Languages & runtimes | Java 17 (JDK), Rust (stable), Node 22, Python 3, .NET 8 |
| Game & app building | Godot 4.3, Android platform tools (adb) |
| Android reversing | JADX, Apktool |
| Web & network | Google Chrome (Chromium-based), mitmproxy (`mitmdump`), Wireshark (`tshark`) |
| Debugging | GDB, LLDB, Frida, Rizin, objdump (binutils) |
| Firmware & binaries | Binwalk, Unblob |
| .NET & Java decompiling | ilspycmd, Vineflower |
| Radio | rtl_433 (built from source) |
| Agent's own tools | ripgrep, ugrep |

## Build it (once, on your machine)

```sh
pip install e2b
e2b auth login                      # or: export E2B_API_KEY=<the key from Settings → AI → Sandbox>
export GITHUB_TOKEN=<token>         # needed only while the agent's repository is private
bash e2b-template/build.sh          # Docker must be running
```

Then put the template id — `craft-zcode` — in **Settings → AI → Sandbox → Template**.

## What the build fetches, and from where

| Piece | Source |
| --- | --- |
| The ZCode agent (`zcode.cjs`, `zcode-builtin.json`, `zai-bridge.cjs`) | `Zetsu4i/zcode-agent-core` release (`zcode-agent-bundle.tar.gz`); authenticated with `GITHUB_TOKEN` when the repository is private |
| Craft Anything's CLI tool server | `Zetsu4i/craft-anything` release (`craft-anything-cli-linux`) |
| Everything else | Their upstream releases (nodejs.org, crates.io, pip, apt) |

The token is a build argument for the one authenticated download; it is not part of the image.
The agent's model credentials never enter the image either: the app copies the person's Z.ai
credentials into the running sandbox at setup only when "Z.ai credentials in the sandbox" is on.

## Without the template

Sandboxes from any other template (E2B's `base`, say) set themselves up at first turn: the app's
sandbox code installs Node 22 when needed, fetches the agent bundle and the CLI tool server into
the box, and starts the bridge. A sandbox from `base` needs the ZCode repo token saved in
Settings → AI → Sandbox whenever the agent's repository is private.
