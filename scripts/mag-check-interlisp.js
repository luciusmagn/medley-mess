#!/usr/bin/env node

const fs = require("fs");

const paths = process.argv.slice(2);
if (paths.length === 0) {
  console.error("usage: mag-check-interlisp.js FILE...");
  process.exit(2);
}

function checkFile(path) {
  const text = fs.readFileSync(path, "utf8");
  const lines = text.split(/\n/);
  const stack = [];
  const diagnostics = [];
  const functions = [];
  let inString = false;
  let stringStart = null;
  let currentFn = null;
  let minDepth = 0;
  let maxDepth = 0;

  function add(line, col, kind, message) {
    diagnostics.push({ line, col, kind, message });
  }

  function openerFor(close) {
    return close === ")" ? "(" : "[";
  }

  function closerFor(open) {
    return open === "(" ? ")" : "]";
  }

  function leadingSpaces(s) {
    const m = s.match(/^\s*/);
    return m ? m[0].length : 0;
  }

  for (let li = 0; li < lines.length; li++) {
    const lineNo = li + 1;
    const line = lines[li];
    const beforeDepth = stack.length;
    const trimmed = line.trim();
    const fnMatch = line.match(/^(\((MAG-[^\s\]\)]+))/);
    const nextTrimmed = li + 1 < lines.length ? lines[li + 1].trim() : "";
    const defineqMatch = line.match(/^(\(DEFINEQ\b)/);

    if (defineqMatch && beforeDepth !== 0) {
      add(lineNo, 1, "defineq-depth",
          `DEFINEQ starts at depth ${beforeDepth}, expected 0`);
    }

    if (fnMatch && nextTrimmed.startsWith("[LAMBDA")) {
      const indent = 0;
      const name = fnMatch[2];
      if (beforeDepth !== 1) {
        add(lineNo, 1, "function-depth",
            `${name} starts at depth ${beforeDepth}, expected DEFINEQ depth 1`);
      }
      if (indent !== 0) {
        add(lineNo, 1, "function-indent",
            `${name} starts at indent ${indent}, expected 0`);
      }
      currentFn = { name, startLine: lineNo, startDepth: beforeDepth };
    }

    for (let ci = 0; ci < line.length; ci++) {
      const ch = line[ci];

      if (inString) {
        if (ch === "\\" && ci + 1 < line.length) {
          ci++;
        } else if (ch === "\"") {
          inString = false;
          stringStart = null;
        }
        continue;
      }

      if (ch === "\"") {
        inString = true;
        stringStart = { line: lineNo, col: ci + 1 };
        continue;
      }

      if (ch === "(" || ch === "[") {
        stack.push({ ch, line: lineNo, col: ci + 1, text: line });
        if (stack.length > maxDepth) maxDepth = stack.length;
        continue;
      }

      if (ch === ")" || ch === "]") {
        if (stack.length === 0) {
          add(lineNo, ci + 1, "underflow", `extra ${ch} with empty stack`);
          minDepth = Math.min(minDepth, -1);
          continue;
        }
        const open = stack[stack.length - 1];
        const want = openerFor(ch);
        if (open.ch !== want) {
          add(lineNo, ci + 1, "mismatch",
              `${ch} closes ${open.ch} from ${open.line}:${open.col}; expected ${closerFor(open.ch)}`);
        }
        stack.pop();
      }
    }

    const afterDepth = stack.length;
    if (afterDepth < minDepth) minDepth = afterDepth;

    if (inString) {
      add(lineNo, line.length + 1, "multiline-string",
          `string from ${stringStart.line}:${stringStart.col} continues past end of physical line`);
    }

    if (currentFn && lineNo >= currentFn.startLine && afterDepth === 1) {
      const endIndent = leadingSpaces(line);
      const splitClose = /^[)\]]+$/.test(trimmed);
      functions.push({
        name: currentFn.name,
        startLine: currentFn.startLine,
        endLine: lineNo,
        endIndent,
        splitClose,
        endText: trimmed,
      });
      if (splitClose) {
        add(lineNo, endIndent + 1, "split-close",
            `${currentFn.name} closes on a standalone ${trimmed} line`);
      }
      currentFn = null;
    }
  }

  for (const open of stack) {
    add(open.line, open.col, "unclosed",
        `${open.ch} opened here is never closed; expected ${closerFor(open.ch)}`);
  }

  if (inString) {
    add(stringStart.line, stringStart.col, "unclosed-string",
        "string opened here is never closed before EOF");
  }

  let lastNonEmpty = null;
  for (let i = lines.length - 1; i >= 0; i--) {
    if (lines[i].trim() !== "") {
      lastNonEmpty = { line: i + 1, text: lines[i].trim() };
      break;
    }
  }
  if (!lastNonEmpty || lastNonEmpty.text !== "STOP") {
    add(lastNonEmpty ? lastNonEmpty.line : lines.length, 1, "missing-stop",
        "Interlisp source should end with a STOP sentinel to avoid EOF while loading");
  }

  const byLine = diagnostics.sort((a, b) => a.line - b.line || a.col - b.col);
  for (const d of byLine) {
    console.log(`${path}:${d.line}:${d.col}: ${d.kind}: ${d.message}`);
  }

  console.log(`${path}: summary: diagnostics=${diagnostics.length} functions=${functions.length} final-depth=${stack.length} min-depth=${minDepth} max-depth=${maxDepth}`);
  return diagnostics.length;
}

let total = 0;
for (const path of paths) total += checkFile(path);
if (total) process.exit(1);
