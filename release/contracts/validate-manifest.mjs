#!/usr/bin/env node
// Release manifest 结构、语义与 SHA256 声明校验；来源由官方 HTTPS 发布渠道保障。
import { readFileSync, readdirSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const SCRIPT_DIR = path.dirname(fileURLToPath(import.meta.url));
const FIXTURES_DIR = path.join(SCRIPT_DIR, 'fixtures');
const VALID_DIR = path.join(FIXTURES_DIR, 'manifest', 'valid');
const INVALID_DIR = path.join(FIXTURES_DIR, 'manifest', 'invalid');
const SCHEMA_PATH = path.join(SCRIPT_DIR, 'manifest-v1.schema.json');

// ---------------------------------------------------------------------------
// 冻结常量（与 manifest-v1.md 保持一致）
// ---------------------------------------------------------------------------

const PRODUCT = 'gamebot';
const KNOWN_PLATFORMS = ['windows-x86_64'];
const KNOWN_CHANNELS = ['stable', 'beta'];
const JAR_BINDING = 'application';
const SEMVER_RE =
  /^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(?:-((?:0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*)(?:\.(?:0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*))*))?(?:\+([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?$/;

const LIMITS = {
  maxArtifactBytes: 2147483648, // 2 GiB：单压缩包
  maxFileBytes: 1073741824, // 1 GiB：单文件
  maxTotalBytes: 6442450944, // 6 GiB：平台内所有声明 size 之和
  maxComponents: 16, // schema 同步约束
  maxFilesPerComponent: 1024, // schema 同步约束
};

// Windows 保留设备名（按“最后一个扩展名之前的主名”判断，大小写不敏感）。
const RESERVED_BASES = new Set([
  'CON', 'PRN', 'AUX', 'NUL',
  'COM0', 'COM1', 'COM2', 'COM3', 'COM4', 'COM5', 'COM6', 'COM7', 'COM8', 'COM9',
  'LPT0', 'LPT1', 'LPT2', 'LPT3', 'LPT4', 'LPT5', 'LPT6', 'LPT7', 'LPT8', 'LPT9',
]);

// selftest 中 invalid fixture 文件名（去 .json）→ 必须命中的错误码。
// 新增 invalid fixture 必须在此登记，否则 selftest 直接失败。
const INVALID_EXPECTATIONS = {
  'malformed-json': 'json-parse-failed',
  'unknown-schema-version': 'unknown-schema-version',
  'unknown-platform': 'unknown-platform',
  'version-not-semver': 'version-not-semver',
  'version-downgrade': 'version-downgrade',
  'channel-mismatch': 'channel-mismatch',
  'jar-binding-mismatch': 'jar-binding-mismatch',
  'path-absolute': 'path-absolute',
  'path-drive-letter': 'path-drive-letter',
  'path-dotdot': 'path-dotdot',
  'path-ads-colon': 'path-ads-colon',
  'path-backslash': 'path-backslash',
  'path-reserved-name': 'path-reserved-name',
  'path-case-collision': 'path-case-collision',
  'path-duplicate-entry': 'path-duplicate-entry',
  'sha256-uppercase': 'sha256-uppercase',
  'sha256-wrong-length': 'sha256-wrong-length',
  'size-negative': 'size-negative',
  'size-oversized': 'size-oversized',
};

// selftest 对全部 fixture 使用的统一期望参数：
// 所有合法/非法 fixture 的 release.version 均为 0.2.0（version-downgrade 为 0.1.0），
// channel 均为 stable —— 只有目标违规项会触发对应错误码。
const SELFTEST_EXPECT_CURRENT_VERSION = '0.2.0';
const SELFTEST_EXPECT_CHANNEL = 'stable';

// ---------------------------------------------------------------------------
// 小工具
// ---------------------------------------------------------------------------

const isObj = (v) => typeof v === 'object' && v !== null && !Array.isArray(v);


function parseSemver(s) {
  const m = SEMVER_RE.exec(s);
  if (!m) return null;
  return { major: Number(m[1]), minor: Number(m[2]), patch: Number(m[3]), pre: m[4] ? m[4].split('.') : null };
}

function comparePreIdentifier(a, b) {
  const na = /^[0-9]+$/.test(a);
  const nb = /^[0-9]+$/.test(b);
  if (na && nb) {
    // 数字标识按数值比较（先比长度避免超精度），禁前导零已由 SEMVER_RE 保证。
    if (a.length !== b.length) return a.length < b.length ? -1 : 1;
    return a < b ? -1 : a > b ? 1 : 0;
  }
  if (na) return -1; // 数字标识 < 字母数字标识
  if (nb) return 1;
  return a < b ? -1 : a > b ? 1 : 0;
}

function semverLt(aStr, bStr) {
  const a = parseSemver(aStr);
  const b = parseSemver(bStr);
  if (!a || !b) return false;
  if (a.major !== b.major) return a.major < b.major;
  if (a.minor !== b.minor) return a.minor < b.minor;
  if (a.patch !== b.patch) return a.patch < b.patch;
  if (a.pre && b.pre) {
    const n = Math.max(a.pre.length, b.pre.length);
    for (let i = 0; i < n; i++) {
      if (a.pre[i] === undefined) return true; // 短的一组更小
      if (b.pre[i] === undefined) return false;
      const c = comparePreIdentifier(a.pre[i], b.pre[i]);
      if (c !== 0) return c < 0;
    }
    return false;
  }
  if (a.pre && !b.pre) return true; // prerelease < 正式版
  return false;
}

// ---------------------------------------------------------------------------
// 路径安全（计划 §6.2 / §11.1）
// 返回错误码字符串；null 表示该单条路径本身安全。
// ---------------------------------------------------------------------------

function checkSinglePath(p) {
  if (typeof p !== 'string' || p.length === 0) return 'path-empty';
  if (p.includes('\\')) return 'path-backslash'; // 必须使用规范化 '/' 分隔
  if (p.startsWith('/')) return 'path-absolute';
  if (/^[A-Za-z]:/.test(p)) return 'path-drive-letter';
  if (p.includes(':')) return 'path-ads-colon'; // NTFS 备用数据流（其余冒号形态）
  if (/[\u0000-\u001f<>|"?*]/.test(p)) return 'path-illegal-chars';
  const segments = p.split('/');
  for (const seg of segments) {
    if (seg === '') return 'path-not-normalized'; // 'a//b'、尾随 '/'
    if (seg === '.' || seg === '..') return 'path-dotdot';
    if (/[. ]$/.test(seg)) return 'path-trailing-dot-space'; // Windows 会剥掉尾随点/空格
    const base = seg.split('.')[0].toUpperCase();
    if (RESERVED_BASES.has(base)) return 'path-reserved-name'; // 含 con.nul / nul.txt 等形态
  }
  return null;
}

function collectPlatformPathEntries(platform) {
  const entries = []; // { at, path } —— 安装树命名空间：entrypoint + required_files + resources
  const names = []; // { at, name } —— 发行资产名：仅做单条安全检查，不参与碰撞检测
  const at = (s) => `windows-x86_64.${s}`;
  if (isObj(platform.app)) {
    if (typeof platform.app.entrypoint === 'string') {
      entries.push({ at: at('app.entrypoint'), path: platform.app.entrypoint });
    }
    if (isObj(platform.app.artifact) && typeof platform.app.artifact.name === 'string') {
      names.push({ at: at('app.artifact.name'), path: platform.app.artifact.name });
    }
  }
  if (Array.isArray(platform.components)) {
    platform.components.forEach((comp, ci) => {
      if (!isObj(comp)) return;
      const cid = typeof comp.id === 'string' ? comp.id : `#${ci}`;
      if (Array.isArray(comp.required_files)) {
        comp.required_files.forEach((f, fi) => {
          if (isObj(f) && typeof f.path === 'string') {
            entries.push({ at: at(`components[${ci}](${cid}).required_files[${fi}]`), path: f.path });
          }
        });
      }
      if (isObj(comp.artifact) && typeof comp.artifact.name === 'string') {
        names.push({ at: at(`components[${ci}](${cid}).artifact.name`), path: comp.artifact.name });
      }
    });
  }
  if (isObj(platform.resources)) {
    for (const [rid, res] of Object.entries(platform.resources)) {
      if (isObj(res) && typeof res.path === 'string') {
        entries.push({ at: at(`resources.${rid}.path`), path: res.path });
      }
    }
  }
  return { entries, names };
}

// ---------------------------------------------------------------------------
// 显式语义校验（在 schema 回退校验之前运行，保证每个违规项有专属错误码）
// ---------------------------------------------------------------------------

function semanticChecks(manifest, { expectCurrentVersion, expectChannel }) {
  const errors = [];
  const push = (code, detail) => errors.push({ code, detail });

  // -- schema / product -----------------------------------------------------
  if (manifest.schema_version !== undefined && manifest.schema_version !== 1) {
    push('unknown-schema-version', `schema_version=${JSON.stringify(manifest.schema_version)}; only 1 is defined`);
  }
  if (manifest.product !== undefined && manifest.product !== PRODUCT) {
    push('product-mismatch', `product=${JSON.stringify(manifest.product)}; expected "${PRODUCT}"`);
  }

  // -- release 级规则 --------------------------------------------------------
  const release = isObj(manifest.release) ? manifest.release : {};
  for (const field of ['version', 'minimum_launcher_version', 'minimum_upgrade_version']) {
    const v = release[field];
    if (typeof v === 'string' && !SEMVER_RE.test(v)) {
      push('version-not-semver', `release.${field}=${JSON.stringify(v)} is not SemVer 2.0.0`);
    }
  }
  if (release.channel !== undefined && !KNOWN_CHANNELS.includes(release.channel)) {
    push('channel-invalid', `release.channel=${JSON.stringify(release.channel)}`);
  }
  if (expectCurrentVersion) {
    if (typeof release.version === 'string' && SEMVER_RE.test(release.version)) {
      if (!SEMVER_RE.test(expectCurrentVersion)) {
        push('version-not-semver', `--expect-current-version ${JSON.stringify(expectCurrentVersion)} is not SemVer`);
      } else if (semverLt(release.version, expectCurrentVersion)) {
        push(
          'version-downgrade',
          `release.version ${release.version} < current ${expectCurrentVersion}; downgrade refused (plan §11.1)`,
        );
      }
    }
  }
  if (expectChannel && release.channel !== undefined && release.channel !== expectChannel) {
    push(
      'channel-mismatch',
      `release.channel=${JSON.stringify(release.channel)} but this track consumes ${JSON.stringify(expectChannel)}`,
    );
  }

  // -- 平台白名单 ------------------------------------------------------------
  const platforms = manifest.platforms;
  if (isObj(platforms)) {
    const keys = Object.keys(platforms);
    if (keys.length === 0) {
      push('unknown-platform', 'platforms is empty');
    }
    for (const key of keys) {
      if (!KNOWN_PLATFORMS.includes(key)) {
        push('unknown-platform', `platform "${key}" is not in [${KNOWN_PLATFORMS.join(', ')}]`);
      }
    }
  }

  // -- 每平台深检 -------------------------------------------------------------
  if (isObj(platforms)) {
    for (const [pname, platform] of Object.entries(platforms)) {
      if (!isObj(platform)) continue;

      // jar 绑定（计划 §2：scrcpy-server.jar 与应用版本强绑定）
      const res = isObj(platform.resources) ? platform.resources : {};
      const jar = res.scrcpy_server;
      if (isObj(jar)) {
        if (jar.binding !== undefined && jar.binding !== JAR_BINDING) {
          push(
            'jar-binding-mismatch',
            `${pname}.resources.scrcpy_server.binding=${JSON.stringify(jar.binding)}; jar must be "${JAR_BINDING}"-bound`,
          );
        }
        if (typeof jar.path === 'string' && !jar.path.startsWith('assets/')) {
          push('jar-path-not-assets', `${pname}.resources.scrcpy_server.path must live under assets/`);
        }
      }

      // 收集 hash / size / path
      const hashes = []; // { at, value }
      const sizes = []; // { at, value, cap, kind }
      const pushArtifact = (at, a) => {
        if (!isObj(a)) return;
        if (typeof a.sha256 === 'string') hashes.push({ at, value: a.sha256 });
        if (typeof a.size === 'number') sizes.push({ at, value: a.size, cap: LIMITS.maxArtifactBytes, kind: 'artifact' });
      };
      pushArtifact(`${pname}.app.artifact`, isObj(platform.app) ? platform.app.artifact : undefined);
      let totalBytes = 0;
      if (Array.isArray(platform.components)) {
        platform.components.forEach((comp, ci) => {
          if (!isObj(comp)) return;
          const cid = typeof comp.id === 'string' ? comp.id : `#${ci}`;
          pushArtifact(`${pname}.components[${ci}](${cid}).artifact`, comp.artifact);
          if (Array.isArray(comp.required_files)) {
            comp.required_files.forEach((f, fi) => {
              if (!isObj(f)) return;
              const at = `${pname}.components[${ci}](${cid}).required_files[${fi}]`;
              if (typeof f.sha256 === 'string') hashes.push({ at, value: f.sha256 });
              if (typeof f.size === 'number') sizes.push({ at, value: f.size, cap: LIMITS.maxFileBytes, kind: 'file' });
            });
          }
        });
      }
      if (isObj(jar) && typeof jar.sha256 === 'string') {
        hashes.push({ at: `${pname}.resources.scrcpy_server.sha256`, value: jar.sha256 });
      }
      for (const s of sizes) totalBytes += s.value;

      // hash 规则：64 位小写 hex
      for (const h of hashes) {
        if (/^[0-9a-fA-F]{64}$/.test(h.value) && /[A-F]/.test(h.value)) {
          push('sha256-uppercase', `${h.at}: hash must be lowercase`);
        } else if (!/^[0-9a-f]{64}$/.test(h.value)) {
          push('sha256-wrong-length', `${h.at}: expected 64 lowercase hex chars, got length ${h.value.length}`);
        }
      }

      // size 规则：>= 0 且受限
      for (const s of sizes) {
        if (s.value < 0) push('size-negative', `${s.at}: size=${s.value}`);
        else if (s.value > s.cap) {
          push('size-oversized', `${s.at}: size=${s.value} exceeds ${s.kind} limit ${s.cap}`);
        }
      }
      if (totalBytes > LIMITS.maxTotalBytes) {
        push('size-oversized', `${pname}: total declared size ${totalBytes} exceeds limit ${LIMITS.maxTotalBytes}`);
      }

      // 路径规则（单条）
      const { entries, names } = collectPlatformPathEntries(platform);
      for (const entry of [...entries, ...names]) {
        const code = checkSinglePath(entry.path);
        if (code) push(code, `${entry.at}: ${JSON.stringify(entry.path)}`);
      }

      // 路径规则（跨条目：安装树命名空间内，大小写不敏感碰撞与重复条目）
      const seen = new Map(); // lower → 原始字符串
      for (const entry of entries) {
        const lower = entry.path.toLowerCase();
        const prev = seen.get(lower);
        if (prev === undefined) {
          seen.set(lower, entry.path);
        } else if (prev === entry.path) {
          push('path-duplicate-entry', `${pname}: ${JSON.stringify(entry.path)} declared twice (${entry.at})`);
        } else {
          push(
            'path-case-collision',
            `${pname}: ${JSON.stringify(entry.path)} collides case-insensitively with ${JSON.stringify(prev)} (${entry.at})`,
          );
        }
      }
    }
  }

  return errors;
}

// ---------------------------------------------------------------------------
// 迷你 JSON Schema 解释器（draft 2020-12 子集，只支持本 schema 用到的关键字），
// 作为结构回退校验执行 manifest-v1.schema.json —— schema 与 fixtures 保持同一份事实。
// ---------------------------------------------------------------------------

let CACHED_SCHEMA = null;
function loadSchema() {
  if (!CACHED_SCHEMA) CACHED_SCHEMA = JSON.parse(readFileSync(SCHEMA_PATH, 'utf8'));
  return CACHED_SCHEMA;
}

function resolveRef(root, ref) {
  if (!ref.startsWith('#/')) throw new Error(`unsupported $ref ${ref}`);
  let cur = root;
  for (const raw of ref.slice(2).split('/')) {
    const seg = raw.replace(/~1/g, '/').replace(/~0/g, '~');
    cur = cur[seg];
    if (cur === undefined) throw new Error(`unresolvable $ref ${ref}`);
  }
  return cur;
}

function typeMatches(t, v) {
  switch (t) {
    case 'object':
      return isObj(v);
    case 'array':
      return Array.isArray(v);
    case 'string':
      return typeof v === 'string';
    case 'boolean':
      return typeof v === 'boolean';
    case 'null':
      return v === null;
    case 'integer':
      return typeof v === 'number' && Number.isInteger(v);
    case 'number':
      return typeof v === 'number' && Number.isFinite(v);
    default:
      return false;
  }
}

function schemaWalk(schema, root, value, ptr, errors) {
  if (schema === true || schema == null) return;
  if (schema === false) {
    errors.push({ ptr, msg: 'schema forbids any value' });
    return;
  }
  const fail = (msg) => errors.push({ ptr, msg });

  if (schema.$ref) {
    schemaWalk(resolveRef(root, schema.$ref), root, value, ptr, errors);
    return;
  }
  if (Array.isArray(schema.allOf)) {
    for (const sub of schema.allOf) schemaWalk(sub, root, value, ptr, errors);
  }
  if (schema.type) {
    const types = Array.isArray(schema.type) ? schema.type : [schema.type];
    if (!types.some((t) => typeMatches(t, value))) {
      fail(`expected type ${types.join('|')}, got ${Array.isArray(value) ? 'array' : typeof value}`);
      return;
    }
  }
  if ('const' in schema && value !== schema.const) fail(`must equal ${JSON.stringify(schema.const)}`);
  if (Array.isArray(schema.enum) && !schema.enum.some((e) => e === value)) {
    fail(`must be one of ${schema.enum.map((e) => JSON.stringify(e)).join(', ')}`);
  }
  if (typeof value === 'string') {
    if (schema.pattern !== undefined && !new RegExp(schema.pattern).test(value)) {
      fail(`string does not match pattern ${schema.pattern}`);
    }
    if (schema.minLength !== undefined && value.length < schema.minLength) fail(`shorter than minLength ${schema.minLength}`);
    if (schema.maxLength !== undefined && value.length > schema.maxLength) fail(`longer than maxLength ${schema.maxLength}`);
  }
  if (typeof value === 'number') {
    if (schema.minimum !== undefined && value < schema.minimum) fail(`less than minimum ${schema.minimum}`);
    if (schema.maximum !== undefined && value > schema.maximum) fail(`greater than maximum ${schema.maximum}`);
  }
  if (isObj(value)) {
    if (Array.isArray(schema.required)) {
      for (const key of schema.required) {
        if (!(key in value)) fail(`missing required property "${key}"`);
      }
    }
    if (isObj(schema.properties)) {
      for (const [key, sub] of Object.entries(schema.properties)) {
        if (key in value) schemaWalk(sub, root, value[key], `${ptr}.${key}`, errors);
      }
    }
    if (schema.additionalProperties === false && isObj(schema.properties)) {
      for (const key of Object.keys(value)) {
        if (!(key in schema.properties)) fail(`unknown property "${key}"`);
      }
    }
    if (isObj(schema.propertyNames)) {
      for (const key of Object.keys(value)) {
        const before = errors.length;
        schemaWalk(schema.propertyNames, root, key, `${ptr}.<key:${key}>`, errors);
        for (let i = before; i < errors.length; i++) errors[i].msg = `property name error: ${errors[i].msg}`;
      }
    }
  }
  if (Array.isArray(value)) {
    if (schema.minItems !== undefined && value.length < schema.minItems) fail(`fewer than minItems ${schema.minItems}`);
    if (schema.maxItems !== undefined && value.length > schema.maxItems) fail(`more than maxItems ${schema.maxItems}`);
    if (schema.items !== undefined) {
      value.forEach((item, i) => schemaWalk(schema.items, root, item, `${ptr}[${i}]`, errors));
    }
  }
}

// ---------------------------------------------------------------------------
// 主校验流程：解析 JSON、语义与结构校验
// ---------------------------------------------------------------------------

function validateManifestFile(options) {
  const {
    manifestPath,
    expectCurrentVersion = null,
    expectChannel = null,
  } = options;
  const errors = [];

  let raw;
  try {
    raw = readFileSync(manifestPath);
  } catch (e) {
    return { ok: false, errors: [{ code: 'io-error', detail: `cannot read manifest: ${e.message}` }], info: {} };
  }

  // 2) 解析
  let manifest;
  try {
    manifest = JSON.parse(raw.toString('utf8'));
  } catch (e) {
    errors.push({ code: 'json-parse-failed', detail: `invalid JSON: ${e.message}` });
    return { ok: false, errors, info: {} };
  }
  if (!isObj(manifest)) {
    errors.push({ code: 'schema-invalid', detail: 'root must be a JSON object' });
    return { ok: false, errors, info: {} };
  }

  // 3) 显式语义规则（专属错误码）
  errors.push(...semanticChecks(manifest, { expectCurrentVersion, expectChannel }));

  // 4) 结构回退校验（迷你 JSON Schema 解释器）
  if (errors.length === 0) {
    const schemaErrors = [];
    schemaWalk(loadSchema(), loadSchema(), manifest, 'manifest', schemaErrors);
    for (const e of schemaErrors) {
      errors.push({ code: 'schema-invalid', detail: `${e.ptr}: ${e.msg}` });
    }
  }

  const info = {
    version: isObj(manifest.release) ? manifest.release.version : undefined,
    channel: isObj(manifest.release) ? manifest.release.channel : undefined,
    platforms: isObj(manifest.platforms) ? Object.keys(manifest.platforms) : [],
  };
  return { ok: errors.length === 0, errors, info };
}

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

function printErrors(errors) {
  for (const e of errors) console.log(`  [${e.code}] ${e.detail}`);
}

function runCheck(args) {
  if (args.length === 0) return usage(2);
  const manifestPath = args[0];
  const opts = { manifestPath };
  for (let i = 1; i < args.length; i++) {
    const a = args[i];
    const needValue = (name) => {
      const v = args[++i];
      if (v === undefined) {
        console.error(`missing value for ${name}`);
        process.exit(2);
      }
      return v;
    };
    if (a === '--expect-current-version') opts.expectCurrentVersion = needValue(a);
    else if (a === '--expect-channel') opts.expectChannel = needValue(a);
    else {
      console.error(`unknown option: ${a}`);
      return usage(2);
    }
  }
  const res = validateManifestFile(opts);
  console.log(`manifest: ${manifestPath}`);
  if (res.ok) {
    console.log(`release: ${res.info.version} (${res.info.channel}); platforms: ${res.info.platforms.join(', ')}`);
    console.log('OK — release manifest v1 valid');
    return 0;
  }
  console.log(`FAIL — ${res.errors.length} error(s)`);
  printErrors(res.errors);
  return 1;
}

function runSelfTest() {
  let pass = 0;
  let fail = 0;
  const results = [];
  const report = (ok, label, detail) => {
    if (ok) pass++;
    else fail++;
    results.push(`${ok ? 'PASS' : 'FAIL'}  ${label}${detail ? `  ${detail}` : ''}`);
  };

  const baseOpts = {
    expectCurrentVersion: SELFTEST_EXPECT_CURRENT_VERSION,
    expectChannel: SELFTEST_EXPECT_CHANNEL,
  };

  console.log('== valid fixtures ==');
  const validFiles = readdirSync(VALID_DIR).filter((f) => f.endsWith('.json')).sort();
  for (const file of validFiles) {
    const manifestPath = path.join(VALID_DIR, file);
    const res = validateManifestFile({ ...baseOpts, manifestPath });
    report(res.ok, `valid/${file}`, res.ok ? `v${res.info.version}` : `unexpected errors:`.concat('\n').concat(res.errors.map((e) => `    [${e.code}] ${e.detail}`).join('\n')));
  }

  console.log('== invalid fixtures ==');
  const invalidFiles = readdirSync(INVALID_DIR).filter((f) => f.endsWith('.json')).sort();
  for (const file of invalidFiles) {
    const stem = file.replace(/\.json$/, '');
    const expected = INVALID_EXPECTATIONS[stem];
    const manifestPath = path.join(INVALID_DIR, file);
    if (!expected) {
      report(false, `invalid/${file}`, `no expected error code registered in INVALID_EXPECTATIONS`);
      continue;
    }
    const res = validateManifestFile({ ...baseOpts, manifestPath });
    if (res.ok) {
      report(false, `invalid/${file}`, `expected rejection (${expected}) but it validated OK`);
    } else if (res.errors.some((e) => e.code === expected)) {
      report(true, `invalid/${file}`, `rejected as ${expected}`);
    } else {
      report(
        false,
        `invalid/${file}`,
        `expected code "${expected}", got: `.concat(res.errors.map((e) => e.code).join(', ')),
      );
    }
  }

  console.log('---- selftest results ----');
  for (const line of results) console.log(line);
  console.log(
    `selftest: ${pass} passed, ${fail} failed  (valid: ${validFiles.length}, invalid: ${invalidFiles.length})`,
  );
  return fail === 0 ? 0 : 1;
}

function usage(exitCode) {
  console.log(
    [
      'Usage:',
      '  node validate-manifest.mjs selftest',
      '  node validate-manifest.mjs check <manifest.json>',
      '       [--expect-current-version x.y.z] [--expect-channel stable|beta]',
    ].join('\n'),
  );
  return exitCode;
}

const args = process.argv.slice(2);
const cmd = args[0];
if (cmd === 'selftest') process.exit(runSelfTest());
else if (cmd === 'check') process.exit(runCheck(args.slice(1)));
else process.exit(usage(cmd === undefined ? 0 : 2));
