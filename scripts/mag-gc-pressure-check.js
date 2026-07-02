#!/usr/bin/env node
'use strict';

const { execFileSync } = require('child_process');
const http = require('http');

const HTTP_HOST = process.env.MAG_MEDLEY_MCP_HOST || '127.0.0.1';
const HTTP_PORT = Number(process.env.MAG_MEDLEY_MCP_PORT || 8765);
const HTTP_PATH = process.env.MAG_MEDLEY_MCP_PATH || '/mcp';
const SHELL_CYCLES = Number(process.env.MAG_GC_PRESSURE_SHELL_CYCLES || 3);
const TELEGRAM_REQUESTS = Number(process.env.MAG_GC_PRESSURE_TELEGRAM_REQUESTS || 200);
const SHELL_SETTLE_MS = Number(process.env.MAG_GC_PRESSURE_SHELL_SETTLE_MS || 5000);
const EXTERNAL_SCAN = process.argv.includes('--external-scan');

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function requestMedley(command, timeoutMs = 10000) {
  return new Promise((resolve, reject) => {
    const body = JSON.stringify({
      jsonrpc: '2.0',
      id: 1,
      method: 'tools/call',
      params: {
        name: 'medley_request',
        arguments: { command, timeout_ms: timeoutMs },
      },
    });
    const req = http.request({
      host: HTTP_HOST,
      port: HTTP_PORT,
      path: HTTP_PATH,
      method: 'POST',
      headers: {
        'content-type': 'application/json',
        'content-length': Buffer.byteLength(body),
      },
    }, (res) => {
      let data = '';
      res.on('data', (chunk) => { data += chunk; });
      res.on('end', () => {
        try {
          const parsed = JSON.parse(data);
          resolve(parsed.result?.content?.[0]?.text || parsed.error?.message || data);
        } catch {
          resolve(data);
        }
      });
    });
    req.on('error', reject);
    req.end(body);
  });
}

function reportValue(report, key) {
  const escaped = String(key).replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  const match = String(report).match(new RegExp(`^${escaped}=(.*)$`, 'm'));
  return match ? match[1].trim() : '?';
}

function lines(text) {
  return String(text).split(/\r?\n|\r/);
}

async function gcReport(label) {
  const gc = await requestMedley('gc-report');
  const line = [
    `@@ ${label}`,
    `hi=${reportValue(gc, 'htcoll-hi-links')}`,
    `live=${reportValue(gc, 'htcoll-live-links')}`,
    `free=${reportValue(gc, 'htcoll-free-links')}`,
    `disabled=${reportValue(gc, 'gc-disabled')}`,
  ].join(' ');
  console.log(line);
  return line;
}

async function runShellPressure() {
  for (let i = 1; i <= SHELL_CYCLES; i++) {
    console.log(`@@ shell-cycle ${i} start`);
    console.log(lines(await requestMedley('shell-load-test'))[0]);
    await sleep(SHELL_SETTLE_MS);
    console.log(lines(await requestMedley('shell-state')).slice(0, 4).join(' | '));
    console.log(lines(await requestMedley('close-shell'))[0]);
    await sleep(1000);
    await gcReport(`after-shell-${i}`);
  }
}

async function runTelegramPressure() {
  const commands = ['telegram-status', 'telegram-auth', 'telegram-doctor', 'telegram-chats'];
  for (let i = 1; i <= TELEGRAM_REQUESTS; i++) {
    const command = commands[i % commands.length];
    const result = await requestMedley(command);
    if (i % 50 === 0 || i === TELEGRAM_REQUESTS) {
      console.log(`@@ telegram-${i} ${command} first=${JSON.stringify(lines(result)[0])}`);
    }
  }
  await gcReport('after-telegram');
}

async function main() {
  console.log(`endpoint=http://${HTTP_HOST}:${HTTP_PORT}${HTTP_PATH}`);
  console.log(`shell-cycles=${SHELL_CYCLES} telegram-requests=${TELEGRAM_REQUESTS}`);
  await gcReport('baseline');
  await runShellPressure();
  await runTelegramPressure();
  if (EXTERNAL_SCAN) {
    console.log('@@ external-scan');
    process.stdout.write(execFileSync(
      '/home/mag/src/medley/scripts/mag-gc-scan-live.py',
      ['--top', '12', '--atom-names', '8'],
      { encoding: 'utf8', maxBuffer: 1024 * 1024 }
    ));
  }
}

main().catch((err) => {
  console.error(err.stack || err);
  process.exit(1);
});
