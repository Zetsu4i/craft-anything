#!/usr/bin/env node
/**
 * zai-bridge — the local bridge behind Craft Anything's ZCode agent ("GLM (built-in)").
 *
 * ZCode speaks OpenAI Chat Completions; this bridge serves them from the machine's Z.ai
 * credentials, the way Z.ai's own hosted machines do: `z-ai-web-dev-sdk` (found preinstalled, or
 * installed globally) carries the account, and this bridge translates. When the SDK isn't there
 * but `.z-ai-config` is, requests go straight to the endpoint it names.
 *
 * Endpoints: GET /health, POST /llm/v1/chat/completions. Port 3031 (CRAFT_ZAI_BRIDGE_PORT).
 * No dependencies beyond the SDK itself; Node 18 or newer. Started by Craft Anything when a
 * ZCode conversation needs it, and never given credentials it wasn't found on this machine.
 */

'use strict';

const http = require('http');
const fs = require('fs');
const path = require('path');
const os = require('os');

const PORT = Number(process.env.CRAFT_ZAI_BRIDGE_PORT || 3031);
const CONFIG_CANDIDATES = [
  path.join(process.cwd(), '.z-ai-config'),
  path.join(os.homedir(), '.z-ai-config'),
  '/etc/.z-ai-config',
];
// Where the SDK is looked for: a normal require, then the usual global installs.
const SDK_CANDIDATES = [
  'z-ai-web-dev-sdk',
  '/usr/lib/node_modules/z-ai-web-dev-sdk/dist/index.js',
  '/usr/local/lib/node_modules/z-ai-web-dev-sdk/dist/index.js',
  path.join(os.homedir(), '.bun/install/global/node_modules/z-ai-web-dev-sdk/dist/index.js'),
];

function loadConfig() {
  for (const file of CONFIG_CANDIDATES) {
    try {
      const config = JSON.parse(fs.readFileSync(file, 'utf8'));
      if (config && config.baseUrl && config.apiKey) return { ...config, source: file };
    } catch {
      // no config here; the next place is tried
    }
  }
  return null;
}

async function loadSdk() {
  for (const candidate of SDK_CANDIDATES) {
    try {
      const mod = candidate.includes('/') ? await import(candidate) : await import(candidate);
      const ZAI = mod.default || mod;
      if (ZAI) return ZAI;
    } catch {
      // not here; the next place is tried
    }
  }
  return null;
}

function readBody(req) {
  return new Promise((resolve, reject) => {
    const chunks = [];
    let size = 0;
    req.on('data', (chunk) => {
      size += chunk.length;
      if (size > 64 * 1024 * 1024) {
        reject(new Error('request body too large'));
        req.destroy();
        return;
      }
      chunks.push(chunk);
    });
    req.on('end', () => resolve(Buffer.concat(chunks)));
    req.on('error', reject);
  });
}

function sendJson(res, status, body) {
  const text = JSON.stringify(body);
  if (!res.headersSent) {
    res.writeHead(status, { 'Content-Type': 'application/json', 'Content-Length': Buffer.byteLength(text) });
  }
  res.end(text);
}

/** The OpenAI-shaped request, with what Z.ai's model needs made explicit. */
function sdkRequest(parsed) {
  return {
    ...parsed,
    stream: Boolean(parsed.stream),
    thinking: parsed.thinking || { type: 'disabled' },
  };
}

async function viaSdk(res, parsed) {
  const ZAI = await loadSdk();
  if (!ZAI) return false;
  const zai = await ZAI.create();
  const request = sdkRequest(parsed);
  if (request.stream) {
    res.writeHead(200, { 'Content-Type': 'text/event-stream', 'Cache-Control': 'no-cache', Connection: 'keep-alive' });
    const stream = await zai.chat.completions.create(request);
    for await (const chunk of stream) {
      // The SDK's stream hands over raw SSE lines (a Uint8Array per chunk, or a parsed object on
      // older versions); both become SSE text on the wire as they come.
      const text = typeof chunk === 'string'
        ? chunk
        : chunk instanceof Uint8Array
          ? Buffer.from(chunk).toString('utf8')
          : `data: ${JSON.stringify(chunk)}\n`;
      res.write(text.endsWith('\n') ? text : `${text}\n`);
    }
    res.write('data: [DONE]\n\n');
    res.end();
    return true;
  }
  const answer = await zai.chat.completions.create(request);
  sendJson(res, 200, answer);
  return true;
}

/** The straight-to-endpoint fallback: the config's own words, nothing smarter. */
async function viaConfig(res, config, raw) {
  const base = config.baseUrl.replace(/\/+$/, '');
  const headers = {
    'Content-Type': 'application/json',
    'Content-Length': Buffer.byteLength(raw),
    Authorization: `Bearer ${config.apiKey}`,
  };
  if (config.chatId) headers['X-Chat-Id'] = String(config.chatId);
  if (config.userId) headers['X-User-Id'] = String(config.userId);
  if (config.token) headers['X-Token'] = String(config.token);
  const upstream = await fetch(`${base}/chat/completions`, { method: 'POST', headers, body: raw });
  const passHeaders = {};
  upstream.headers.forEach((value, name) => {
    if (!['content-length', 'transfer-encoding', 'connection'].includes(name.toLowerCase())) {
      passHeaders[name] = value;
    }
  });
  if (!res.headersSent) res.writeHead(upstream.status, passHeaders);
  if (upstream.body) {
    const reader = upstream.body.getReader();
    try {
      for (;;) {
        const { done, value } = await reader.read();
        if (done) break;
        res.write(Buffer.from(value));
      }
    } catch {
      // the client went away mid-stream; nothing to answer with
    }
  }
  res.end();
}

async function forward(req, res) {
  const raw = await readBody(req);
  let parsed = null;
  try {
    parsed = JSON.parse(raw.toString('utf8'));
  } catch {
    // not JSON: the fallback forwards it as it is, and the endpoint says what's wrong
  }
  try {
    if (parsed && (await viaSdk(res, parsed))) return;
  } catch (error) {
    sendJson(res, 502, { error: { message: `The Z.ai bridge couldn't serve the model: ${error && error.message}`, type: 'bridge_error' } });
    return;
  }
  const config = loadConfig();
  if (!config) {
    sendJson(res, 503, {
      error: {
        message:
          'The local Z.ai bridge found neither z-ai-web-dev-sdk nor a .z-ai-config on this machine (looked in the working folder, the home folder and /etc). Save a Z.ai API key for ZCode in Settings → AI instead, or install the SDK.',
        type: 'bridge_error',
      },
    });
    return;
  }
  try {
    await viaConfig(res, config, raw);
  } catch (error) {
    sendJson(res, 502, { error: { message: `The Z.ai bridge couldn't reach the model: ${error && error.message}`, type: 'bridge_error' } });
  }
}

const server = http.createServer((req, res) => {
  if (req.method === 'GET' && req.url.startsWith('/health')) {
    sendJson(res, 200, { ok: true, sdk: true });
    return;
  }
  if (req.method === 'POST' && req.url.startsWith('/llm/v1/chat/completions')) {
    forward(req, res).catch((error) => {
      sendJson(res, 502, { error: { message: `The Z.ai bridge failed: ${error && error.message}`, type: 'bridge_error' } });
    });
    return;
  }
  sendJson(res, 404, { error: { message: `No route for ${req.method} ${req.url}`, type: 'bridge_error' } });
});

server.listen(PORT, '127.0.0.1', () => {
  process.stderr.write(`zai-bridge listening on 127.0.0.1:${PORT}\n`);
});

process.on('SIGTERM', () => process.exit(0));
process.on('SIGINT', () => process.exit(0));
