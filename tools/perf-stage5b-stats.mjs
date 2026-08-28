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

const RESOURCE_ALIASES = new Map([
  ['cpu_ms', 'cpu_ms'],
  ['cpu_time_ms', 'cpu_ms'],
  ['cpu_ms_total', 'cpu_ms'],
  ['cpu_seconds', 'cpu_ms'],
  ['peak_mem_bytes', 'peak_mem_bytes'],
  ['peak_memory_bytes', 'peak_mem_bytes'],
  ['rss_bytes', 'peak_mem_bytes'],
  ['memory_bytes', 'peak_mem_bytes'],
]);

function usage(exitCode = 0) {
  const msg = [
    'usage: node tools/perf-stage5b-stats.mjs --input <file|dir> [--input ...] [--json] [--include-resource] [--self-test] [--dry-run]',
    '',
    'Reads JSONL/CSV benchmark samples and prints p50/p95/max for the stage5 B metrics.',
    'Use --include-resource to include CPU and peak-memory fields when present.',
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
  for (const [lineNo, line] of text.replace(/^\uFEFF/, '').split(/\r?\n/).entries()) {
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
  const headers = splitCsvLine(lines[0]).map((h, idx) => (idx === 0 ? h.replace(/^\uFEFF/, '') : h).trim());
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
      if (!inferFormat(input)) {
        throw new Error(`${input}: unsupported input format; use .jsonl, .json, or .csv`);
      }
      files.push(input);
    }
  }
  files.sort();
  return files;
}

function walkDir(dir, out) {
  const entries = fs.readdirSync(dir, { withFileTypes: true }).sort((a, b) => a.name.localeCompare(b.name));
  for (const entry of entries) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) walkDir(full, out);
    else if (entry.isFile() && inferFormat(full)) out.push(full);
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

function firstNumber(row, names) {
  for (const name of names) {
    const value = toNumber(row[name]);
    if (value !== null) return value;
  }
  return null;
}

function normalizeResource(raw) {
  if (!raw) return null;
  return RESOURCE_ALIASES.get(String(raw).trim().toLowerCase()) || null;
}

function timingValueUs(row) {
  const valueUs = firstNumber(row, ['value_us', 'us', 'duration_us', 'elapsed_us', 'time_us', 'latency_us']);
  if (valueUs !== null) return valueUs;
  const valueMs = firstNumber(row, ['value_ms', 'ms', 'duration_ms', 'elapsed_ms', 'time_ms', 'latency_ms']);
  if (valueMs !== null) return valueMs * 1000;
  const valueNs = firstNumber(row, ['value_ns', 'ns', 'duration_ns', 'elapsed_ns', 'time_ns', 'latency_ns']);
  if (valueNs !== null) return valueNs / 1000;
  return null;
}

function resourceValue(row, resource, rawName) {
  if (resource === 'cpu_ms') {
    const direct = firstNumber(row, ['cpu_ms', 'cpu_time_ms', 'cpu_ms_total']);
    if (direct !== null) return direct;
    const valueMs = firstNumber(row, ['value_ms', 'duration_ms', 'elapsed_ms', 'time_ms', 'latency_ms']);
    if (valueMs !== null) return valueMs;
    const valueSeconds = firstNumber(row, ['value_s', 'value_seconds', 'duration_s', 'duration_seconds', 'cpu_seconds']);
    if (valueSeconds !== null) return valueSeconds * 1000;
    const valueUs = firstNumber(row, ['value_us', 'duration_us', 'elapsed_us', 'time_us', 'latency_us']);
    if (valueUs !== null) return valueUs / 1000;
    const valueNs = firstNumber(row, ['value_ns', 'duration_ns', 'elapsed_ns', 'time_ns', 'latency_ns']);
    if (valueNs !== null) return valueNs / 1000000;
    const generic = firstNumber(row, ['value', 'amount']);
    if (generic !== null) return rawName === 'cpu_seconds' ? generic * 1000 : generic;
  } else {
    const direct = firstNumber(row, ['peak_mem_bytes', 'peak_memory_bytes', 'rss_bytes', 'memory_bytes']);
    if (direct !== null) return direct;
    const valueBytes = firstNumber(row, ['value_bytes', 'bytes', 'memory']);
    if (valueBytes !== null) return valueBytes;
    const valueKb = firstNumber(row, ['value_kb', 'memory_kb']);
    if (valueKb !== null) return valueKb * 1024;
    const valueMb = firstNumber(row, ['value_mb', 'memory_mb']);
    if (valueMb !== null) return valueMb * 1024 * 1024;
    const generic = firstNumber(row, ['value', 'amount']);
    if (generic !== null) return generic;
  }
  return null;
}

function extractResourceSample(row) {
  const rawName = field(row, ['resource', 'resource_metric', 'metric', 'name']);
  const resource = normalizeResource(rawName);
  if (!resource) return null;
  const value = resourceValue(row, resource, String(rawName).trim().toLowerCase());
  if (value === null) {
    throw new Error(`${row.__source}:${row.__line} missing resource value for ${resource}`);
  }
  return { resource, value };
}

function extractMetadataResources(row) {
  const values = [];
  const cpuMs = firstNumber(row, ['cpu_ms', 'cpu_time_ms', 'cpu_ms_total']);
  if (cpuMs !== null) values.push({ resource: 'cpu_ms', value: cpuMs });
  const cpuSeconds = firstNumber(row, ['cpu_seconds']);
  if (cpuSeconds !== null && cpuMs === null) values.push({ resource: 'cpu_ms', value: cpuSeconds * 1000 });
  const memory = firstNumber(row, ['peak_mem_bytes', 'peak_memory_bytes', 'rss_bytes', 'memory_bytes']);
  if (memory !== null) values.push({ resource: 'peak_mem_bytes', value: memory });
  return values;
}

function extractSample(row) {
  const metric = normalizeMetric(field(row, ['metric', 'name', 'stage', 'op', 'step']));
  if (!metric) return null;
  const value = timingValueUs(row);
  if (value === null) {
    throw new Error(`${row.__source}:${row.__line} missing timing value for ${metric}`);
  }
  return { metric, value };
}

function summarize(rows) {
  const buckets = new Map();
  const resources = { cpu_ms: null, peak_mem_bytes: null };
  for (const row of rows) {
    const resourceSample = extractResourceSample(row);
    if (resourceSample) resources[resourceSample.resource] = resourceSample.value;
    else {
      for (const resource of extractMetadataResources(row)) resources[resource.resource] = resource.value;
    }
    const sample = extractSample(row);
    if (!sample) continue;
    if (!buckets.has(sample.metric)) buckets.set(sample.metric, []);
    buckets.get(sample.metric).push(sample.value);
  }
  return { buckets, resources };
}

function formatNumber(n) {
  if (n === null || n === undefined || Number.isNaN(n)) return 'n/a';
  return Number.isInteger(n) ? String(n) : String(Number(n.toFixed(2)));
}

function renderSummary(summary, includeResource = false) {
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
  if (includeResource) {
    if (summary.resources.cpu_ms !== null) lines.push(`cpu_ms=${formatNumber(summary.resources.cpu_ms)}`);
    if (summary.resources.peak_mem_bytes !== null) lines.push(`peak_mem_bytes=${formatNumber(summary.resources.peak_mem_bytes)}`);
  }
  return lines;
}

function jsonSummary(summary, includeResource) {
  const payload = { metrics: {} };
  if (includeResource) {
    payload.cpu_ms = summary.resources.cpu_ms;
    payload.peak_mem_bytes = summary.resources.peak_mem_bytes;
  }
  for (const metric of METRIC_ORDER) {
    const samples = summary.buckets.get(metric);
    payload.metrics[metric] = samples?.length ? { count: samples.length, ...percentiles(samples) } : { skipped: true };
  }
  return payload;
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
    if (summary.resources.cpu_ms !== 12) throw new Error(`self-test expected cpu_ms=12 in ${name}`);
    if (summary.resources.peak_mem_bytes !== 3456789) throw new Error(`self-test expected peak memory in ${name}`);
    const out = renderSummary(summary, true).join('\n');
    if (!out.includes('ffmpeg_start: count=3')) throw new Error(`self-test missing ffmpeg_start summary in ${name}`);
    if (!out.includes('cpu_ms=12') || !out.includes('peak_mem_bytes=3456789')) {
      throw new Error(`self-test missing resource summary in ${name}`);
    }
    console.log(`[${name}]`);
    console.log(out);
  }
  const converted = summarize([
    { metric: 'ncc_region', value_ms: 1.5, __source: 'self-test', __line: 1 },
    { metric: 'ncc_fullscreen', value_ns: 2500, __source: 'self-test', __line: 2 },
    { metric: 'find_round', value_us: 3, cpu_seconds: 0.25, __source: 'self-test', __line: 3 },
  ]);
  if (converted.buckets.get('ncc_region')[0] !== 1500) throw new Error('self-test ms to us conversion failed');
  if (converted.buckets.get('ncc_fullscreen')[0] !== 2.5) throw new Error('self-test ns to us conversion failed');
  if (converted.resources.cpu_ms !== 250) throw new Error('self-test seconds to ms conversion failed');
  console.log('self-test: ok');
}

function main(argv) {
  const inputs = [];
  let json = false;
  let includeResource = false;
  let dryRun = false;
  let self = false;
  for (let i = 2; i < argv.length; i++) {
    const arg = argv[i];
    if (arg === '--help' || arg === '-h') usage(0);
    if (arg === '--json') { json = true; continue; }
    if (arg === '--include-resource') { includeResource = true; continue; }
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
    console.log(JSON.stringify(jsonSummary(summary, includeResource), null, 2));
    return;
  }
  console.log(renderSummary(summary, includeResource).join('\n'));
}

try {
  main(process.argv);
} catch (err) {
  console.error(err instanceof Error ? err.message : String(err));
  process.exit(1);
}
