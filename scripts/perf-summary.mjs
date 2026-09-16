#!/usr/bin/env node
// Summarise probe samples against the issue #88 budgets.
//
//   node scripts/perf-summary.mjs <samples.jsonl>
//
// Prints every sample with its count, median, p95 (nearest rank), maximum, and budget verdict.
// Exit 0 when every measured budget passes, 1 when one fails or has no samples.

import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

export const BUDGETS = {
  startup: { median: 3000, max: 5000 },
  'editor-key': { p95: 50, max: 100 },
  'graph-render': { median: 2000, max: 4000 },
  'semantic-search': { p95: 2000 },
  'exact-scan': { p95: 250 },
};

function rank(sorted, fraction) {
  return sorted[Math.max(0, Math.ceil(fraction * sorted.length) - 1)];
}

export function stats(values) {
  const sorted = [...values].sort((a, b) => a - b);
  const middle = Math.floor(sorted.length / 2);
  const median = sorted.length % 2 ? sorted[middle] : (sorted[middle - 1] + sorted[middle]) / 2;
  return { count: sorted.length, median, p95: rank(sorted, 0.95), max: sorted.at(-1) };
}

export function collect(records) {
  const samples = Object.fromEntries(Object.keys(BUDGETS).map((kind) => [kind, []]));
  let launch = null;
  for (const record of records) {
    if (record.kind === 'launch') launch = record;
    else if (record.kind === 'startup-ready' && launch) {
      // A mark without the sidebar, note list, and an editable editor is not a usable start.
      const usable = record.sidebar && record.noteList && record.editorEditable;
      samples.startup.push(usable ? record.epochMs - launch.epochMs : Infinity);
      launch = null;
    } else if (record.kind === 'semantic-backend') samples['exact-scan'].push(record.exactScanMs);
    else if (samples[record.kind]) samples[record.kind].push(record.ms);
  }
  return samples;
}

export function summarise(records) {
  return Object.entries(collect(records)).map(([kind, values]) => {
    if (values.length === 0) return { kind, pass: false, missing: true };
    const result = { kind, samples: values, ...stats(values) };
    result.pass = Object.entries(BUDGETS[kind]).every(([measure, limit]) => result[measure] <= limit);
    return result;
  });
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const [log] = process.argv.slice(2);
  if (!log) {
    console.error('usage: perf-summary.mjs <samples.jsonl>');
    process.exit(2);
  }
  const records = readFileSync(log, 'utf8').split('\n').filter(Boolean).map((line) => JSON.parse(line));
  const rows = summarise(records);
  console.log(JSON.stringify(rows, null, 2));
  process.exit(rows.every((row) => row.pass) ? 0 : 1);
}
