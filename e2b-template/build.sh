#!/usr/bin/env bash
# Build the Craft Anything sandbox template (e2b-template/Dockerfile) and print the template id
# to put in Settings → AI → Sandbox → Template.
#
# What this needs on this computer:
#   1. Docker running.
#   2. The E2B CLI and an E2B API key:  pip install e2b  &&  e2b auth login
#      (or export E2B_API_KEY=…; the key from Settings → AI → Sandbox works).
#   3. When the agent's repository is private: a GitHub token with read access to it, exported as
#      GITHUB_TOKEN. It is passed to the build for that one download and never stored in the image.
#
# Usage:  bash e2b-template/build.sh
set -euo pipefail

cd "$(dirname "$0")/.."

if ! command -v e2b >/dev/null 2>&1; then
  echo "The E2B CLI is missing:  pip install e2b" >&2
  exit 1
fi
if ! docker info >/dev/null 2>&1; then
  echo "Docker isn't running: start it, then run this again." >&2
  exit 1
fi

args=(--dockerfile e2b-template/Dockerfile)
if [ -n "${GITHUB_TOKEN:-}" ]; then
  args+=(--build-arg "GITHUB_TOKEN=${GITHUB_TOKEN}")
  echo "Building with the agent fetch authenticated (GITHUB_TOKEN set)."
else
  echo "GITHUB_TOKEN is not set: the agent bundle is fetched unauthenticated."
  echo "If the agent repository is private, export GITHUB_TOKEN first, or this build will fail."
fi
[ -n "${AGENT_REPO:-}" ] && args+=(--build-arg "AGENT_REPO=${AGENT_REPO}")
[ -n "${AGENT_BUNDLE_API_URL:-}" ] && args+=(--build-arg "AGENT_BUNDLE_API_URL=${AGENT_BUNDLE_API_URL}")
[ -n "${CLI_URL:-}" ] && args+=(--build-arg "CLI_URL=${CLI_URL}")

e2b template build "${args[@]}"

echo
echo "Done. Put the template id (default: craft-zcode) in Settings → AI → Sandbox → Template."
