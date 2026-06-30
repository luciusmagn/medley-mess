#!/usr/bin/env node
'use strict';

const fs = require('fs');
const { execFileSync } = require('child_process');

const MEDLEY_DIR = process.env.MEDLEYDIR || '/home/mag/src/medley';
const MAIKO_DIR = process.env.MAIKODIR || '/home/mag/src/maiko';
const LOG_PATH = process.env.MAG_MEDLEY_LOG || '/tmp/medley-interlisp.log';
const DEBUG_PATH = process.env.MAG_MEDLEY_DEBUG_REPORT || '/tmp/medley-mag-debug.txt';
const REQUEST_PATH = process.env.MAG_MEDLEY_REQUEST || '/tmp/medley-mag-request';
const RESPONSE_PATH = process.env.MAG_MEDLEY_RESPONSE || '/tmp/medley-mag-response';

const ALLOWED_REQUESTS = new Set([
  'ping',
  'debug-report',
  'config-report',
  'performance-report',
  'status-report',
  'process-status',
  'native-jobs',
  'goal-status',
  'write-debug-report',
  'battery',
  'who-line-battery',
  'reload-mag',
  'restart-rpc',
  'open-shell',
  'close-shell',
  'shell-self-test',
  'shell-key-probe',
  'shell-state',
  'shell-render-stats',
  'shell-reset-render-stats',
  'shell-box-test',
  'shell-reset-native-stats',
  'shell-reset-all-native-stats',
  'mag-self-test',
  'open-gopher',
  'close-gopher',
  'keys-test',
  'key-encode-test',
  'gopher-keys',
  'gopher-state',
  'gopher-draw-stats',
  'gopher-reset-draw-stats',
  'gopher-test-page',
  'gopher-self-test',
  'gopher-viewport-status',
  'gopher-label-status',
  'gopher-type-status',
  'gopher-key-up',
  'gopher-key-down',
  'gopher-key-left',
  'gopher-key-right',
  'keys-help',
]);

function text(content) {
  return { content: [{ type: 'text', text: String(content) }] };
}

function safeRead(path, fallback) {
  try {
    return fs.readFileSync(path, 'utf8');
  } catch (err) {
    return fallback ?? `${path}: ${err.message}`;
  }
}

function run(command, args, opts = {}) {
  try {
    return execFileSync(command, args, {
      encoding: 'utf8',
      maxBuffer: 1024 * 1024,
      ...opts,
    });
  } catch (err) {
    const out = `${err.stdout || ''}${err.stderr || ''}`.trim();
    return out || `${command} failed: ${err.message}`;
  }
}

function tailString(s, lines) {
  const parts = s.split(/\n/);
  return parts.slice(Math.max(0, parts.length - lines)).join('\n');
}

function sleepMs(ms) {
  Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, ms);
}

function safeUnlink(path) {
  try {
    fs.unlinkSync(path);
  } catch (err) {
    if (err.code !== 'ENOENT') throw err;
  }
}

function writeAtomic(path, content) {
  const tmp = `${path}.${process.pid}.${Date.now()}.tmp`;
  fs.writeFileSync(tmp, content, { encoding: 'utf8', mode: 0o600 });
  fs.renameSync(tmp, path);
}

function readStableResponse(path, deadline) {
  let first;
  let second;

  try {
    first = fs.statSync(path);
  } catch {
    return null;
  }

  if (first.size === 0) return null;
  if (Date.now() + 25 >= deadline) return safeRead(path, '');

  sleepMs(25);

  try {
    second = fs.statSync(path);
  } catch {
    return null;
  }

  if (first.size !== second.size || first.mtimeMs !== second.mtimeMs) return null;
  return safeRead(path, '');
}

function requestMedley(command, timeoutMs) {
  if (!ALLOWED_REQUESTS.has(command)) {
    throw new Error(`unsupported request: ${command}`);
  }

  safeUnlink(RESPONSE_PATH);
  writeAtomic(REQUEST_PATH, `${command}\n`);

  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (fs.existsSync(RESPONSE_PATH)) {
      const response = readStableResponse(RESPONSE_PATH, deadline);
      if (response === null) {
        sleepMs(50);
        continue;
      }
      safeUnlink(RESPONSE_PATH);
      return response;
    }
    sleepMs(100);
  }

  throw new Error(`timed out waiting for Medley response to ${command}`);
}

const tools = [
  {
    name: 'medley_processes',
    description: 'Show current run-medley, lde, and ldex processes.',
    inputSchema: { type: 'object', properties: {}, additionalProperties: false },
  },
  {
    name: 'medley_log_tail',
    description: 'Tail the Medley launcher log.',
    inputSchema: {
      type: 'object',
      properties: { lines: { type: 'integer', minimum: 1, maximum: 500 } },
      additionalProperties: false,
    },
  },
  {
    name: 'medley_debug_report_file',
    description: 'Read the last report written by MAG-DEBUG-WRITE-REPORT inside Medley.',
    inputSchema: { type: 'object', properties: {}, additionalProperties: false },
  },
  {
    name: 'medley_worktree_status',
    description: 'Show git status for local Medley and Maiko trees.',
    inputSchema: { type: 'object', properties: {}, additionalProperties: false },
  },
  {
    name: 'medley_request',
    description: 'Send a limited safe request to the running Medley debug RPC poller.',
    inputSchema: {
      type: 'object',
      properties: {
        command: {
          type: 'string',
          enum: Array.from(ALLOWED_REQUESTS),
        },
        timeout_ms: {
          type: 'integer',
          minimum: 500,
          maximum: 10000,
        },
      },
      required: ['command'],
      additionalProperties: false,
    },
  },
];

function callTool(name, args = {}) {
  switch (name) {
    case 'medley_processes':
      return text(run('ps', ['-eo', 'pid,ppid,stat,comm,args'])
        .split('\n')
        .filter((line) => /run-medley|ldex|lde .*Medley/.test(line))
        .join('\n') || 'No Medley processes found.');

    case 'medley_log_tail': {
      const lines = Number.isInteger(args.lines) ? args.lines : 120;
      return text(tailString(safeRead(LOG_PATH, ''), lines));
    }

    case 'medley_debug_report_file': {
      if (!fs.existsSync(DEBUG_PATH)) {
        return text(`${DEBUG_PATH} does not exist yet. In Medley, run (MAG-DEBUG-WRITE-REPORT NIL) or use the Mag Debug menu item after command 38 is loaded.`);
      }
      return text(safeRead(DEBUG_PATH, ''));
    }

    case 'medley_worktree_status': {
      const medley = run('git', ['-C', MEDLEY_DIR, 'status', '--short']);
      const maiko = run('git', ['-C', MAIKO_DIR, 'status', '--short']);
      return text(`medley:\n${medley || '(clean)'}\nmaiko:\n${maiko || '(clean)'}`);
    }

    case 'medley_request': {
      const command = String(args.command || '');
      const timeoutMs = Number.isInteger(args.timeout_ms) ? args.timeout_ms : 5000;
      return text(requestMedley(command, timeoutMs));
    }

    default:
      throw new Error(`unknown tool: ${name}`);
  }
}

function send(msg) {
  const body = JSON.stringify(msg);
  process.stdout.write(`Content-Length: ${Buffer.byteLength(body, 'utf8')}\r\n\r\n${body}`);
}

function respond(id, result) {
  send({ jsonrpc: '2.0', id, result });
}

function respondError(id, code, message) {
  send({ jsonrpc: '2.0', id, error: { code, message } });
}

function handle(msg) {
  if (!msg || typeof msg !== 'object') return;
  const { id, method, params } = msg;
  try {
    switch (method) {
      case 'initialize':
        respond(id, {
          protocolVersion: params?.protocolVersion || '2024-11-05',
          capabilities: { tools: {} },
          serverInfo: { name: 'mag-medley', version: '0.1.0' },
        });
        break;
      case 'notifications/initialized':
        break;
      case 'tools/list':
        respond(id, { tools });
        break;
      case 'tools/call':
        respond(id, callTool(params?.name, params?.arguments || {}));
        break;
      case 'ping':
        respond(id, {});
        break;
      default:
        if (id !== undefined) respondError(id, -32601, `method not found: ${method}`);
    }
  } catch (err) {
    if (id !== undefined) respondError(id, -32000, err.message || String(err));
  }
}

let buffer = Buffer.alloc(0);

function parseMessages() {
  while (buffer.length) {
    const textBuf = buffer.toString('utf8');
    if (textBuf.startsWith('Content-Length:')) {
      const headerEnd = textBuf.indexOf('\r\n\r\n');
      if (headerEnd < 0) return;
      const header = textBuf.slice(0, headerEnd);
      const m = header.match(/Content-Length:\s*(\d+)/i);
      if (!m) {
        buffer = Buffer.alloc(0);
        return;
      }
      const length = Number(m[1]);
      const bodyStart = Buffer.byteLength(textBuf.slice(0, headerEnd + 4), 'utf8');
      if (buffer.length < bodyStart + length) return;
      const body = buffer.slice(bodyStart, bodyStart + length).toString('utf8');
      buffer = buffer.slice(bodyStart + length);
      handle(JSON.parse(body));
      continue;
    }

    const newline = textBuf.indexOf('\n');
    if (newline < 0) return;
    const line = textBuf.slice(0, newline).trim();
    buffer = buffer.slice(Buffer.byteLength(textBuf.slice(0, newline + 1), 'utf8'));
    if (line) handle(JSON.parse(line));
  }
}

process.stdin.on('data', (chunk) => {
  buffer = Buffer.concat([buffer, chunk]);
  parseMessages();
});

process.stdin.on('end', () => process.exit(0));
