#!/usr/bin/env node
// 从斗鱼网页端自动同步内置静态表情表。
//
// 用法（在 simple_live_core 根目录）：
//   node tools/generate_douyu_emoticons.mjs
//   node tools/generate_douyu_emoticons.mjs --check
//
// 生成目标：crates/slive-core/src/platforms/douyu/emoticon/static_emoticons.rs

import { readFile, writeFile } from 'node:fs/promises';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const OUTPUT = resolve(
  ROOT,
  'crates/slive-core/src/platforms/douyu/emoticon/static_emoticons.rs',
);
const TEMPLATE = `${OUTPUT}.tmpl`;
const ROOM_URL = process.env.DOUYU_EMOTICON_ROOM_URL ?? 'https://www.douyu.com/317422';
const USER_AGENT =
  'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36';

const checkOnly = process.argv.includes('--check');

function fail(message) {
  throw new Error(`[douyu-emoticons] ${message}`);
}

async function fetchText(url) {
  const response = await fetch(url, {
    headers: { 'user-agent': USER_AGENT, referer: 'https://www.douyu.com/' },
  });
  if (!response.ok) fail(`${url} -> HTTP ${response.status}`);
  return response.text();
}

/** 从房间 HTML 找当前 player-aside 的带哈希 bundle。 */
async function findBundleUrl() {
  const html = await fetchText(ROOM_URL);
  const direct = html.match(
    /https:\/\/shark2\.douyucdn\.cn\/front-publish\/live-next-player-aside-master\/js\/live-next-player-aside_[0-9a-z]+\.js/,
  );
  if (direct) return direct[0];

  // player-aside 的带哈希地址在页面某个 `__SERVICE_META_INFO__` 注册表里，
  // 注册表所在的 JS 不是固定文件名。逐个页面 JS 搜索，避免把某个 chunk
  // 的哈希写死到生成器里。
  const scripts = [
    ...html.matchAll(
      /https:\/\/shark2\.douyucdn\.cn\/front-publish\/[^"\\ ]+\.js/g,
    ),
  ].map((match) => match[0]);
  for (const scriptUrl of new Set(scripts)) {
    const script = await fetchText(scriptUrl);
    const found = script.match(
      /https:\/\/shark2\.douyucdn\.cn\/front-publish\/live-next-player-aside-master\/js\/live-next-player-aside_[0-9a-z]+\.js/,
    );
    if (found) return found[0];
  }
  fail('页面 JS 中找不到 player-aside bundle');
}

function moduleStart(source, id) {
  const escaped = id.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  const match = new RegExp(`(?:"${escaped}"|${escaped}):function\\(e,t,n\\)\\{`).exec(source);
  if (!match) fail(`bundle 中找不到模块 ${id}`);
  return match.index + match[0].length - 1;
}

function functionBody(source, id) {
  const start = moduleStart(source, id);
  let depth = 0;
  let quote = null;
  let escaped = false;
  for (let index = start; index < source.length; index++) {
    const ch = source[index];
    if (quote) {
      if (escaped) escaped = false;
      else if (ch === '\\') escaped = true;
      else if (ch === quote) quote = null;
      continue;
    }
    if (ch === '"' || ch === "'" || ch === '`') {
      quote = ch;
      continue;
    }
    if (ch === '{') depth++;
    else if (ch === '}') {
      depth--;
      if (depth === 0) return source.slice(start + 1, index);
    }
  }
  fail(`模块 ${id} 括号未闭合`);
}

/** 把 webpack 的 URL 模块和目录映射模块还原成可调用的虚拟文件系统。 */
function buildAssetResolver(source) {
  const assets = new Map();
  const assetRe =
    /(?:"([0-9a-z]+)"|([0-9a-z]+)):function\(e,t(?:,n)?\)\{e\.exports="(https:[^"]+)"\}/g;
  for (const match of source.matchAll(assetRe)) {
    assets.set(match[1] ?? match[2], match[3]);
  }

  const contexts = new Map();
  const e6b57 = functionBody(source, 'e6b57');
  const requiredIds = [...new Set([...e6b57.matchAll(/n\("([0-9a-z]+)"\)/g)].map((m) => m[1]))];
  for (const id of requiredIds) {
    let body;
    try {
      body = functionBody(source, id);
    } catch {
      continue;
    }
    const mapMatch = /var r=\{([^}]*)\}/.exec(body);
    if (!mapMatch || !mapMatch[1].includes('"./')) continue;
    const mapping = new Map();
    for (const match of mapMatch[1].matchAll(/"\.\/([^"]+)":"([^"]+)"/g)) {
      mapping.set(match[1], match[2]);
    }
    if (mapping.size) {
      contexts.set(id, (path) => {
        const assetId = mapping.get(path.replace(/^\.\//, ''));
        return assetId ? assets.get(assetId) ?? null : null;
      });
    }
  }

  const require = (id) => {
    if (contexts.has(id)) return contexts.get(id);
    if (assets.has(id)) return assets.get(id);
    return { a: '' };
  };
  require.d = (target, key, getter) =>
    Object.defineProperty(target, key, { get: getter, enumerable: true });
  require.n = (module) => module;
  require.r = () => {};
  require.o = (object, key) => Object.prototype.hasOwnProperty.call(object, key);
  return { require, e6b57 };
}

function extractEntries(source) {
  const { require, e6b57 } = buildAssetResolver(source);
  const exportsObject = {};
  // 网页端在此处读取 window.faceListConfig；无该变量时走默认内置表。
  globalThis.window = { faceListConfig: [] };
  new Function('e', 't', 'n', e6b57)({}, exportsObject, require);
  const raw = exportsObject.i ?? [];
  const seen = new Set();
  const list = [];
  for (const entry of raw) {
    const title = String(entry.title ?? '').trim();
    const url = String(entry.url ?? '');
    if (!title || !url.startsWith('https://') || seen.has(title)) continue;
    seen.add(title);
    list.push({ title, url });
  }
  if (list.length < 200) fail(`只解析到 ${list.length} 项，疑似 bundle 结构变化`);
  return list;
}

function render(list) {
  const entries = list
    .map((item) => `    (${JSON.stringify(item.title)}, ${JSON.stringify(item.url)}),`)
    .join('\n');
  return `//! 斗鱼网页端内置静态表情表（自动生成）。
//!
//! 数据来源：斗鱼当前 player-aside bundle 的 \`e6b57\` 模块。
//! 共 ${list.length} 项；重新生成：
//! \`node tools/generate_douyu_emoticons.mjs\`

pub const STATIC_EMOTICONS: &[(&str, &str)] = &[
${entries}
];
`;
}

const bundleUrl = await findBundleUrl();
const bundle = await fetchText(bundleUrl);
const list = extractEntries(bundle);
const generated = render(list);

if (checkOnly) {
  const current = await readFile(OUTPUT, 'utf8');
  if (current !== generated) {
    fail('静态表情表已过期，请运行 node tools/generate_douyu_emoticons.mjs');
  }
  console.log(`[douyu-emoticons] up to date (${list.length} entries)`);
} else {
  await writeFile(OUTPUT, generated);
  console.log(`[douyu-emoticons] ${list.length} entries <- ${bundleUrl}`);
}

// 模板文件保留给人工审阅；生成逻辑始终从远端 bundle 重新推导。
await readFile(TEMPLATE).catch(() => {});
