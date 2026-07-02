#!/usr/bin/env node
'use strict';

const fs = require('fs');
const net = require('net');
const path = require('path');
const zlib = require('zlib');
const { spawnSync } = require('child_process');

const MEDLEY_DIR = process.env.MEDLEYDIR || '/home/mag/src/medley';
const REQUEST_PATH = process.env.MAG_MEDLEY_REQUEST || '/tmp/medley-mag-request';
const RESPONSE_PATH = process.env.MAG_MEDLEY_RESPONSE || '/tmp/medley-mag-response';
const SCREENSHOT_PATH = process.env.MAG_MEDLEY_SCREENSHOT || '/tmp/medley-mag-screenshot.ppm';
const MCP_SOCKET = process.env.MAG_MEDLEY_MCP_SOCKET || '/tmp/medley-mag-mcp.sock';
const DEFAULT_MAX_WIDTH = 880;
const DEFAULT_MAX_HEIGHT = 650;
const DEFAULT_THRESHOLD = 128;
const DEFAULT_BITS_PER_PIXEL = 1;

function usage() {
  console.log(`Usage:
  mag-slides-export.js convert-image <source> [dest.magbitmap] [--max-width N] [--max-height N] [--threshold N] [--dither none|ordered] [--flip-y]
  mag-slides-export.js prepare <deck.mag>
  mag-slides-export.js export <deck.mag> [output.pdf] [--out-dir DIR]

Image conversion writes Medley's native READBITMAP text payload.
Default image conversion is a 1bpp ordered-dithered bitmap, matching the
monochrome image objects Medley can display directly. 4bpp/8bpp payloads remain
available with --bits for experiments or color-capable runtimes.
PDF export renders slides through the running Medley instance and screenshots Maiko's DisplayRegion.`);
}

function die(message) {
  console.error(message);
  process.exit(1);
}

function parseOptions(argv) {
  const opts = {
    maxWidth: DEFAULT_MAX_WIDTH,
    maxHeight: DEFAULT_MAX_HEIGHT,
    threshold: DEFAULT_THRESHOLD,
    bits: DEFAULT_BITS_PER_PIXEL,
    dither: 'ordered',
    flipY: false,
    outDir: null,
  };
  const positional = [];
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    const next = () => {
      i += 1;
      if (i >= argv.length) die(`missing value for ${arg}`);
      return argv[i];
    };
    if (arg === '--max-width') opts.maxWidth = Number(next());
    else if (arg === '--max-height') opts.maxHeight = Number(next());
    else if (arg === '--threshold') opts.threshold = Number(next());
    else if (arg === '--bits' || arg === '--bpp') opts.bits = Number(next());
    else if (arg === '--dither') opts.dither = next();
    else if (arg === '--flip-y') opts.flipY = true;
    else if (arg === '--no-flip-y') opts.flipY = false;
    else if (arg === '--out-dir') opts.outDir = next();
    else if (arg === '--help' || arg === '-h') { usage(); process.exit(0); }
    else positional.push(arg);
  }
  if (!Number.isFinite(opts.maxWidth) || opts.maxWidth < 1) die('invalid --max-width');
  if (!Number.isFinite(opts.maxHeight) || opts.maxHeight < 1) die('invalid --max-height');
  if (!Number.isFinite(opts.threshold) || opts.threshold < 0 || opts.threshold > 255) die('invalid --threshold');
  if (![1, 4, 8].includes(opts.bits)) die('invalid --bits; use 1, 4, or 8');
  if (!['none', 'ordered'].includes(opts.dither)) die('invalid --dither');
  return { opts, positional };
}

function defaultBitmapPath(source) {
  if (/\.(magbitmap|bitmap)$/i.test(source)) return source;
  return `${source}.magbitmap`;
}

function ffmpegToPgm(source, opts) {
  const vf = `scale=${Math.trunc(opts.maxWidth)}:${Math.trunc(opts.maxHeight)}:force_original_aspect_ratio=decrease:flags=lanczos,format=gray`;
  const result = spawnSync('ffmpeg', [
    '-v', 'error',
    '-i', source,
    '-vf', vf,
    '-frames:v', '1',
    '-f', 'image2pipe',
    '-vcodec', 'pgm',
    '-',
  ], { encoding: null, maxBuffer: 256 * 1024 * 1024 });
  if (result.status !== 0) {
    const err = result.stderr ? result.stderr.toString('utf8') : '';
    throw new Error(`ffmpeg failed for ${source}: ${err.trim() || `exit ${result.status}`}`);
  }
  return result.stdout;
}

function readToken(buf, state) {
  while (state.i < buf.length) {
    const c = buf[state.i];
    if (c === 35) {
      while (state.i < buf.length && buf[state.i] !== 10 && buf[state.i] !== 13) state.i += 1;
    } else if (c === 9 || c === 10 || c === 13 || c === 32) {
      state.i += 1;
    } else {
      break;
    }
  }
  const start = state.i;
  while (state.i < buf.length) {
    const c = buf[state.i];
    if (c === 9 || c === 10 || c === 13 || c === 32 || c === 35) break;
    state.i += 1;
  }
  if (start === state.i) throw new Error('unexpected end of image header');
  return buf.slice(start, state.i).toString('ascii');
}

function parsePnm(buf) {
  const state = { i: 0 };
  const magic = readToken(buf, state);
  const width = Number(readToken(buf, state));
  const height = Number(readToken(buf, state));
  const max = Number(readToken(buf, state));
  if (!Number.isInteger(width) || !Number.isInteger(height) || width <= 0 || height <= 0) {
    throw new Error('bad PNM dimensions');
  }
  if (max !== 255) throw new Error(`unsupported PNM max value ${max}`);
  if (state.i >= buf.length || ![9, 10, 13, 32].includes(buf[state.i])) {
    throw new Error('missing PNM raster separator');
  }
  state.i += 1;
  if (magic === 'P5') {
    const gray = buf.slice(state.i, state.i + width * height);
    if (gray.length !== width * height) throw new Error('truncated PGM data');
    return { magic, width, height, gray };
  }
  if (magic === 'P6') {
    const rgb = buf.slice(state.i, state.i + width * height * 3);
    if (rgb.length !== width * height * 3) throw new Error('truncated PPM data');
    return { magic, width, height, rgb };
  }
  throw new Error(`unsupported PNM format ${magic}`);
}

const BAYER4 = [
  0, 8, 2, 10,
  12, 4, 14, 6,
  3, 11, 1, 9,
  15, 7, 13, 5,
];

function grayscaleToLevel(grayValue, x, y, opts) {
  if (opts.bits === 1) {
    let threshold = opts.threshold;
    if (opts.dither === 'ordered') {
      threshold += (BAYER4[(y % 4) * 4 + (x % 4)] - 7.5) * 10;
    }
    return grayValue < threshold ? 1 : 0;
  }

  const maxLevel = (1 << opts.bits) - 1;
  const level = ((255 - grayValue) / 255) * maxLevel;
  return Math.max(0, Math.min(maxLevel, Math.round(level)));
}

function medleyRowChars(gray, width, sourceY, opts) {
  const chars = [];
  const bitsPerRow = Math.ceil((width * opts.bits) / 16) * 16;
  let nibble = 0;
  let nibbleBits = 0;

  function pushBit(bit) {
    nibble = (nibble << 1) | (bit ? 1 : 0);
    nibbleBits += 1;
    if (nibbleBits === 4) {
      chars.push(String.fromCharCode(64 + nibble));
      nibble = 0;
      nibbleBits = 0;
    }
  }

  for (let x = 0; x < width; x += 1) {
    const level = grayscaleToLevel(gray[sourceY * width + x], x, sourceY, opts);
    for (let bit = opts.bits - 1; bit >= 0; bit -= 1) {
      pushBit((level >> bit) & 1);
    }
  }

  for (let bit = width * opts.bits; bit < bitsPerRow; bit += 1) {
    pushBit(0);
  }

  return chars.join('');
}

function writeMedleyBitmap(source, dest, opts) {
  const pgm = ffmpegToPgm(source, opts);
  const { width, height, gray } = parsePnm(pgm);
  const rows = [];
  rows.push(`(${width} ${height} ${opts.bits}`);
  for (let y = 0; y < height; y += 1) {
    const sourceY = opts.flipY ? height - 1 - y : y;
    rows.push(`"${medleyRowChars(gray, width, sourceY, opts)}"`);
  }
  rows.push(')');
  fs.mkdirSync(path.dirname(dest), { recursive: true });
  fs.writeFileSync(dest, `${rows.join('\n')}\n`, 'utf8');
  return { source, dest, width, height };
}

function parseDeck(deckPath) {
  const text = fs.readFileSync(deckPath, 'utf8');
  const slides = [];
  let current = [];
  for (const raw of text.split(/\r?\n/)) {
    const line = raw.trimEnd();
    if (line === '---') {
      if (current.some((l) => l.trim() !== '')) slides.push(current);
      current = [];
    } else {
      current.push(line);
    }
  }
  if (current.some((l) => l.trim() !== '')) slides.push(current);
  const images = [];
  for (const slide of slides) {
    for (const line of slide) {
      const m = line.match(/^@image\s+(.+)$/);
      if (m) images.push(resolveDeckPath(deckPath, m[1].trim()));
    }
  }
  return { slides, images };
}

function resolveDeckPath(deckPath, value) {
  if (path.isAbsolute(value)) return value;
  return path.resolve(path.dirname(deckPath), value);
}

function prepareDeck(deckPath, opts) {
  const { images } = parseDeck(deckPath);
  const converted = [];
  for (const image of images) {
    if (/\.(magbitmap|bitmap)$/i.test(image)) {
      converted.push({ source: image, dest: image, skipped: true });
      continue;
    }
    if (!fs.existsSync(image)) {
      throw new Error(`image source does not exist: ${image}`);
    }
    const dest = defaultBitmapPath(image);
    const needs = !fs.existsSync(dest) || fs.statSync(dest).mtimeMs < fs.statSync(image).mtimeMs;
    if (needs) converted.push(writeMedleyBitmap(image, dest, opts));
    else converted.push({ source: image, dest, skipped: true });
  }
  return converted;
}

function writeAtomic(file, text) {
  const tmp = `${file}.${process.pid}.tmp`;
  fs.writeFileSync(tmp, text);
  fs.renameSync(tmp, file);
}

function readStable(file, deadline) {
  while (Date.now() < deadline) {
    if (!fs.existsSync(file)) {
      sleep(50);
      continue;
    }
    const a = fs.statSync(file);
    sleep(25);
    if (!fs.existsSync(file)) continue;
    const b = fs.statSync(file);
    if (a.size === b.size && a.mtimeMs === b.mtimeMs) return fs.readFileSync(file, 'utf8');
  }
  return null;
}

function sleep(ms) {
  Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, ms);
}

function medleyRequestRaw(command, timeoutMs = 10000) {
  try { fs.unlinkSync(RESPONSE_PATH); } catch {}
  writeAtomic(REQUEST_PATH, `${command}\n`);
  const deadline = Date.now() + timeoutMs;
  const response = readStable(RESPONSE_PATH, deadline);
  if (response === null) throw new Error(`timed out waiting for Medley response to ${command}`);
  try { fs.unlinkSync(RESPONSE_PATH); } catch {}
  return response;
}

function daemonRequest(name, args = {}, timeoutMs = 10000) {
  return new Promise((resolve, reject) => {
    const client = net.createConnection(MCP_SOCKET, () => {
      client.write(`${JSON.stringify({ name, args })}\n`);
    });
    let data = '';
    const timer = setTimeout(() => {
      client.destroy();
      reject(new Error(`timed out waiting for ${name}`));
    }, timeoutMs);
    client.on('data', (chunk) => {
      data += chunk.toString('utf8');
      const nl = data.indexOf('\n');
      if (nl >= 0) {
        clearTimeout(timer);
        client.destroy();
        try {
          const response = JSON.parse(data.slice(0, nl));
          if (!response.ok) reject(new Error(response.error || `${name} failed`));
          else resolve(response.result);
        } catch (err) {
          reject(err);
        }
      }
    });
    client.on('error', (err) => {
      clearTimeout(timer);
      reject(err);
    });
  });
}

function parsePpmFile(file) {
  const parsed = parsePnm(fs.readFileSync(file));
  if (parsed.magic === 'P6') return { width: parsed.width, height: parsed.height, rgb: parsed.rgb };
  const rgb = Buffer.alloc(parsed.width * parsed.height * 3);
  for (let i = 0; i < parsed.gray.length; i += 1) {
    rgb[i * 3] = parsed.gray[i];
    rgb[i * 3 + 1] = parsed.gray[i];
    rgb[i * 3 + 2] = parsed.gray[i];
  }
  return { width: parsed.width, height: parsed.height, rgb };
}

function pngChunk(type, payload) {
  const t = Buffer.from(type, 'ascii');
  const len = Buffer.alloc(4);
  len.writeUInt32BE(payload.length, 0);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(Buffer.concat([t, payload])), 0);
  return Buffer.concat([len, t, payload, crc]);
}

let CRC_TABLE = null;
function crc32(buf) {
  if (!CRC_TABLE) {
    CRC_TABLE = new Uint32Array(256);
    for (let n = 0; n < 256; n += 1) {
      let c = n;
      for (let k = 0; k < 8; k += 1) c = (c & 1) ? (0xedb88320 ^ (c >>> 1)) : (c >>> 1);
      CRC_TABLE[n] = c >>> 0;
    }
  }
  let c = 0xffffffff;
  for (const byte of buf) c = CRC_TABLE[(c ^ byte) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}

function writePngFromRgb(file, width, height, rgb) {
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(width, 0);
  ihdr.writeUInt32BE(height, 4);
  ihdr[8] = 8;
  ihdr[9] = 2;
  ihdr[10] = 0;
  ihdr[11] = 0;
  ihdr[12] = 0;
  const rows = [];
  for (let y = 0; y < height; y += 1) {
    rows.push(Buffer.from([0]));
    rows.push(rgb.slice(y * width * 3, (y + 1) * width * 3));
  }
  const png = Buffer.concat([
    Buffer.from('\x89PNG\r\n\x1a\n', 'binary'),
    pngChunk('IHDR', ihdr),
    pngChunk('IDAT', zlib.deflateSync(Buffer.concat(rows))),
    pngChunk('IEND', Buffer.alloc(0)),
  ]);
  fs.writeFileSync(file, png);
}

function pdfString(bufs) {
  const objects = [];
  objects[1] = '<< /Type /Catalog /Pages 2 0 R >>';
  const kids = [];

  for (let i = 0; i < bufs.length; i += 1) {
    const { width, height, rgb } = bufs[i];
    const pageW = width * 72 / 96;
    const pageH = height * 72 / 96;
    const imageId = 3 + i * 3;
    const contentId = imageId + 1;
    const pageId = imageId + 2;
    kids.push(`${pageId} 0 R`);

    const imageData = zlib.deflateSync(rgb);
    objects[imageId] = Buffer.concat([
      Buffer.from(`<< /Type /XObject /Subtype /Image /Width ${width} /Height ${height} /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /FlateDecode /Length ${imageData.length} >>\nstream\n`, 'binary'),
      imageData,
      Buffer.from('\nendstream', 'binary'),
    ]);

    const content = Buffer.from(`q\n${pageW.toFixed(4)} 0 0 ${pageH.toFixed(4)} 0 0 cm\n/Im${i + 1} Do\nQ\n`, 'ascii');
    objects[contentId] = Buffer.concat([
      Buffer.from(`<< /Length ${content.length} >>\nstream\n`, 'ascii'),
      content,
      Buffer.from('endstream', 'ascii'),
    ]);

    objects[pageId] = `<< /Type /Page /Parent 2 0 R /MediaBox [0 0 ${pageW.toFixed(4)} ${pageH.toFixed(4)}] /Resources << /XObject << /Im${i + 1} ${imageId} 0 R >> >> /Contents ${contentId} 0 R >>`;
  }

  objects[2] = `<< /Type /Pages /Count ${kids.length} /Kids [${kids.join(' ')}] >>`;
  const maxId = 2 + bufs.length * 3;
  const chunks = [];
  const offsets = [];
  let pos = 0;
  function push(chunk) {
    const b = Buffer.isBuffer(chunk) ? chunk : Buffer.from(String(chunk), 'binary');
    chunks.push(b);
    pos += b.length;
  }

  push('%PDF-1.4\n%\xE2\xE3\xCF\xD3\n');
  for (let id = 1; id <= maxId; id += 1) {
    offsets[id] = pos;
    push(`${id} 0 obj\n`);
    push(objects[id]);
    push('\nendobj\n');
  }
  const xref = pos;
  push(`xref\n0 ${maxId + 1}\n0000000000 65535 f \n`);
  for (let id = 1; id <= maxId; id += 1) push(`${String(offsets[id]).padStart(10, '0')} 00000 n \n`);
  push(`trailer\n<< /Size ${maxId + 1} /Root 1 0 R >>\nstartxref\n${xref}\n%%EOF\n`);
  return Buffer.concat(chunks);
}

function writePdf(file, images) {
  fs.writeFileSync(file, pdfString(images));
}

function shellSafePathForMedleyRequest(p) {
  if (/\r|\n/.test(p)) throw new Error(`path contains newline: ${p}`);
  return p;
}

async function exportDeck(deckPath, pdfPath, opts) {
  const absDeck = path.resolve(deckPath);
  const parsed = parseDeck(absDeck);
  if (parsed.slides.length === 0) throw new Error(`deck has no slides: ${absDeck}`);
  prepareDeck(absDeck, opts);
  const outDir = path.resolve(opts.outDir || `${absDeck}.export`);
  fs.mkdirSync(outDir, { recursive: true });
  const outPdf = path.resolve(pdfPath || `${absDeck}.pdf`);

  medleyRequestRaw(`slides-open ${shellSafePathForMedleyRequest(absDeck)}`, 30000);
  sleep(800);

  const images = [];
  for (let i = 1; i <= parsed.slides.length; i += 1) {
    medleyRequestRaw(`slides-goto ${i} 999`, 30000);
    sleep(350);
    await daemonRequest('medley_screenshot', { timeout_ms: 10000, include_base64: false }, 15000);
    const ppmDest = path.join(outDir, `slide-${String(i).padStart(2, '0')}.ppm`);
    const pngDest = path.join(outDir, `slide-${String(i).padStart(2, '0')}.png`);
    fs.copyFileSync(SCREENSHOT_PATH, ppmDest);
    const image = parsePpmFile(ppmDest);
    writePngFromRgb(pngDest, image.width, image.height, image.rgb);
    images.push(image);
    console.log(`rendered ${pngDest}`);
  }
  writePdf(outPdf, images);
  console.log(`wrote ${outPdf}`);
}

async function main() {
  const [cmd, ...rest] = process.argv.slice(2);
  if (!cmd || cmd === '--help' || cmd === '-h') { usage(); return; }
  const { opts, positional } = parseOptions(rest);
  if (cmd === 'convert-image') {
    const source = positional[0];
    if (!source) die('convert-image requires <source>');
    const dest = positional[1] || defaultBitmapPath(source);
    const result = writeMedleyBitmap(path.resolve(source), path.resolve(dest), opts);
    console.log(`wrote ${result.dest} ${result.width}x${result.height}`);
    return;
  }
  if (cmd === 'prepare') {
    const deck = positional[0];
    if (!deck) die('prepare requires <deck.mag>');
    const converted = prepareDeck(path.resolve(deck), opts);
    for (const item of converted) console.log(`${item.skipped ? 'ok' : 'wrote'} ${item.dest}`);
    return;
  }
  if (cmd === 'export') {
    const deck = positional[0];
    if (!deck) die('export requires <deck.mag>');
    await exportDeck(deck, positional[1], opts);
    return;
  }
  die(`unknown command: ${cmd}`);
}

main().catch((err) => die(err.stack || err.message));
