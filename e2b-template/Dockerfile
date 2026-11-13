# syntax=docker/dockerfile:1
# The Craft Anything ZCode sandbox template.
#
# Everything a builder could need is here, so a sandbox from this template is fully set up the
# moment it boots: the agent (fetched at build time from its own release — it never ships inside
# the app), Node 22 for it, and the whole workshop of tools below. Build it once on your machine
# (see build.sh), then put the template id in Settings → AI → Sandbox → Template.
#
# The build needs one secret when the agent's repository is private: GITHUB_TOKEN (a classic or
# fine-grained token with read access to it), passed as a build arg and used for that download
# alone. It is not baked into the image; the agent's model credentials arrive at runtime, from
# the app.

FROM ubuntu:22.04

ARG GITHUB_TOKEN=""
# The agent repository's release listing (API); its bundle asset is resolved and downloaded below.
ARG AGENT_BUNDLE_API_URL="https://api.github.com/repos/Zetsu4i/zcode-agent-core/releases/latest"
ARG AGENT_BUNDLE_ASSET="zcode-agent-bundle.tar.gz"
ARG AGENT_REPO="Zetsu4i/zcode-agent-core"
ARG CLI_URL="https://github.com/Zetsu4i/craft-anything/releases/latest/download/craft-anything-cli-linux"
ARG NODE_VERSION=22.20.0
ARG GODOT_VERSION=4.3-stable
ARG JADX_VERSION=1.5.1
ARG APKTOOL_VERSION=2.11.1
ARG RIZIN_VERSION=0.8.0
ARG VINEFLOWER_VERSION=1.10.1

ENV DEBIAN_FRONTEND=noninteractive
ENV TZ=UTC

# The base workshop: fetch, build, unpack, inspect.
RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates curl wget git unzip zip xz-utils bzip2 file jq gnupg lsb-release \
        build-essential cmake ninja-build pkg-config autoconf automake libtool \
        python3 python3-pip python3-dev python3-venv \
        openjdk-17-jdk \
        android-tools-adb \
        gdb lldb \
        binutils \
        binwalk \
        tshark \
        p7zip-full \
        libusb-1.0-0-dev librtlsdr-dev \
        ripgrep \
    && rm -rf /var/lib/apt/lists/*

# Node 22 — the agent needs it (>= 22.5, for node:sqlite), the bridge too.
RUN curl -fsSL https://nodejs.org/dist/v${NODE_VERSION}/node-v${NODE_VERSION}-linux-x64.tar.xz \
        -o /tmp/node.tar.xz \
    && tar -xJf /tmp/node.tar.xz -C /usr/local --strip-components=1 \
    && rm /tmp/node.tar.xz \
    && node --version && npm --version

# Rust, minimal profile, on PATH for every shell.
ENV RUSTUP_HOME=/usr/local/rustup CARGO_HOME=/usr/local/cargo PATH=/usr/local/cargo/bin:$PATH
RUN curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain stable \
    && rustc --version && cargo --version

# Python tooling: the dynamic-analysis kit.
RUN pip3 install --no-cache-dir --upgrade pip \
    && pip3 install --no-cache-dir frida-tools mitmproxy \
    && mitmdump --version && frida --version

# Unblob: the firmware extractor kit, plus the helpers it shells out to.
RUN pip3 install --no-cache-dir unblob || echo "unblob unavailable for this Python; install it later if needed"

# Chromium-based browser: Google Chrome (Ubuntu's chromium is a snap, which has no daemon here).
RUN wget -q https://dl.google.com/linux/direct/google-chrome-stable_current_amd64.deb -O /tmp/chrome.deb \
    && apt-get update && apt-get install -y --no-install-recommends /tmp/chrome.deb \
    && rm /tmp/chrome.deb && rm -rf /var/lib/apt/lists/* \
    && google-chrome --version

# Godot.
RUN wget -q https://github.com/godotengine/godot/releases/download/${GODOT_VERSION}/Godot_v${GODOT_VERSION}_linux.x86_64.zip -O /tmp/godot.zip \
    && unzip -q /tmp/godot.zip -d /tmp/godot \
    && mv /tmp/godot/Godot_v${GODOT_VERSION}_linux.x86_64 /usr/local/bin/godot \
    && chmod +x /usr/local/bin/godot && rm -rf /tmp/godot* \
    && godot --version

# JADX: dex to java decompiler.
RUN wget -q https://github.com/skylot/jadx/releases/download/v${JADX_VERSION}/jadx-${JADX_VERSION}.zip -O /tmp/jadx.zip \
    && unzip -q /tmp/jadx.zip -d /opt/jadx \
    && ln -sf /opt/jadx/bin/jadx /usr/local/bin/jadx && ln -sf /opt/jadx/bin/jadx-gui /usr/local/bin/jadx-gui \
    && rm /tmp/jadx.zip && jadx --version

# Apktool: APK rebuild and resource decoding.
RUN wget -q https://github.com/iBotPeaches/Apktool/releases/download/v${APKTOOL_VERSION}/apktool_${APKTOOL_VERSION}.jar -O /opt/apktool.jar \
    && printf '#!/bin/sh\nexec java -jar /opt/apktool.jar "$@"\n' > /usr/local/bin/apktool \
    && chmod +x /usr/local/bin/apktool && apktool --version

# Vineflower: the java decompiler.
RUN wget -q https://github.com/Vineflower/vineflower/releases/download/${VINEFLOWER_VERSION}/vineflower-${VINEFLOWER_VERSION}.jar -O /opt/vineflower.jar \
    && printf '#!/bin/sh\nexec java -jar /opt/vineflower.jar "$@"\n' > /usr/local/bin/vineflower \
    && chmod +x /usr/local/bin/vineflower

# ILSpy's command line, on .NET.
RUN curl -fsSL https://dot.net/v1/dotnet-install.sh -o /tmp/dotnet-install.sh \
    && bash /tmp/dotnet-install.sh --channel 8.0 --install-dir /usr/share/dotnet \
    && rm /tmp/dotnet-install.sh \
    && ln -sf /usr/share/dotnet/dotnet /usr/local/bin/dotnet \
    && export PATH="/usr/share/dotnet:/root/.dotnet/tools:$PATH" \
    && dotnet tool install --global ilspycmd \
    && /root/.dotnet/tools/ilspycmd --version

# Rizin (the radare2 fork), from its release deb.
RUN wget -q https://github.com/rizinorg/rizin/releases/download/v${RIZIN_VERSION}/rizin_${RIZIN_VERSION}_amd64.deb -O /tmp/rizin.deb \
    && apt-get update && apt-get install -y --no-install-recommends /tmp/rizin.deb \
    && rm /tmp/rizin.deb && rm -rf /var/lib/apt/lists/* \
    && rizin -v

# ugrep, the agent's other search binary.
RUN wget -q https://github.com/Genivia/ugrep/releases/download/v7.0.3/ugrep-linux-x64.tar.gz -O /tmp/ugrep.tgz \
    && tar -xzf /tmp/ugrep.tgz -C /tmp \
    && find /tmp/ugrep* -name ugrep -type f -exec mv {} /usr/local/bin/ugrep \; \
    && chmod +x /usr/local/bin/ugrep && rm -rf /tmp/ugrep* \
    && ugrep --version

# rtl_433: no release binaries for Linux; built from its latest tagged source.
RUN git clone --depth 1 https://github.com/merbanan/rtl_433.git /tmp/rtl_433 \
    && cmake -S /tmp/rtl_433 -B /tmp/rtl_433/build -DCMAKE_BUILD_TYPE=Release \
    && cmake --build /tmp/rtl_433/build --parallel "$(nproc)" \
    && cmake --install /tmp/rtl_433/build \
    && rm -rf /tmp/rtl_433 \
    && rtl_433 -h | head -1

# Craft Anything's own CLI tool server, the agent's command surface inside the sandbox.
RUN mkdir -p /craft-anything/bin \
    && curl -fsSL "${CLI_URL}" -o /craft-anything/bin/craft-anything-cli \
    && chmod +x /craft-anything/bin/craft-anything-cli

# The ZCode agent itself, fetched into the sandbox's image from its release — never shipped with
# the app. A private repository needs GITHUB_TOKEN for this one download; GitHub's browser
# download URLs answer 404 to a bare token there, so the API's asset endpoint serves the bytes.
RUN <<PYEOF
set -eu
mkdir -p /craft-anything/agent/config /craft-anything/zcode/home
export GITHUB_TOKEN="${GITHUB_TOKEN}" AGENT_REPO="${AGENT_REPO}" AGENT_BUNDLE_API_URL="${AGENT_BUNDLE_API_URL}" AGENT_BUNDLE_ASSET="${AGENT_BUNDLE_ASSET}"
python3 <<'PY'
import json, os, sys, urllib.request

token = os.environ.get("GITHUB_TOKEN", "")
repo = os.environ.get("AGENT_REPO") or "Zetsu4i/zcode-agent-core"
listing_url = os.environ.get("AGENT_BUNDLE_API_URL") or f"https://api.github.com/repos/{repo}/releases/latest"
asset_name = os.environ.get("AGENT_BUNDLE_ASSET") or "zcode-agent-bundle.tar.gz"
headers = {"Accept": "application/vnd.github+json", "User-Agent": "craft-anything-template"}
if token:
    headers["Authorization"] = f"Bearer {token}"

req = urllib.request.Request(listing_url, headers=headers)
with urllib.request.urlopen(req, timeout=60) as r:
    release = json.load(r)
assets = [a for a in release.get("assets", []) if a.get("name") == asset_name]
if not assets:
    sys.exit(f"{listing_url} holds no {asset_name} asset")

fetch = urllib.request.Request(assets[0]["url"], headers={**headers, "Accept": "application/octet-stream"})
with urllib.request.urlopen(fetch, timeout=300) as r, open("/tmp/zcode-agent.tgz", "wb") as f:
    while True:
        chunk = r.read(1 << 20)
        if not chunk:
            break
        f.write(chunk)
PY
tar -xzf /tmp/zcode-agent.tgz -C /craft-anything/agent --strip-components=1
rm /tmp/zcode-agent.tgz
test -f /craft-anything/agent/zcode.cjs
test -f /craft-anything/agent/config/zcode-builtin.json
test -f /craft-anything/agent/zai-bridge.cjs
PYEOF

# The SDK the bridge serves the free provider through, installed beside the agent (the bridge
# falls back to /etc/.z-ai-config on its own when the SDK is missing).
RUN cd /craft-anything/agent && npm install --omit=dev z-ai-web-dev-sdk --no-fund --no-audit

# What the app expects to find when it opens a sandbox from this template.
ENV PATH="/craft-anything/bin:/root/.dotnet/tools:${PATH}"
WORKDIR /craft-anything
