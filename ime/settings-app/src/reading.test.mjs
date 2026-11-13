// USERLEX 3.6 settings-app cases: tone normalisation, the virtual keyboard, an unknown syllable is rejected, a stale
// validation never overwrites a newer input. Run: node --test src/*.test.mjs scripts/*.test.mjs
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';
import ts from 'typescript';

const source = readFileSync(new URL('./main.ts', import.meta.url), 'utf8').replace("import './style.css';", '');
const code = ts.transpileModule(source, { compilerOptions: { target: ts.ScriptTarget.ES2022 } }).outputText;
const settle = () => new Promise(resolve => setImmediate(resolve));

async function app(check, options = () => Promise.reject(new Error("reading_options"))) {
  const nodes = new Map(), checks = [];
  const node = () => ({
    value: '', textContent: '', children: [], disabled: false, classes: new Set(),
    classList: { toggle(name, on) { this.owner.classes[on ? 'add' : 'delete'](name); } },
    setAttribute() {}, removeAttribute() {},
    replaceChildren(...children) { this.children = children; },
    append(...children) { this.children.push(...children); },
  });
  const make = () => { const n = node(); n.classList.owner = n; return n; };
  const el = id => { if (!nodes.has(id)) nodes.set(id, make()); return nodes.get(id); };
  const context = vm.createContext({
    document: { body: make(), getElementById: el, createElement: make },
    window: { __TAURI__: { core: { invoke(command, args) {
      if (command === "check_reading") { checks.push(args); return check(args); }
      if (command === "reading_options") return options(args);
      if (command === 'lex_list') return Promise.resolve({ offset: 0, total: 0, entries: [], invalid_lines: 0 });
      if (command === 'status') return Promise.resolve({ online: false, note: '' });
      if (command === 'get_settings') return Promise.resolve({ settings: {}, last_error: null, file_error: false });
      throw new Error(command);
    } } } },
    setInterval() {},
  });
  vm.runInContext(code, context);
  await settle();
  const key = text => el('keyboard').children.find(b => b.textContent === text);
  const fields = () => el('reading-selects').children.map(label => label.children.find(c => c.oninput));
  const notes = () => el('reading-selects').children.map(label => label.children.at(-1).textContent);
  return { el, checks, key, fields, notes, run: expr => vm.runInContext(expr, context) };
}
// Like the backend: a position holding more than one syllable is an error.
const known = args => args.readings.some(s => s.includes(' ')) ? Promise.reject('第 1 個讀音只能是一個音節') : Promise.resolve({
  syllables: args.readings.map(s => ({ syllable: s, known: s !== '' && s !== 'ㄅㄧㄤ', custom: s === 'ㄐㄧㄚ' })),
  ok: args.readings.every(s => s !== '' && s !== 'ㄅㄧㄤ'), reason: null, hint: null,
});

test('first tone without a mark and the neutral tone before or after give one spelling', async () => {
  const ui = await app(known);
  assert.equal(ui.run('normalizeSyllable("˙ㄉㄜ")'), 'ㄉㄜ˙');
  assert.equal(ui.run('normalizeSyllable("ㄉㄜ˙")'), 'ㄉㄜ˙');
  assert.equal(ui.run('normalizeSyllable("ㄇㄚˉ")'), 'ㄇㄚ');
  assert.equal(ui.run('normalizeSyllable(" ㄇㄚ ")'), 'ㄇㄚ');
  // The keyboard composes in initial / medial / final / tone order and a later key of the same group replaces it.
  ui.el('word').value = '地瓜';
  ui.el('word').oninput();
  for (const k of ['ㄉ', 'ㄧ', 'ˋ', 'ㄍ', 'ㄨ', 'ㄚ', '一聲']) ui.key(k).onclick();
  await settle();
  assert.deepEqual(ui.fields().map(f => f.value), ['ㄉㄧˋ', 'ㄍㄨㄚ']);
  assert.equal(ui.el('add').disabled, false);
  // Neutral tone typed through the keyboard ends with ˙, like the typed-first spelling.
  ui.fields()[1].onfocus();
  ui.key('˙').onclick();
  await settle();
  assert.equal(ui.fields()[1].value, 'ㄍㄨㄚ˙');
  ui.fields()[1].value = '˙ㄍㄨㄚ';
  ui.fields()[1].onchange();
  assert.equal(ui.fields()[1].value, 'ㄍㄨㄚ˙');
});

test('a syllable outside the syllable table is rejected and a custom pair is marked', async () => {
  const ui = await app(known);
  ui.el('word').value = '加油';
  ui.el('word').oninput();
  ui.fields()[0].value = 'ㄐㄧㄚ'; ui.fields()[0].oninput();
  ui.fields()[1].value = 'ㄅㄧㄤ'; ui.fields()[1].oninput();
  await settle();
  assert.deepEqual(ui.notes(), ['自訂讀音', '不在音節表']);
  assert.equal(ui.el('add').disabled, true);
});

test('an older validation reply never overwrites the result of a newer input', async () => {
  const pending = [];
  const ui = await app(args => new Promise(resolve => pending.push({ args, resolve })));
  ui.el('word').value = '字';
  ui.el('word').oninput();
  ui.fields()[0].value = 'ㄅㄧㄤ'; ui.fields()[0].oninput();   // older input: unknown syllable
  ui.fields()[0].value = 'ㄗˋ'; ui.fields()[0].oninput();      // newer input: fine
  assert.equal(pending.length, 2);
  pending[1].resolve(await known(pending[1].args));
  await settle();
  assert.equal(ui.el('add').disabled, false);
  pending[0].resolve(await known(pending[0].args));
  await settle();
  assert.equal(ui.el('add').disabled, false);
  assert.deepEqual(ui.notes(), ['']);
});

test('typing keeps the raw text while validation and adding use the normalized spelling', async () => {
  const ui = await app(known);
  ui.el('word').value = '的';
  ui.el('word').oninput();
  const f = ui.fields()[0];
  for (const raw of ['˙', '˙ㄉ', '˙ㄉㄜ']) { f.value = raw; f.oninput(); }
  await settle();
  assert.equal(f.value, '˙ㄉㄜ');                       // a neutral tone typed first stays first while typing
  assert.deepEqual(Array.from(ui.checks.at(-1).readings), ['ㄉㄜ˙']);        // the engine spelling is what gets validated
  f.onchange();
  assert.equal(f.value, 'ㄉㄜ˙');
});

test('keys of one group replace each other, backspace and clear edit the selected character', async () => {
  const ui = await app(known);
  ui.el('word').value = '破字';
  ui.el('word').oninput();
  for (const k of ['ㄅ', 'ㄆ', 'ㄧ', 'ㄨ', 'ㄚ', 'ㄛ', 'ˊ']) ui.key(k).onclick();
  assert.equal(ui.fields()[0].value, 'ㄆㄨㄛˊ');
  for (const k of ['ㄗ', 'ˋ']) ui.key(k).onclick();      // the tone moved the keyboard to the second character
  await settle();
  assert.deepEqual(ui.fields().map(f => f.value), ['ㄆㄨㄛˊ', 'ㄗˋ']);
  assert.equal(ui.el('add').disabled, false);
  ui.fields()[0].onfocus();
  ui.key('⌫').onclick();
  assert.equal(ui.fields()[0].value, 'ㄆㄨㄛ');
  ui.key('ˋ').onclick();                                   // another tone replaces the tone slot
  assert.equal(ui.fields()[0].value, 'ㄆㄨㄛˋ');
  ui.fields()[0].onfocus();
  ui.key('清除').onclick();
  await settle();
  assert.equal(ui.fields()[0].value, '');
  assert.equal(ui.el('add').disabled, true);
});

test('a late validation reply after clearing does not enable adding', async () => {
  const pending = [];
  const ui = await app(args => new Promise(resolve => pending.push({ args, resolve })));
  ui.el('word').value = '字';
  ui.el('word').oninput();
  ui.fields()[0].value = 'ㄗˋ'; ui.fields()[0].oninput();
  ui.key('清除').onclick();
  pending[0].resolve(await known(pending[0].args));
  await settle();
  assert.equal(ui.el('add').disabled, true);
});

test('a late 查讀音 reply keeps the readings typed meanwhile and still adds the dropdowns', async () => {
  let reply;
  const ui = await app(known, () => new Promise(resolve => { reply = resolve; }));
  ui.el('word').value = '加油';
  ui.el('word').oninput();
  ui.el('readings').onclick();
  ui.fields()[0].value = 'ㄐㄧㄚ'; ui.fields()[0].oninput();
  reply({ default: 'ㄐㄧㄚ ㄧㄡˊ', per_char: [['ㄐㄧㄚ'], ['ㄧㄡˊ']] });
  await settle();
  assert.deepEqual(ui.fields().map(f => f.value), ['ㄐㄧㄚ', 'ㄧㄡˊ']);   // untouched positions take the default
  assert.ok(ui.el('reading-selects').children.every(label => label.children.some(c => c.onchange && !c.oninput)));
});

test('a filled syllable is checked while other characters are still empty', async () => {
  const ui = await app(known);
  ui.el('word').value = '加油';
  ui.el('word').oninput();
  ui.fields()[0].value = 'ㄅㄧㄤ'; ui.fields()[0].oninput();
  await settle();
  assert.deepEqual(Array.from(ui.checks.at(-1).readings), ['ㄅㄧㄤ', '']);
  assert.deepEqual(ui.notes(), ['不在音節表', '']);
  assert.equal(ui.el('add').disabled, true);
});

test('查讀音 keeps readings typed before the query, a tone key replaces the tone, adding sends the engine spelling', async () => {
  const added = [];
  const ui = await app(known, () => Promise.resolve({ default: 'ㄐㄧㄚ ㄧㄡˊ', per_char: [['ㄐㄧㄚ'], ['ㄧㄡˊ']] }));
  ui.el('word').value = '加油';
  ui.el('word').oninput();
  ui.fields()[0].value = 'ㄐㄧㄚˇ'; ui.fields()[0].oninput();
  ui.el('readings').onclick();
  await settle(); await settle();
  assert.deepEqual(ui.fields().map(f => f.value), ['ㄐㄧㄚˇ', 'ㄧㄡˊ']);
  ui.fields()[0].onfocus();
  ui.key('ˋ').onclick();                                   // ˇ is replaced, nothing deleted first
  assert.equal(ui.fields()[0].value, 'ㄐㄧㄚˋ');
  ui.fields()[1].value = '˙ㄧㄡ'; ui.fields()[1].oninput(); // raw text, neutral tone first
  await settle();
  ui.run(`window.__TAURI__.core.invoke = (c, a) => c === 'lex_add' ? (globalThis.added.push(a), Promise.resolve({ added: 1, updated: 0, rejected: [] })) : c === 'lex_list' ? Promise.resolve({ offset: 0, total: 0, entries: [], invalid_lines: 0 }) : c === 'get_settings' ? Promise.resolve({ settings: {}, last_error: null, file_error: false }) : Promise.resolve({ online: false, note: '' })`);
  ui.run('globalThis').added = added;
  ui.el('add-form').onsubmit({ preventDefault() {} });
  await settle(); await settle();
  assert.equal(added.length, 1);
  assert.equal(added[0].reading, 'ㄐㄧㄚˋ ㄧㄡ˙');
});

test('losing focus after a passed check keeps adding enabled and sends no new check', async () => {
  const ui = await app(known);
  ui.el('word').value = '字';
  ui.el('word').oninput();
  ui.fields()[0].value = '˙ㄗ'; ui.fields()[0].oninput();
  await settle();
  const count = ui.checks.length;
  assert.equal(ui.el('add').disabled, false);
  ui.fields()[0].onchange();                               // the click on 加入 first blurs the field
  assert.equal(ui.fields()[0].value, 'ㄗ˙');
  assert.equal(ui.el('add').disabled, false);
  assert.equal(ui.checks.length, count);
});

test('inner spaces are kept so two syllables in one position are rejected', async () => {
  const ui = await app(known);
  ui.el('word').value = '有';
  ui.el('word').oninput();
  ui.fields()[0].value = 'ㄧ ㄡˇ'; ui.fields()[0].oninput();
  await settle();
  assert.deepEqual(Array.from(ui.checks.at(-1).readings), ['ㄧ ㄡˇ']);
  assert.equal(ui.el('add').disabled, true);
  assert.match(ui.el('message').textContent, /只能是一個音節/);
});
