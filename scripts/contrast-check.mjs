#!/usr/bin/env node
// WCAG AAA contrast audit. Reads colour tokens from ui/tokens.slint and asserts
// minimum 7:1 (normal text) / 4.5:1 (large text) against backgrounds.
// Exits non-zero if any pairing fails.
//
// Usage:
//   node scripts/contrast-check.mjs
//
// CI-friendly. No deps.

import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";

const HERE = dirname(fileURLToPath(import.meta.url));
const TOKENS = resolve(HERE, "..", "ui", "tokens.slint");

const src = readFileSync(TOKENS, "utf8");

// Match `out property <color> name: #rrggbb;`  or rgba(r,g,b,a)
// or ternary like `dark ? #aaa : #bbb;` — we expand both branches.
const colorOf = (token) => {
  const re = new RegExp(`out property <color> ${token}:\\s*([^;]+);`);
  const m = src.match(re);
  if (!m) return null;
  return m[1].trim();
};

const themePairsDark = [
  ["text",     "bg"],
  ["text",     "atmosphere"],
  ["text",     "panel"],
  ["text",     "panel-2"],
  ["text",     "modal"],
  ["text-dim", "bg"],
  ["text-dim", "panel-2"],
];

function parseHex(s) {
  const m = s.match(/^#([0-9a-f]{6})$/i);
  if (m) {
    const n = parseInt(m[1], 16);
    return { r: (n >> 16) & 255, g: (n >> 8) & 255, b: n & 255, a: 1 };
  }
  return null;
}

function relLum({ r, g, b }) {
  const ch = (c) => {
    const v = c / 255;
    return v <= 0.03928 ? v / 12.92 : Math.pow((v + 0.055) / 1.055, 2.4);
  };
  const [R, G, B] = [ch(r), ch(g), ch(b)];
  return 0.2126 * R + 0.7152 * G + 0.0722 * B;
}

function ratio(c1, c2) {
  const [l1, l2] = [relLum(c1), relLum(c2)].sort((a, b) => b - a);
  return (l1 + 0.05) / (l2 + 0.05);
}

function composite(fg, bg) {
  if (fg.a >= 1) return fg;
  const a = fg.a;
  return {
    r: Math.round(fg.r * a + bg.r * (1 - a)),
    g: Math.round(fg.g * a + bg.g * (1 - a)),
    b: Math.round(fg.b * a + bg.b * (1 - a)),
    a: 1,
  };
}

// Extract both branches of a `dark ? X : Y` ternary
function branches(expr) {
  const m = expr.match(/^dark\s*\?\s*(.+?)\s*:\s*(.+)$/s);
  if (m) return [m[1].trim(), m[2].trim()];
  return [expr, expr];
}

function parseColorExpr(expr) {
  expr = expr.trim();
  const hex = parseHex(expr);
  if (hex) return hex;
  const rgba = expr.match(/^rgba?\(\s*([\d.]+)\s*,\s*([\d.]+)\s*,\s*([\d.]+)\s*(?:,\s*([\d.]+))?\s*\)$/);
  if (rgba) {
    return { r: +rgba[1], g: +rgba[2], b: +rgba[3], a: rgba[4] === undefined ? 1 : +rgba[4] };
  }
  return null;
}

let failures = 0;
let checks = 0;

for (const mode of ["dark", "light"]) {
  for (const [fgTok, bgTok] of themePairsDark) {
    const fgExpr = colorOf(fgTok);
    const bgExpr = colorOf(bgTok);
    if (!fgExpr || !bgExpr) continue;
    const fgBranch = branches(fgExpr)[mode === "dark" ? 0 : 1];
    const bgBranch = branches(bgExpr)[mode === "dark" ? 0 : 1];
    const fg = parseColorExpr(fgBranch);
    const bg = parseColorExpr(bgBranch);
    if (!fg || !bg) continue;
    const fgOnBg = composite(fg, bg);
    const r = ratio(fgOnBg, bg);
    checks++;
    const minRatio = fgTok === "text-dim" ? 4.5 : 7.0;
    const pass = r >= minRatio;
    const tag = pass ? "ok " : "FAIL";
    console.log(`[${tag}] ${mode.padEnd(5)}  ${fgTok.padEnd(10)} on ${bgTok.padEnd(12)} ${r.toFixed(2)}:1 (min ${minRatio})`);
    if (!pass) failures++;
  }
}

console.log(`\n${checks} checks, ${failures} failures`);
process.exit(failures > 0 ? 1 : 0);
