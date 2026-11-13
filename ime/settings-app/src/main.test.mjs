import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';
import ts from 'typescript';

const source = readFileSync(new URL('./main.ts', import.meta.url), 'utf8').replace("import './style.css';", '');
const code = ts.transpileModule(source, { compilerOptions: { target: ts.ScriptTarget.ES2022 } }).outputText;
const settle = () => new Promise(resolve => setImmediate(resolve));
const page = query => ({ offset: 0, total: 1, entries: [{ word: query, reading: 'ㄘˊ', weight: null }], invalid_lines: 0 });

async function app() {
  const nodes = new Map(), requests = [];
  const node = () => ({
    value: '', textContent: '', children: [], classList: { toggle() {} },
    setAttribute() {}, removeAttribute() {},
    replaceChildren(...children) { this.children = children; },
    append(...children) { this.children.push(...children); },
  });
  const el = id => { if (!nodes.has(id)) nodes.set(id, node()); return nodes.get(id); };
  let list = args => Promise.resolve(page(args.query));
  vm.runInNewContext(code, {
    document: { body: node(), getElementById: el, createElement: node },
    window: { __TAURI__: { core: { invoke(command, args) {
      if (command === 'lex_list') { requests.push(args); return list(args); }
      if (command === 'status') return Promise.resolve({ online: false, note: '' });
      if (command === 'get_autocomp') return Promise.resolve({ list: false, pop: false, file_error: false });
      if (command === 'get_settings') return Promise.resolve({ settings: {}, last_error: null, file_error: false });
      throw new Error(command);
    } } } },
    setInterval() {},
  });
  await settle();
  return { el, requests, setList(fn) { list = fn; } };
}

for (const fails of [false, true]) {
  test(`busy search replays AB after A ${fails ? 'fails' : 'replies'}`, async () => {
    const ui = await app(), pending = [];
    ui.setList(args => new Promise((resolve, reject) => pending.push({ args, resolve, reject })));
    ui.el('search').value = 'A';
    ui.el('search').oninput();
    ui.el('search').value = 'AB';
    ui.el('search').oninput();
    assert.equal(pending.length, 1);
    assert.equal(pending[0].args.query, 'A');
    if (fails) pending[0].reject(new Error('fake request failed'));
    else pending[0].resolve(page('A'));
    await settle();
    assert.equal(pending.length, 2);
    assert.equal(pending[1].args.query, 'AB');
    assert.equal(pending[1].args.offset, 0);
    pending[1].resolve(page('AB'));
    await settle();
    assert.equal(ui.el('search').value, 'AB');
    assert.equal(ui.el('entries').children[0].children[0].textContent, 'AB');
    assert.equal(ui.requests.length, 3);
  });
}

test('autocomplete: the popup switch needs the list switch', async () => {
  const ui = await app();
  assert.equal(ui.el('autocomp-pop').disabled, true);
  ui.el('autocomp-list').checked = true;
  ui.el('autocomp-list').onchange();
  assert.equal(ui.el('autocomp-pop').disabled, false);
  ui.el('autocomp-pop').checked = true;
  ui.el('autocomp-list').checked = false;
  ui.el('autocomp-list').onchange();
  assert.equal(ui.el('autocomp-pop').disabled, true);
  assert.equal(ui.el('autocomp-pop').checked, false);
});
