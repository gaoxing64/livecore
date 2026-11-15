#!/usr/bin/env node
// 从抖音公开表情接口同步内置静态表情表（运行时失败时的兜底）。
//
// 用法（在 livecore 根目录）：
//   node tools/generate_douyin_emoticons.mjs
//   node tools/generate_douyin_emoticons.mjs --check
//
// 生成目标：crates/slive-core/src/platforms/douyin/danmaku/emoticon.rs
// 只替换 `const DOUYIN_EMOTICONS` 数组段，其余代码与测试保持原样。

import { readFile, writeFile } from 'node:fs/promises';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const OUTPUT = resolve(
  ROOT,
  'crates/slive-core/src/platforms/douyin/danmaku/emoticon.rs',
);
const API = process.env.DOUYIN_EMOJI_URL ?? 'https://www.douyin.com/aweme/v1/web/emoji/list';
const USER_AGENT =
  'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/125.0.0.0 Safari/537.36 Edg/125.0.0.0';

const BEGIN = 'const DOUYIN_EMOTICONS: &[(&str, &str)] = &[';
const END = '];';
const checkOnly = process.argv.includes('--check');

function fail(message) {
  throw new Error(`[douyin-emoticons] ${message}`);
}

function normalizeUrl(raw) {
  const url = String(raw ?? '').trim();
  if (url.startsWith('https://')) return url;
  if (url.startsWith('http://')) return `https://${url.slice('http://'.length)}`;
  if (url.startsWith('//')) return `https:${url}`;
  return null;
}

function extractEntries(value) {
  if (value.status_code !== 0) fail(`接口返回 status_code=${value.status_code}`);
  const list = Array.isArray(value.emoji_list) ? value.emoji_list : [];
  const seen = new Set();
  const entries = [];
  for (const item of list) {
    if (Number(item.hide ?? 0) === 1) continue;
    const name = String(item.display_name ?? '').trim();
    if (!name.startsWith('[') || !name.endsWith(']') || name.length <= 2) continue;
    const url = normalizeUrl(item.emoji_url?.url_list?.[0]);
    if (!url || seen.has(name)) continue;
    seen.add(name);
    entries.push({ name, url });
  }
  if (entries.length < 100) fail(`只解析到 ${entries.length} 项，疑似接口结构变化`);
  return entries;
}

function renderArray(entries) {
  const rows = entries
    .map((item) => `    (${JSON.stringify(item.name)}, ${JSON.stringify(item.url)}),`)
    .join('\n');
  return `${BEGIN}\n${rows}\n${END}`;
}

const response = await fetch(API, {
  headers: { 'user-agent': USER_AGENT, referer: 'https://www.douyin.com/' },
});
if (!response.ok) fail(`${API} -> HTTP ${response.status}`);
const entries = extractEntries(await response.json());
const block = renderArray(entries);

const current = await readFile(OUTPUT, 'utf8');
const start = current.indexOf(BEGIN);
if (start < 0) fail(`找不到数组起点: ${BEGIN}`);
const end = current.indexOf(`\n${END}`, start);
if (end < 0) fail('找不到数组终点');
const updated = current.slice(0, start) + block + current.slice(end + 1 + END.length);

if (checkOnly) {
  // 接口每次都会重新签发 CDN 的 x-signature，URL 必然变化；--check 只比较
  // 「表情码集合与顺序」，用来发现平台增删表情，而不是制造无意义的 diff。
  const currentBlock = current.slice(start, end + 1 + END.length);
  const currentNames = [...currentBlock.matchAll(/\(\s*"((?:[^"\\]|\\.)*)"\s*,/g)].map(
    (match) => JSON.parse(`"${match[1]}"`),
  );
  const expectedNames = entries.map((item) => item.name);
  if (currentNames.join('\u0000') !== expectedNames.join('\u0000')) {
    fail('静态表情表已过期，请运行 node tools/generate_douyin_emoticons.mjs');
  }
  console.log(
    `[douyin-emoticons] codes up to date (${entries.length} entries; signed URLs not compared)`,
  );
} else {
  await writeFile(OUTPUT, updated);
  console.log(`[douyin-emoticons] ${entries.length} entries <- ${API}`);
}
