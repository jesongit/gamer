#!/usr/bin/env node
/**
 * REL-003: release manifest 分离签名工具（Node 内置 crypto，零依赖）。
 *
 * 签名文件格式（冻结，与 release/contracts/fixtures/manifest/valid/*.sig 完全一致，
 * 权威定义见 release/contracts/manifest-v1.md）:
 *   行1: gamebot-manifest-sig-1 <key_id>
 *   行2: base64(64 字节 Ed25519 签名，覆盖 manifest 原始字节)
 *   （LF 行尾，行尾各一个换行符）
 *
 * 用法:
 *   node sign-manifest.mjs keygen [--id <key_id>] [--out-dir <dir>] [--force]
 *       生成 Ed25519 密钥对: 公钥 <dir>/<id>.pem（SPKI，可提交/随包分发），
 *       私钥 <dir>/<id>.private.pem（PKCS#8，绝不入库）。默认 id=dev-ed25519-1，
 *       默认目录 <repo>/release/keys。
 *
 *   node sign-manifest.mjs sign <manifest.json> [--key <private.pem>] [--key-env <VAR>] [--key-id <id>] [--out <sig>]
 *       对 manifest 原始字节签名，写出 .sig。私钥二选一：
 *         --key <file>     私钥文件（缺省 <repo>/release/keys/dev-ed25519-1.private.pem，
 *                          key-id 缺省从文件名 <id>.private.pem 推断）；
 *         --key-env <VAR>  从环境变量 <VAR> 读 PKCS#8 PEM（CI 注入 secret 用，私钥不落盘），
 *                          必须显式配对 --key-id（环境变量无法推断 key_id）。
 *       --out 缺省 <manifest 去掉 .json>.sig。
 *
 * 验签走 release/contracts/validate-manifest.mjs check（--keys-dir 指向公钥目录）。
 */

import {
  createPrivateKey,
  generateKeyPairSync,
  sign as cryptoSign,
} from 'node:crypto';
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const SCRIPT_DIR = path.dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = path.resolve(SCRIPT_DIR, '..', '..');
const DEFAULT_ID = 'dev-ed25519-1';
const DEFAULT_OUT_DIR = path.join(REPO_ROOT, 'release', 'keys');
const SIG_MAGIC = 'gamebot-manifest-sig-1';
const KEY_ID_RE = /^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$/;

function usage(exitCode) {
  console.log(
    [
      'Usage:',
      '  node sign-manifest.mjs keygen [--id <key_id>] [--out-dir <dir>] [--force]',
      '  node sign-manifest.mjs sign <manifest.json> [--key <private.pem>] [--key-env <VAR>] [--key-id <id>] [--out <sig>]',
    ].join('\n'),
  );
  return exitCode;
}

function parseArgs(args) {
  const opts = { _: [] };
  for (let i = 0; i < args.length; i++) {
    const a = args[i];
    if (a === '--force') opts.force = true;
    else if (a === '--id') opts.id = args[++i];
    else if (a === '--out-dir') opts.outDir = args[++i];
    else if (a === '--key') opts.key = args[++i];
    else if (a === '--key-env') opts.keyEnv = args[++i];
    else if (a === '--key-id') opts.keyId = args[++i];
    else if (a === '--out') opts.out = args[++i];
    else if (a === '--help' || a === '-h') { process.exit(usage(0)); }
    else opts._.push(a);
  }
  return opts;
}

function fail(message) {
  console.error(`[sign-manifest] FAIL: ${message}`);
  process.exit(1);
}

// ---------------------------------------------------------------------------
// keygen
// ---------------------------------------------------------------------------
function cmdKeygen(opts) {
  const id = opts.id || DEFAULT_ID;
  if (!KEY_ID_RE.test(id)) fail(`非法 key_id "${id}"（须匹配 ${KEY_ID_RE}）`);
  const outDir = path.resolve(opts.outDir || DEFAULT_OUT_DIR);
  const pubPath = path.join(outDir, `${id}.pem`);
  const privPath = path.join(outDir, `${id}.private.pem`);

  for (const [p, what] of [[pubPath, '公钥'], [privPath, '私钥']]) {
    if (existsSync(p) && !opts.force) {
      fail(`${what}已存在: ${p}（换 --id 或加 --force 覆盖）`);
    }
  }

  const { publicKey, privateKey } = generateKeyPairSync('ed25519');
  const pubPem = publicKey.export({ type: 'spki', format: 'pem' });
  const privPem = privateKey.export({ type: 'pkcs8', format: 'pem' });

  mkdirSync(outDir, { recursive: true });
  writeFileSync(pubPath, pubPem);
  // 私钥：仅本用户可读（POSIX 下收紧；Windows 的 ACL 由目录权限兜底）
  writeFileSync(privPath, privPem, { mode: 0o600 });

  console.log(`[sign-manifest] keygen OK（key_id=${id}）`);
  console.log(`  公钥（可提交/随包分发）: ${pubPath}`);
  console.log(`  私钥（绝不入库）      : ${privPath}`);
}

// ---------------------------------------------------------------------------
// sign
// ---------------------------------------------------------------------------
function cmdSign(opts) {
  if (opts._.length !== 1) process.exit(usage(2));
  const manifestPath = path.resolve(opts._[0]);
  if (!existsSync(manifestPath)) fail(`manifest 不存在: ${manifestPath}`);
  if (opts.keyEnv && opts.key) fail('--key-env 与 --key 互斥（私钥来源二选一）');

  let privateKey;
  let keyId;
  let keyLabel;
  if (opts.keyEnv) {
    // CI 路径：私钥 PEM 从环境变量读入（如 GitHub secret RELEASE_MANIFEST_PRIVATE_KEY），
    // 不经磁盘文件；必须显式 --key-id（无法从环境变量名推断）
    if (!opts.keyId) fail(`--key-env ${opts.keyEnv} 需要显式 --key-id（环境变量无法推断 key_id）`);
    keyId = opts.keyId;
    const pem = process.env[opts.keyEnv];
    if (!pem || pem.trim() === '') fail(`环境变量 ${opts.keyEnv} 未设置或为空（--key-env 指定）`);
    keyLabel = `env:${opts.keyEnv}`;
    try {
      privateKey = createPrivateKey(Buffer.from(pem, 'utf8'));
    } catch (e) {
      fail(`私钥解析失败（来自环境变量 ${opts.keyEnv}，${e.message}）`);
    }
  } else {
    // 私钥路径 → key_id 推断（<id>.private.pem）
    const keyPath = opts.key ? path.resolve(opts.key) : path.join(DEFAULT_OUT_DIR, `${DEFAULT_ID}.private.pem`);
    keyId = opts.keyId || null;
    if (!keyId) {
      const base = path.basename(keyPath);
      const m = /^(.*)\.private\.pem$/.exec(base);
      keyId = m ? m[1] : DEFAULT_ID;
    }
    if (!existsSync(keyPath)) fail(`私钥不存在: ${keyPath}（先运行 keygen 子命令）`);
    keyLabel = keyPath;
    try {
      privateKey = createPrivateKey(readFileSync(keyPath));
    } catch (e) {
      fail(`私钥解析失败: ${keyPath}（${e.message}）`);
    }
  }
  if (!KEY_ID_RE.test(keyId)) fail(`非法 key_id "${keyId}"`);
  if (privateKey.asymmetricKeyType !== 'ed25519') {
    fail(`密钥类型不是 Ed25519: ${keyLabel}（${privateKey.asymmetricKeyType}）`);
  }

  const raw = readFileSync(manifestPath); // 原始字节——不改写、不重排、不加 BOM
  const sig = cryptoSign(null, raw, privateKey);
  if (sig.length !== 64) fail(`内部错误: 签名长度 ${sig.length} != 64`);

  const sigText = `${SIG_MAGIC} ${keyId}\n${sig.toString('base64')}\n`;
  const outPath = opts.out
    ? path.resolve(opts.out)
    : (manifestPath.toLowerCase().endsWith('.json')
        ? manifestPath.slice(0, -'.json'.length) + '.sig'
        : manifestPath + '.sig');
  writeFileSync(outPath, sigText, 'utf8');

  console.log(`[sign-manifest] sign OK: ${outPath}`);
  console.log(`  key_id=${keyId}  key=${keyLabel}  覆盖字节=${raw.length}  manifest=${manifestPath}`);
}

const args = process.argv.slice(2);
const cmd = args[0];
const opts = parseArgs(args.slice(1));
if (cmd === 'keygen') cmdKeygen(opts);
else if (cmd === 'sign') cmdSign(opts);
else process.exit(usage(cmd === undefined ? 0 : 2));
