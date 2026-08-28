#!/usr/bin/env node
import fs from 'node:fs';
import path from 'node:path';
import process from 'node:process';
import { fileURLToPath } from 'node:url';

const METRIC_ORDER = [
  'decode_latest_png',
  'ffmpeg_start',
  'ffmpeg_input',
  'ffmpeg_decode',
  'ffmpeg_png',
  'png_grayscale',
  'ncc_fullscreen',
  'ncc_region',
  'template_read',
  'template_preprocess',
  'find_round',
];

const METRIC_ALIASES = new Map([
  ['decode_latest_png_total', 'decode_latest_png'],
  ['decode_png', 'decode_latest_png'],
  ['ffmpeg启动', 'ffmpeg_start'],
  ['ffmpeg_start', 'ffmpeg_start'],
  ['ffmpeg_input', 'ffmpeg_input'],
  ['ffmpeg_decode', 'ffmpeg_decode'],
  ['ffmpeg_png', 'ffmpeg_png'],
  ['png_gray', 'png_grayscale'],
  ['png_grayscale', 'png_grayscale'],
  ['ncc_fullscreen', 'ncc_fullscreen'],
  ['ncc_region', 'ncc_region'],
  ['template_read', 'template_read'],
  ['template_preprocess', 'template_preprocess'],
  ['find_round', 'find_round'],
  ['find轮次', 'find_round'],
]);

function usage(exitCode = 0) {
  const msg = [
    'usage: node tools/perf-stage5b-stats.mjs --input <file|dir> [--input ...] [--json] [--include-resource] [--self-test] [--dry-run]',
    '',
    'Reads JSONL/CSV benchmark samples and prints p50/p95/max for the stage5 B metrics.',
  ].join('\n');
  console[exitCode === 0 ? 'log' : 'error'](msg);
  process.exit(exitCode);
}

function percentiles(values) {
  const sorted = [...values].sort((a, b) => a - b);
  const pick = (p) => sorted[Math.min(sorted.length - 1, Math.floor((sorted.length - 1) * p))];
  return { p50: pick(0.5), p95: pick(0.95), max: sorted[sorted.length - 1] };
}

function toNumber(value) {
  if (value === null || value === undefined || value === '') return null;
  const n = Number(value);
  return Number.isFinite(n) ? n : null;
}

function normalizeMetric(raw) {
  if (!raw) return null;
  const key = String(raw).trim().toLowerCase();
  return METRIC_ALIASES.get(key) || (METRIC_ORDER.includes(key) ? key : null);
}

function parseJsonl(text, source) {
  const rows = [];
  for (const [lineNo, line] of text.split(/\r?\n/).entries()) {
    if (!line.trim()) continue;
    try {
      rows.push({ ...JSON.parse(line), __source: source, __line: lineNo + 1 });
    } catch (err) {
      throw new Error(`${source}:${lineNo + 1} invalid JSONL: ${err.message}`);
    }
  }
  return rows;
}

function parseCsv(text, source) {
  const lines = text.split(/\r?\n/).filter((line) => line.trim().length > 0);
  if (lines.length === 0) return [];
  const headers = splitCsvLine(lines[0]).map((h) => h.trim());
  return lines.slice(1).map((line, idx) => {
    const values = splitCsvLine(line);
    const row = {};
    headers.forEach((h, i) => {
      row[h] = values[i] ?? '';
    });
    row.__source = source;
    row.__line = idx + 2;
    return row;
  });
}

function splitCsvLine(line) {
  const out = [];
  let cur = '';
  let i = 0;
  let quoted = false;
  while (i < line.length) {
    const ch = line[i];
    if (quoted) {
      if (ch === '"') {
        if (line[i + 1] === '"') {
          cur += '"';
          i += 2;
          continue;
        }
        quoted = false;
        i += 1;
        continue;
      }
      cur += ch;
      i += 1;
      continue;
    }
    if (ch === ',') {
      out.push(cur);
      cur = '';
      i += 1;
      continue;
    }
    if (ch === '"') {
      quoted = true;
      i += 1;
      continue;
    }
    cur += ch;
    i += 1;
  }
  out.push(cur);
  return out;
}

function collectFiles(inputs) {
  const files = [];
  for (const input of inputs) {
    const stat = fs.statSync(input);
    if (stat.isDirectory()) {
      walkDir(input, files);
    } else if (stat.isFile()) {
      files.push(input);
    }
  }
  return files;
}

function walkDir(dir, out) {
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) walkDir(full, out);
    else if (entry.isFile()) out.push(full);
  }
}

function inferFormat(file) {
  const ext = path.extname(file).toLowerCase();
  if (ext === '.jsonl' || ext === '.json') return 'jsonl';
  if (ext === '.csv') return 'csv';
  return null;
}

function readRows(files) {
  const rows = [];
  for (const file of files) {
    const text = fs.readFileSync(file, 'utf8');
    const format = inferFormat(file);
    const parsed = format === 'csv' ? parseCsv(text, file) : parseJsonl(text, file);
    rows.push(...parsed);
  }
  return rows;
}

function field(row, names) {
  for (const name of names) {
    if (row[name] !== undefined && row[name] !== null && row[name] !== '') return row[name];
  }
  return undefined;
}

function extractSample(row) {
  const metric = normalizeMetric(field(row, ['metric', 'name', 'stage', 'op', 'step']));
  if (!metric) return null;
  const value = toNumber(field(row, ['value_us', 'us', 'duration_us', 'elapsed_us', 'time_us', 'latency_us']))
    ?? (toNumber(field(row, ['value_ms', 'duration_ms', 'elapsed_ms', 'time_ms', 'latency_ms'])) * 1000)
    ?? (toNumber(field(row, ['value_ns', 'duration_ns', 'elapsed_ns', 'time_ns', 'latency_ns'])) / 1000);
  if (!Number.isFinite(value)) {
    throw new Error(`${row.__source}:${row.__line} missing timing value for ${metric}`);
  }
  return {
    metric,
    value,
    cpu: toNumber(field(row, ['cpu_ms', 'cpu_time_ms', 'cpu_ms_total', 'cpu_seconds'])) ?? null,
    memory: toNumber(field(row, ['peak_mem_bytes', 'peak_memory_bytes', 'rss_bytes', 'memory_bytes'])) ?? null,
  };
}

function summarize(rows) {
  const buckets = new Map();
  let cpu = null;
  let memory = null;
  for (const row of rows) {
    const sample = extractSample(row);
    if (!sample) continue;
    if (!buckets.has(sample.metric)) buckets.set(sample.metric, []);
    buckets.get(sample.metric).push(sample.value);
    if (sample.cpu !== null) cpu = sample.cpu;
    if (sample.memory !== null) memory = sample.memory;
  }
  return { buckets, cpu, memory };
}

function formatNumber(n) {
  if (n === null || n === undefined || Number.isNaN(n)) return 'n/a';
  return Number.isInteger(n) ? String(n) : String(Number(n.toFixed(2)));
}

function renderSummary(summary) {
  const lines = [];
  for (const metric of METRIC_ORDER) {
    const samples = summary.buckets.get(metric);
    if (!samples || samples.length === 0) {
      lines.push(`${metric}: skipped`);
      continue;
    }
    const s = percentiles(samples);
    lines.push(`${metric}: count=${samples.length} p50_us=${formatNumber(s.p50)} p95_us=${formatNumber(s.p95)} max_us=${formatNumber(s.max)}`);
  }
  if (summary.cpu !== null) lines.push(`cpu_ms=${formatNumber(summary.cpu)}`);
  if (summary.memory !== null) lines.push(`peak_mem_bytes=${formatNumber(summary.memory)}`);
  return lines;
}

function selfTestDir() {
  return path.join(path.dirname(fileURLToPath(import.meta.url)), 'fixtures', 'perf-stage5b');
}

function selfTest() {
  const dir = selfTestDir();
  for (const name of ['sample.jsonl', 'sample.csv']) {
    const file = path.join(dir, name);
    const summary = summarize(readRows([file]));
    const decode = summary.buckets.get('decode_latest_png') || [];
    if (decode.length !== 3) throw new Error(`self-test expected 3 decode_latest_png samples in ${name}`);
    const out = renderSummary(summary).join('\n');
    if (!out.includes('ffmpeg_start: count=3')) throw new Error(`self-test missing ffmpeg_start summary in ${name}`);
    console.log(`[${name}]`);
    console.log(out);
  }
  console.log('self-test: ok');
}

function main(argv) {
  const inputs = [];
  let json = false;
  let dryRun = false;
  let self = false;
  for (let i = 2; i < argv.length; i++) {
    const arg = argv[i];
    if (arg === '--help' || arg === '-h') usage(0);
    if (arg === '--json') { json = true; continue; }
    if (arg === '--dry-run') { dryRun = true; continue; }
    if (arg === '--self-test') { self = true; continue; }
    if (arg === '--input') {
      const next = argv[++i];
      if (!next) usage(1);
      inputs.push(next);
      continue;
    }
    if (arg.startsWith('--input=')) {
      inputs.push(arg.slice('--input='.length));
      continue;
    }
    throw new Error(`unknown argument: ${arg}`);
  }

  if (self) {
    selfTest();
    return;
  }
  if (inputs.length === 0) usage(1);
  const files = collectFiles(inputs);
  if (files.length === 0) {
    console.log('skipped: no input files found');
    return;
  }
  const summary = summarize(readRows(files));
  const totalSamples = [...summary.buckets.values()].reduce((n, arr) => n + arr.length, 0);
  if (totalSamples === 0) {
    console.log('skipped: no benchmark samples');
    return;
  }
  if (dryRun) {
    console.log(`dry-run: parsed ${files.length} file(s), ${totalSamples} sample(s)`);
    return;
  }
  if (json) {
    const payload = { metrics: {}, cpu_ms: summary.cpu, peak_mem_bytes: summary.memory };
    for (const metric of METRIC_ORDER) {
      const samples = summary.buckets.get(metric);
      payload.metrics[metric] = samples?.length ? { count: samples.length, ...percentiles(samples) } : { skipped: true };
    }
    console.log(JSON.stringify(payload, null, 2));
    return;
  }
  console.log(renderSummary(summary).join('\n'));
}

try {
  main(process.argv);
} catch (err) {
  console.error(err instanceof Error ? err.message : String(err));
  process.exit(1);
}
