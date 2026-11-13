import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';
import ts from 'typescript';

const source = readFileSync(new URL('../src/main.ts', import.meta.url), 'utf8').replace("import './style.css';", '');
const code = ts.transpileModule(source, { compilerOptions: { target: ts.ScriptTarget.ES2022 } }).outputText;
const settle = () => new Promise(resolve => setImmediate(resolve));
const page = offset => ({ offset, total: 250, entries: [{ word: '詞', reading: 'ㄘˊ', weight: null }], invalid_lines: 0 });

async function app() {
  const nodes = new Map();
  const node = () => ({ value: '', textContent: '', classList: { toggle() {} }, setAttribute() {}, removeAttribute() {}, replaceChildren() {}, append() {} });
  const el = id => { if (!nodes.has(id)) nodes.set(id, node()); return nodes.get(id); };
  const requests = [];
  let list = args => Promise.resolve(page(args.offset));
  const context = vm.createContext({
    document: { body: node(), getElementById: el, createElement: node },
    window: { __TAURI__: { core: { invoke(command, args) {
      if (command === 'lex_list') { requests.push(args); return list(args); }
      if (command === 'status') return Promise.resolve({ online: false, note: '' });
      if (command === 'get_settings') return Promise.resolve({ settings: {}, last_error: null, file_error: false });
      if (command === 'get_autocomp') return Promise.resolve({ list: false, pop: false, file_error: false });
      throw new Error(command);
    } } } },
    setInterval() {},
  });
  vm.runInContext(code, context);
  await settle();
  return { el, requests, offset: () => vm.runInContext('offset', context), setList(fn) { list = fn; } };
}

test('rapid pagination ignores busy clicks and commits only the reply offset', async () => {
  const ui = await app();
  let reply;
  ui.setList(() => new Promise(resolve => { reply = resolve; }));
  ui.el('next').onclick();
  ui.el('next').onclick();
  ui.el('prev').onclick();
  ui.el('search').oninput();
  assert.equal(ui.requests.length, 2);
  assert.equal(ui.requests.at(-1).offset, 50);
  assert.equal(ui.offset(), 0);
  reply(page(50));
  await settle();
  assert.equal(ui.offset(), 50);
  assert.match(ui.el('page').textContent, /第 2 頁/);
  ui.el('next').onclick();
  assert.equal(ui.requests.at(-1).offset, 100);
  // A reply may report an offset different from the requested one.
  reply(page(150));
  await settle();
  assert.equal(ui.offset(), 150);
  assert.match(ui.el('page').textContent, /第 4 頁/);
});

test('failed page and search requests preserve the displayed offset', async () => {
  const ui = await app();
  ui.el('next').onclick();
  await settle();
  assert.equal(ui.offset(), 50);
  ui.setList(() => Promise.reject(new Error('fake request failed')));
  ui.el('next').onclick();
  await settle();
  assert.equal(ui.offset(), 50);
  assert.match(ui.el('page').textContent, /第 2 頁/);
  ui.el('search').oninput();
  await settle();
  assert.equal(ui.requests.at(-1).offset, 0);
  assert.equal(ui.offset(), 50);
  ui.setList(args => Promise.resolve(page(args.offset)));
  ui.el('prev').onclick();
  await settle();
  assert.equal(ui.requests.at(-1).offset, 0);
  assert.equal(ui.offset(), 0);
});
