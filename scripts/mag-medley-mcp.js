#!/usr/bin/env node
'use strict';

const fs = require('fs');
const http = require('http');
const net = require('net');
const { execFileSync } = require('child_process');

const MEDLEY_DIR = process.env.MEDLEYDIR || '/home/mag/src/medley';
const MAIKO_DIR = process.env.MAIKODIR || '/home/mag/src/maiko';
const LOG_PATH = process.env.MAG_MEDLEY_LOG || '/tmp/medley-interlisp.log';
const DEBUG_PATH = process.env.MAG_MEDLEY_DEBUG_REPORT || '/tmp/medley-mag-debug.txt';
const REQUEST_PATH = process.env.MAG_MEDLEY_REQUEST || '/tmp/medley-mag-request';
const RESPONSE_PATH = process.env.MAG_MEDLEY_RESPONSE || '/tmp/medley-mag-response';
const EVAL_DIR = process.env.MAG_MEDLEY_EVAL_DIR || '/tmp/medley-mag-eval';
const TYPEAHEAD_PATH = process.env.MAG_MEDLEY_TYPEAHEAD || '/tmp/medley-mag-typeahead';
const DAEMON_SOCKET = process.env.MAG_MEDLEY_MCP_SOCKET || '/tmp/medley-mag-mcp.sock';
const HTTP_HOST = process.env.MAG_MEDLEY_MCP_HOST || '127.0.0.1';
const HTTP_PORT = Number(process.env.MAG_MEDLEY_MCP_PORT || 8765);
const HTTP_PATH = process.env.MAG_MEDLEY_MCP_PATH || '/mcp';
const DAEMON_MODE = process.argv.includes('--daemon');
let activeInstanceId = process.env.MAG_MEDLEY_INSTANCE || 'default';

const ALLOWED_REQUESTS = new Set([
  'ping',
  'debug-report',
  'config-report',
  'performance-report',
  'background-report',
  'status-report',
  'process-status',
  'native-jobs',
  'gc-report',
  'goal-status',
  'write-debug-report',
  'battery',
  'who-line-battery',
  'eval-status',
  'eval-reset',
  'reload-mag',
  'restart-rpc',
  'open-shell',
  'shell-load-test',
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

function requestMedley(command, timeoutMs, opts = {}) {
  if (!opts.allowDynamic && !ALLOWED_REQUESTS.has(command)) {
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

function makeEvalId() {
  return `eval-${process.pid}-${Date.now()}-${Math.random().toString(36).slice(2, 10)}`;
}

function ensureEvalDir() {
  fs.mkdirSync(EVAL_DIR, { recursive: true, mode: 0o700 });
  try {
    fs.chmodSync(EVAL_DIR, 0o700);
  } catch {
    // Non-fatal if the filesystem rejects chmod.
  }
}

function evalPath(id, ext) {
  if (!/^[A-Za-z0-9-]{1,80}$/.test(id)) throw new Error(`invalid eval id: ${id}`);
  return `${EVAL_DIR}/${id}${ext}`;
}

function writeEvalInput(id, form) {
  ensureEvalDir();
  const source = String(form || '').trim();
  if (!source) throw new Error('form is required');
  if (Buffer.byteLength(source, 'utf8') > 64 * 1024) {
    throw new Error('form is too large; limit is 64 KiB');
  }

  writeAtomic(evalPath(id, '.lisp'), `${source}\n`);
  safeUnlink(evalPath(id, '.out'));
  return source;
}

function writeEvalTypeahead(id, source) {
  if (!/^[A-Za-z0-9-]{1,80}$/.test(id)) throw new Error(`invalid eval id: ${id}`);
  if (/[\r\n]/.test(source)) {
    throw new Error('form must be one physical line for the native typeahead eval backend');
  }
  writeAtomic(
    TYPEAHEAD_PATH,
    `(IL:MAG-DEBUG-EVAL-CALL-THUNK "${id}" (CL:FUNCTION (CL:LAMBDA () ${source})))\n`
  );
}

function readStableFile(path, deadline) {
  const value = readStableResponse(path, deadline);
  return value === null ? null : value;
}

function waitForEvalResult(id, timeoutMs, startResponse) {
  const outputPath = evalPath(id, '.out');
  const deadline = Date.now() + timeoutMs;

  while (Date.now() < deadline) {
    if (fs.existsSync(outputPath)) {
      const result = readStableFile(outputPath, deadline);
      if (result === null) {
        sleepMs(50);
        continue;
      }
      safeUnlink(evalPath(id, '.lisp'));
      safeUnlink(outputPath);
      return result;
    }
    sleepMs(100);
  }

  return `id=${id}\nstatus=timeout\nmessage=timed out waiting for eval result\nstart-response=${String(startResponse || '').trim()}\n`;
}

function evalMedley(form, timeoutMs) {
  const id = makeEvalId();
  const source = writeEvalInput(id, form);
  writeEvalTypeahead(id, source);

  let startResponse;
  try {
    startResponse = requestMedley(`eval ${id}`, Math.min(timeoutMs, 5000), {
      allowDynamic: true,
    });
  } catch (err) {
    safeUnlink(TYPEAHEAD_PATH);
    throw err;
  }

  if (!String(startResponse).includes(`eval-started id=${id}`)) {
    safeUnlink(evalPath(id, '.lisp'));
    safeUnlink(TYPEAHEAD_PATH);
    throw new Error(`Medley did not start eval ${id}: ${String(startResponse).trim()}`);
  }

  return waitForEvalResult(id, timeoutMs, startResponse);
}

const tools = [
  {
    name: 'medley_processes',
    description: 'Show current run-medley, lde, and ldex processes.',
    inputSchema: { type: 'object', properties: {}, additionalProperties: false },
  },
  {
    name: 'medley_daemon_status',
    description: 'Show whether the persistent Mag Medley MCP daemon is reachable.',
    inputSchema: { type: 'object', properties: {}, additionalProperties: false },
  },
  {
    name: 'medley_instances',
    description: 'List detected Medley/Maiko instances and the selected instance id.',
    inputSchema: { type: 'object', properties: {}, additionalProperties: false },
  },
  {
    name: 'medley_attach',
    description: 'Select a detected Medley instance id for daemon-side diagnostics.',
    inputSchema: {
      type: 'object',
      properties: {
        id: { type: 'string' },
      },
      required: ['id'],
      additionalProperties: false,
    },
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
  {
    name: 'medley_eval',
    description: 'Evaluate one Interlisp form in the running Medley Exec process and return its printed value.',
    inputSchema: {
      type: 'object',
      properties: {
        form: {
          type: 'string',
          description: 'A single physical-line Medley/XCL expression. Prefer explicit CL: or IL: package prefixes.',
          maxLength: 65536,
        },
        timeout_ms: {
          type: 'integer',
          minimum: 500,
          maximum: 30000,
        },
      },
      required: ['form'],
      additionalProperties: false,
    },
  },
  {
    name: 'medley_eval_status',
    description: 'Report whether Medley-side eval dispatch is available.',
    inputSchema: { type: 'object', properties: {}, additionalProperties: false },
  },
  {
    name: 'medley_eval_reset',
    description: 'Report eval reset status. The current backend has no separate worker to kill.',
    inputSchema: { type: 'object', properties: {}, additionalProperties: false },
  },
];

function detectMedleyInstances() {
  return run('ps', ['-eo', 'pid,ppid,stat,comm,args'])
    .split('\n')
    .filter((line) => /\/home\/mag\/src\/medley\/medley|\/ldex |\/lde /.test(line))
    .map((line) => {
      const id = line.match(/ -id ([^ ]+)/)?.[1] || 'unknown';
      const geometry = line.match(/ -(?:g|geometry) ([0-9]+x[0-9]+)/)?.[1]
        || line.match(/ --geometry ([0-9]+x[0-9]+)/)?.[1]
        || '';
      return { id, geometry, line: line.trim() };
    });
}

function daemonRequestSync(name, args = {}, timeoutMs = 1500) {
  const helper = [
    'const net=require("net");',
    `const sock=${JSON.stringify(DAEMON_SOCKET)};`,
    `const req=${JSON.stringify(JSON.stringify({ name, args }) + '\n')};`,
    `const timeout=${Number(timeoutMs)};`,
    'const c=net.createConnection(sock);',
    'let data="";',
    'const t=setTimeout(()=>{console.error("timeout");process.exit(124)},timeout);',
    'c.on("connect",()=>c.write(req));',
    'c.on("data",(chunk)=>{data+=chunk.toString("utf8");const i=data.indexOf("\\n");if(i>=0){clearTimeout(t);console.log(data.slice(0,i));c.end();}});',
    'c.on("error",(err)=>{clearTimeout(t);console.error(err.message);process.exit(1)});',
  ].join('');
  const raw = execFileSync(process.execPath, ['-e', helper], {
    encoding: 'utf8',
    timeout: timeoutMs + 500,
    maxBuffer: 1024 * 1024,
  }).trim();
  const response = JSON.parse(raw);
  if (!response.ok) throw new Error(response.error || 'daemon request failed');
  return response.result;
}

function callToolLocal(name, args = {}) {
  switch (name) {
    case 'medley_daemon_status': {
      if (DAEMON_MODE) {
        return text(`daemon running pid=${process.pid} socket=${DAEMON_SOCKET} http=http://${HTTP_HOST}:${HTTP_PORT}${HTTP_PATH} active-instance=${activeInstanceId}`);
      }
      if (!fs.existsSync(DAEMON_SOCKET)) {
        return text(`daemon not reachable at ${DAEMON_SOCKET}: socket does not exist`);
      }
      try {
        const result = daemonRequestSync('medley_daemon_status', {}, 1000);
        return result;
      } catch (err) {
        return text(`daemon not reachable at ${DAEMON_SOCKET}: ${err.message}`);
      }
    }

    case 'medley_instances': {
      const instances = detectMedleyInstances();
      const lines = instances.map((instance) =>
        `${instance.id === activeInstanceId ? '*' : ' '} id=${instance.id} geometry=${instance.geometry || '?'} ${instance.line}`);
      return text(lines.length
        ? `active-instance=${activeInstanceId}\n${lines.join('\n')}`
        : 'No Medley processes found.');
    }

    case 'medley_attach': {
      const id = String(args.id || '').trim();
      if (!id) throw new Error('id is required');
      activeInstanceId = id;
      return text(`active-instance=${activeInstanceId}`);
    }

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

    case 'medley_eval': {
      const timeoutMs = Number.isInteger(args.timeout_ms) ? args.timeout_ms : 5000;
      return text(evalMedley(args.form, timeoutMs));
    }

    case 'medley_eval_status':
      return text(requestMedley('eval-status', 5000));

    case 'medley_eval_reset':
      return text(requestMedley('eval-reset', 5000));

    default:
      throw new Error(`unknown tool: ${name}`);
  }
}

function callTool(name, args = {}) {
  return callToolLocal(name, args);
}

function dispatch(msg) {
  if (!msg || typeof msg !== 'object') return null;
  const { id, method, params } = msg;

  try {
    switch (method) {
      case 'initialize':
        return {
          jsonrpc: '2.0',
          id,
          result: {
            protocolVersion: params?.protocolVersion || '2024-11-05',
            capabilities: { tools: {} },
            serverInfo: { name: 'mag-medley', version: '0.1.0' },
          },
        };
      case 'notifications/initialized':
        return null;
      case 'tools/list':
        return { jsonrpc: '2.0', id, result: { tools } };
      case 'tools/call':
        return { jsonrpc: '2.0', id, result: callTool(params?.name, params?.arguments || {}) };
      case 'ping':
        return { jsonrpc: '2.0', id, result: {} };
      default:
        if (id !== undefined) {
          return { jsonrpc: '2.0', id, error: { code: -32601, message: `method not found: ${method}` } };
        }
        return null;
    }
  } catch (err) {
    if (id !== undefined) {
      return { jsonrpc: '2.0', id, error: { code: -32000, message: err.message || String(err) } };
    }
    return null;
  }
}

function dispatchPayload(payload) {
  if (Array.isArray(payload)) {
    return payload.map(dispatch).filter(Boolean);
  }
  return dispatch(payload);
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
  const response = dispatch(msg);
  if (response) send(response);
}

function startHttpServer() {
  const server = http.createServer((req, res) => {
    if (req.method === 'GET' && req.url === '/health') {
      res.writeHead(200, { 'content-type': 'text/plain; charset=utf-8' });
      res.end(`ok pid=${process.pid} endpoint=http://${HTTP_HOST}:${HTTP_PORT}${HTTP_PATH}\n`);
      return;
    }

    if (req.url !== HTTP_PATH) {
      res.writeHead(404, { 'content-type': 'text/plain; charset=utf-8' });
      res.end('not found\n');
      return;
    }

    if (req.method !== 'POST') {
      res.writeHead(405, {
        allow: 'POST',
        'content-type': 'text/plain; charset=utf-8',
      });
      res.end('method not allowed\n');
      return;
    }

    let body = '';
    req.setEncoding('utf8');
    req.on('data', (chunk) => {
      body += chunk;
      if (body.length > 1024 * 1024) req.destroy(new Error('request too large'));
    });
    req.on('end', () => {
      try {
        const payload = JSON.parse(body || 'null');
        const response = dispatchPayload(payload);
        if (response === null || (Array.isArray(response) && response.length === 0)) {
          res.writeHead(202, { 'mcp-session-id': activeInstanceId });
          res.end();
          return;
        }

        const responseBody = JSON.stringify(response);
        res.writeHead(200, {
          'content-type': 'application/json',
          'mcp-session-id': activeInstanceId,
        });
        res.end(responseBody);
      } catch (err) {
        const responseBody = JSON.stringify({
          jsonrpc: '2.0',
          id: null,
          error: { code: -32700, message: err.message || String(err) },
        });
        res.writeHead(400, { 'content-type': 'application/json' });
        res.end(responseBody);
      }
    });
  });

  server.listen(HTTP_PORT, HTTP_HOST, () => {
    console.error(`mag-medley MCP HTTP pid=${process.pid} url=http://${HTTP_HOST}:${HTTP_PORT}${HTTP_PATH}`);
  });

  return server;
}

function legacyDaemonPid(statusText) {
  const match = String(statusText || '').match(/pid=(\d+)/);
  return match ? Number(match[1]) : null;
}

function replaceLegacyDaemonIfNeeded() {
  if (!fs.existsSync(DAEMON_SOCKET)) return;

  try {
    const existing = daemonRequestSync('medley_daemon_status', {}, 500);
    const statusText = existing.content?.[0]?.text || '';
    if (statusText.includes(`http://${HTTP_HOST}:${HTTP_PORT}${HTTP_PATH}`)) {
      console.error(statusText);
      process.exit(0);
    }

    const pid = legacyDaemonPid(statusText);
    if (pid && pid !== process.pid) {
      console.error(`replacing legacy mag-medley MCP daemon pid=${pid}`);
      try {
        process.kill(pid, 'SIGTERM');
      } catch {
        // It may have exited after answering.
      }
      sleepMs(250);
    }
  } catch {
    // No live daemon answered; remove a stale socket below.
  }

  safeUnlink(DAEMON_SOCKET);
}

function startDaemon() {
  replaceLegacyDaemonIfNeeded();

  safeUnlink(DAEMON_SOCKET);
  const server = net.createServer((socket) => {
    let data = '';
    socket.on('data', (chunk) => {
      data += chunk.toString('utf8');
      for (;;) {
        const newline = data.indexOf('\n');
        if (newline < 0) break;
        const line = data.slice(0, newline).trim();
        data = data.slice(newline + 1);
        if (!line) continue;
        try {
          const request = JSON.parse(line);
          const result = callToolLocal(request.name, request.args || {});
          socket.write(`${JSON.stringify({ ok: true, result })}\n`);
        } catch (err) {
          socket.write(`${JSON.stringify({ ok: false, error: err.message || String(err) })}\n`);
        }
      }
    });
  });

  server.listen(DAEMON_SOCKET, () => {
    try {
      fs.chmodSync(DAEMON_SOCKET, 0o600);
    } catch {
      // Non-fatal; /tmp ownership still protects the socket on this machine.
    }
    console.error(`mag-medley MCP daemon pid=${process.pid} socket=${DAEMON_SOCKET}`);
  });

  const httpServer = startHttpServer();

  const cleanup = () => {
    server.close(() => process.exit(0));
    httpServer.close();
    safeUnlink(DAEMON_SOCKET);
    setTimeout(() => process.exit(0), 250).unref();
  };
  process.on('SIGINT', cleanup);
  process.on('SIGTERM', cleanup);
  process.on('SIGHUP', () => {
    console.error(`mag-medley MCP daemon ignoring SIGHUP pid=${process.pid}`);
  });
  process.on('uncaughtException', (err) => {
    console.error(`mag-medley MCP daemon uncaughtException: ${err?.stack || err}`);
    process.exit(1);
  });
  process.on('unhandledRejection', (err) => {
    console.error(`mag-medley MCP daemon unhandledRejection: ${err?.stack || err}`);
    process.exit(1);
  });
  process.on('exit', (code) => {
    console.error(`mag-medley MCP daemon exit pid=${process.pid} code=${code}`);
  });
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

if (DAEMON_MODE) {
  startDaemon();
} else {
  process.stdin.on('data', (chunk) => {
    buffer = Buffer.concat([buffer, chunk]);
    parseMessages();
  });

  process.stdin.on('end', () => process.exit(0));
}
