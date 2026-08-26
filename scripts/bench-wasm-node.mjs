// Node benchmark for the wasm32-unknown-unknown decode module.
//
// Usage:
//   node scripts/bench-wasm-node.mjs
//
// Expects .refbuild/wb-{scalar,scalar128,simd,simd128}.wasm and the committed
// corpus. Interleaves variants per case, warms up, and reports medians.

import { readFileSync } from 'node:fs';
import { performance } from 'node:perf_hooks';
import { resolve } from 'node:path';

const root = resolve(import.meta.dirname, '..');
const variants = ['scalar', 'scalar128', 'simd', 'simd128'];
const cases = [
  ['music-a-celt-096k-20ms.opus', 2],
  ['speech-silk-012k-20ms.opus', 1],
  ['speech-hybrid-032k-20ms.opus', 1],
  ['speech-silk-012k-dtx-20ms.opus', 1],
];

async function loadVariant(name) {
  const bytes = readFileSync(resolve(root, `.refbuild/wb-${name}.wasm`));
  const { instance } = await WebAssembly.instantiate(bytes, {});
  return instance.exports;
}

function prepare(exp, data, channels) {
  const inPtr = exp.alloc(data.length);
  new Uint8Array(exp.memory.buffer, inPtr, data.length).set(data);
  const outPtr = exp.alloc(64 * 1024 * 1024);
  const out = new Uint8Array(exp.memory.buffer, outPtr, 64 * 1024 * 1024);
  return { inPtr, outPtr, out };
}

function checksum(bytes) {
  let h = 0x811c9dc5;
  const step = 4096;
  for (let i = 0; i < bytes.length; i += step) {
    const end = Math.min(i + step, bytes.length);
    for (let j = i; j < end; j++) {
      h ^= bytes[j];
      h = Math.imul(h, 0x01000193);
    }
  }
  return h >>> 0;
}

const warmups = 3;
const iters = 31;

for (const [file, channels] of cases) {
  const data = new Uint8Array(readFileSync(resolve(root, 'corpus', file)));
  const exps = {};
  for (const v of variants) exps[v] = await loadVariant(v);
  const pre = {};
  for (const v of variants) pre[v] = prepare(exps[v], data, channels);

  const samples = Object.fromEntries(variants.map((v) => [v, []]));
  const order = [];
  for (let round = 0; round < warmups + iters; round++) {
    for (const v of variants) order.push(v);
  }
  // deterministic shuffle
  let seed = 7;
  for (let i = order.length - 1; i > 0; i--) {
    seed = (seed * 1664525 + 1013904223) >>> 0;
    const j = seed % (i + 1);
    [order[i], order[j]] = [order[j], order[i]];
  }

  for (const v of order) {
    const { inPtr, outPtr, out } = pre[v];
    const t0 = performance.now();
    const n = exps[v].bench_decode(inPtr, data.length, channels, outPtr);
    const dt = performance.now() - t0;
    if (Number(n) < 0) throw new Error(`${file} ${v}: decode error ${n}`);
    samples[v].push(dt);
  }
  // discard warmups (order interleaves them); collect all timed runs
  const timed = Object.fromEntries(variants.map((v) => [v, []]));
  const counts = Object.fromEntries(variants.map((v) => [v, 0]));
  for (const v of order) {
    if (counts[v] >= warmups) timed[v].push(samples[v][counts[v]]);
    counts[v]++;
  }
  const med = Object.fromEntries(
    variants.map((v) => [v, timed[v].sort((a, b) => a - b)[Math.floor(timed[v].length / 2)]]),
  );
  const base = med.scalar;
  const row = variants
    .map((v) => `${v}=${med[v].toFixed(2)}ms (${(med[v] / base).toFixed(3)}x)`)
    .join('  ');
  console.log(`${file}  ${row}`);
  const chk = variants.map((v) => checksum(pre[v].out)).join(',');
  console.log(`  checksums: ${chk}`);
}
