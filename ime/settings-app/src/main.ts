import './style.css';

type Settings = { toggle_key: string; default_mode: string; punct_half: boolean; symbol_table: boolean; cache: boolean };
type Entry = { word: string; reading: string; weight: number | null };
type SettingsReply = { settings: Settings; file_error: boolean; last_error: string | null };
type Page = { entries: Entry[]; total: number; offset: number; invalid_lines: number };
type Readings = { default: string | null; per_char: string[][] };
type Added = { added: number; updated: number; rejected: { line: number; reason: string }[] };
type ImportResult = { added: number; updated: number; rejected: number; details: { line: number; reason: string }[] };
declare global { interface Window { __TAURI__: { core: { invoke<T>(command: string, args?: Record<string, unknown>): Promise<T> } } } }
const invoke = <T>(command: string, args?: Record<string, unknown>) => window.__TAURI__.core.invoke<T>(command, args);
const el = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
const input = (id: string) => el<HTMLInputElement>(id);
const select = (id: string) => el<HTMLSelectElement>(id);
const button = (id: string) => el<HTMLButtonElement>(id);
let offset = 0, listGeneration = 0, readingGeneration = 0, selectedWord = '', busy = false;
let lastError: string | null = null;
let listQuery = '';
function message(text: string, failed = false) { el('message').textContent = text; el('message').classList.toggle('warning', failed); }
async function refreshStatus() {
  const status = await invoke<{ online: boolean; note: string }>('status');
  el('status').textContent = status.online ? '已連線引擎' : '離線編輯（引擎未執行，變更會在引擎啟動後生效）';
  el('status-note').textContent = status.note;
}
async function loadSettings() {
  const r = await invoke<SettingsReply>('get_settings');
  lastError = r.last_error;
  select('toggle-key').value = r.settings.toggle_key;
  select('default-mode').value = r.settings.default_mode;
  select('punctuation').value = r.settings.punct_half ? 'half' : 'full';
  select('symbol-table').value = r.settings.symbol_table ? 'on' : 'off';
  input('cache').checked = r.settings.cache;
  el('settings-warning').textContent = [r.file_error ? '設定檔有誤，目前顯示預設值；儲存可修復設定檔。' : '', lastError ?? ''].filter(Boolean).join(' ');
  button('save').disabled = false;
  await loadAutocomp();
}
type Autocomp = { list: boolean; pop: boolean; file_error: boolean };
function syncAutocomp() { input('autocomp-pop').disabled = !input('autocomp-list').checked; if (input('autocomp-pop').disabled) input('autocomp-pop').checked = false; }
async function loadAutocomp() {
  const a = await invoke<Autocomp>('get_autocomp');
  input('autocomp-list').checked = a.list; input('autocomp-pop').checked = a.pop; syncAutocomp();
  el('autocomp-warning').textContent = a.file_error ? '自動完成設定檔損毀，已忽略；儲存會重建。' : '';
  button('autocomp-apply').disabled = false; button('autocomp-save').disabled = false;
}
async function saveAutocomp(restart: boolean) {
  await invoke<Autocomp>('set_autocomp', { list: input('autocomp-list').checked, pop: input('autocomp-pop').checked });
  await loadAutocomp();
  if (!restart) { message('自動完成設定已儲存，下次登入後生效'); return; }
  message('自動完成設定已儲存，正在重新啟動引擎（最多約 20 秒）…');
  message(await invoke<string>('restart_engine')); await refreshStatus();
}
async function loadList(requestedOffset = offset): Promise<void> {
  const generation = ++listGeneration;
  listQuery = input('search').value;
  const r = await invoke<Page>('lex_list', { query: listQuery, offset: requestedOffset, limit: 50 });
  if (generation !== listGeneration) return;
  if (r.offset >= r.total && r.offset > 0) return loadList(Math.max(0, Math.ceil(r.total / 50) * 50 - 50));
  offset = r.offset;
  el('entries').replaceChildren();
  for (const e of r.entries) {
    const tr = document.createElement('tr');
    for (const value of [e.word, e.reading, e.weight === null ? '預設' : String(e.weight)]) { const td = document.createElement('td'); td.textContent = value; tr.append(td); }
    const td = document.createElement('td'), remove = document.createElement('button');
    remove.textContent = '刪除'; remove.setAttribute('aria-label', `刪除 ${e.word}（${e.reading}）`);
    remove.onclick = () => void run(async () => { await invoke('lex_remove', { word: e.word, reading: e.reading }); message(`已刪除「${e.word}」`); await refreshData(); });
    td.append(remove); tr.append(td); el('entries').append(tr);
  }
  if (r.entries.length === 0) { const tr = document.createElement('tr'), td = document.createElement('td'); td.colSpan = 4; td.textContent = '沒有符合的詞彙'; tr.append(td); el('entries').append(tr); }
  el('lex-warning').textContent = [`無效行數：${r.invalid_lines}`, lastError ?? ''].filter(Boolean).join('　');
  el('page').textContent = `第 ${Math.floor(offset / 50) + 1} 頁，共 ${r.total} 筆`;
  button('prev').disabled = offset === 0; button('next').disabled = offset + r.entries.length >= r.total;
}
async function refreshData() { await loadSettings(); await loadList(); }
async function run(action: () => Promise<void>) {
  if (busy) return;
  busy = true; document.body.setAttribute('aria-busy', 'true');
  try { await action(); } catch (e) { message(String(e), true); }
  finally {
    busy = false; document.body.removeAttribute('aria-busy');
    // Input events during a busy action are reconciled against its last list query.
    if (input('search').value !== listQuery) void run(() => loadList(0));
    try { await refreshStatus(); } catch (e) { message(String(e), true); }
  }
}
for (const tab of ['lex', 'settings']) button(`tab-${tab}`).onclick = () => {
  for (const name of ['lex', 'settings']) { el(name).hidden = name !== tab; button(`tab-${name}`).setAttribute('aria-pressed', String(name === tab)); }
  void run(async () => { if (tab === 'settings') await loadSettings(); else await refreshData(); });
};
input('search').oninput = () => { void run(() => loadList(0)); };
button('prev').onclick = () => void run(() => loadList(Math.max(0, offset - 50)));
button('next').onclick = () => void run(() => loadList(offset + 50));
// ---- adding a word: per-character readings, the 查讀音 shortcut and the virtual Bopomofo keyboard (USERLEX 1.6) ----
const INITIALS = 'ㄅㄆㄇㄈㄉㄊㄋㄌㄍㄎㄏㄐㄑㄒㄓㄔㄕㄖㄗㄘㄙ', MEDIALS = 'ㄧㄨㄩ', FINALS = 'ㄚㄛㄜㄝㄞㄟㄠㄡㄢㄣㄤㄥㄦ';
// Tone keys in keyboard order; the first tone has no mark.
const TONES: [string, string][] = [['', '一聲'], ['ˊ', 'ˊ'], ['ˇ', 'ˇ'], ['ˋ', 'ˋ'], ['˙', '˙']];
type Check = { syllables: { syllable: string; known: boolean; custom: boolean }[]; ok: boolean; reason: string | null; hint: string | null };
type Position = { char: string; input: HTMLInputElement; select: HTMLSelectElement | null; note: HTMLElement };
let positions: Position[] = [], active = 0, checkGeneration = 0, checkedOk = false;
/** The engine's spelling: first tone without a mark (ˉ dropped), neutral tone ˙ at the end (also when typed first).
 *  Inner spaces stay: they separate syllables, and the backend rejects a position with more than one. */
function normalizeSyllable(raw: string): string {
  let s = raw.trim().replace(/ˉ/g, '');
  if (s.startsWith('˙')) s = s.slice(1) + '˙';
  return s;
}
/** Splits a syllable into initial, medial, final and tone; anything else is dropped. */
function parts(syllable: string) {
  const p = { i: '', m: '', f: '', t: '' };
  for (const c of normalizeSyllable(syllable)) {
    if (INITIALS.includes(c)) p.i = c; else if (MEDIALS.includes(c)) p.m = c; else if (FINALS.includes(c)) p.f = c;
    else if (TONES.some(([mark]) => mark !== '' && mark === c)) p.t = c;
  }
  return p;
}
const compose = (p: { i: string; m: string; f: string; t: string }) => p.i + p.m + p.f + p.t;
const readingAt = (i: number) => normalizeSyllable(positions[i]?.input.value ?? '');
function setActive(i: number) {
  active = Math.max(0, Math.min(i, positions.length - 1));
  positions.forEach((p, k) => p.input.classList.toggle('active', k === active));
}
/** After any edit: the dropdown shows the matching dictionary reading or the empty 自訂 entry, then validation runs. */
function edited(i: number) {
  syncSelect(i);
  void validate();
}
function syncSelect(i: number) {
  const p = positions[i];
  if (p?.select) { const r = readingAt(i); p.select.value = Array.from(p.select.children).some(o => (o as HTMLOptionElement).value === r) ? r : ''; }
}
/** Programmatic change (dropdown, keyboard): the field shows the normalized spelling at once. */
function setReading(i: number, value: string) {
  const p = positions[i];
  if (!p) return;
  p.input.value = normalizeSyllable(value);
  edited(i);
}
/** One position per character; `options` / `defaults` from 查讀音 add a dropdown shortcut per character. `keep`: the
  *  readings typed meanwhile (non-empty ones), which a late 查讀音 reply must not overwrite. */
function buildPositions(word: string, options?: string[][], defaults?: string[], keep?: string[]) {
  positions = [];
  el('reading-selects').replaceChildren();
  Array.from(word).forEach((char, i) => {
    const label = document.createElement('label'), field = document.createElement('input') as HTMLInputElement;
    const note = document.createElement('span');
    let select: HTMLSelectElement | null = null;
    label.textContent = `${char} `;
    field.setAttribute('aria-label', `第 ${i + 1} 個字「${char}」的讀音`);
    // Typing keeps the raw text (a neutral tone typed first stays first while typing); validation and adding use the
    // normalized spelling, and the field shows it when it loses focus.
    field.oninput = () => edited(i);
    // The validated text is already the normalized one, so losing focus only updates what is shown.
    field.onchange = () => { field.value = normalizeSyllable(field.value); syncSelect(i); };
    field.onfocus = () => setActive(i);
    const d = defaults?.[i], own = options?.[i] ?? [];
    field.value = (keep?.[i] || undefined) ?? (d !== undefined && own.includes(d) ? d : (own[0] ?? ''));
    if (options) {
      const s = document.createElement('select') as HTMLSelectElement;
      s.setAttribute('aria-label', `第 ${i + 1} 個字「${char}」的常用讀音`);
      const blank = document.createElement('option') as HTMLOptionElement;
      blank.value = ''; blank.textContent = '（自訂）'; s.append(blank);
      for (const reading of own) { const option = document.createElement('option') as HTMLOptionElement; option.value = reading; option.textContent = reading; s.append(option); }
      s.onchange = () => { setActive(i); if (s.value !== '') setReading(i, s.value); };
      s.onfocus = () => setActive(i);
      select = s;
      label.append(s);
    }
    label.append(field, note);
    el('reading-selects').append(label);
    positions.push({ char, input: field, select, note });
    if (select) select.value = own.includes(normalizeSyllable(field.value)) ? normalizeSyllable(field.value) : '';
  });
  setActive(0);
}
/** Live validation; a reply for an older input never overwrites the result for a newer one. */
async function validate() {
  const generation = ++checkGeneration, word = selectedWord;
  checkedOk = false; button('add').disabled = true; el('reading-hint').textContent = '';
  const readings = positions.map((_, i) => readingAt(i));
  if (!word || readings.every(r => r === '')) { for (const p of positions) p.note.textContent = ''; return; }
  try {
    // Filled positions are checked at once; adding needs every position (`ok`).
    const r = await invoke<Check>('check_reading', { word, readings });
    if (generation !== checkGeneration) return;
    r.syllables.forEach((s, i) => {
      const p = positions[i];
      if (!p) return;
      p.note.textContent = s.syllable === '' ? '' : !s.known ? '不在音節表' : s.custom ? '自訂讀音' : '';
      p.note.classList.toggle('warning', !s.known);
    });
    el('reading-hint').textContent = r.ok ? (r.hint ?? '') : (r.reason ?? '');
    checkedOk = r.ok; button('add').disabled = !r.ok;
  } catch (e) {
    if (generation === checkGeneration) message(String(e), true);
  }
}
function keyboardPress(symbol: string, tone: boolean) {
  if (!positions[active]) return;
  const q = parts(readingAt(active));
  if (tone) q.t = symbol;
  else if (INITIALS.includes(symbol)) q.i = symbol; else if (MEDIALS.includes(symbol)) q.m = symbol; else q.f = symbol;
  setReading(active, compose(q));
  // A tone completes the syllable: continue with the next character.
  if (tone) setActive(active + 1);
}
function buildKeyboard() {
  const add = (text: string, label: string, onclick: () => void) => {
    const b = document.createElement('button') as HTMLButtonElement;
    b.type = 'button'; b.textContent = text; b.setAttribute('aria-label', label); b.onclick = onclick;
    el('keyboard').append(b);
  };
  el('keyboard').replaceChildren();
  for (const group of [INITIALS, MEDIALS, FINALS]) for (const c of group) add(c, c, () => keyboardPress(c, false));
  for (const [mark, text] of TONES) add(text, mark === '' ? '一聲' : `聲調 ${mark}`, () => keyboardPress(mark, true));
  add('⌫', '刪除目前讀音的最後一個符號', () => {
    if (positions[active]) setReading(active, Array.from(readingAt(active)).slice(0, -1).join(''));
  });
  add('清除', '清除目前讀音', () => setReading(active, ''));
}
buildKeyboard();
input('word').oninput = () => {
  readingGeneration++;
  const word = input('word').value.trim(), n = Array.from(word).length;
  selectedWord = n >= 1 && n <= 8 ? word : '';
  buildPositions(selectedWord);
  void validate();
};
button('readings').onclick = () => void run(async () => {
  const word = input('word').value.trim(), generation = ++readingGeneration;
  if (Array.from(word).length < 1 || Array.from(word).length > 8) throw '詞長必須為 1～8 個字';
  const r = await invoke<Readings>('reading_options', { word });
  if (generation !== readingGeneration) return;
  // Readings already typed for this word stay (also those typed while the reply was pending); empty positions take
  // the default; the dropdowns are added either way.
  const keep = word === selectedWord ? positions.map(p => p.input.value) : undefined;
  selectedWord = word;
  buildPositions(word, r.per_char, r.default?.split(/[\s-]+/) ?? [], keep);
  await validate();
  message('請選擇或輸入讀音後加入；可用下方的注音鍵盤輸入自訂讀音。');
});
el<HTMLFormElement>('add-form').onsubmit = event => { event.preventDefault(); void run(async () => {
  if (!selectedWord || selectedWord !== input('word').value.trim() || !checkedOk) throw '請先輸入完整且有效的讀音';
  const reading = positions.map((_, i) => readingAt(i)).join(' ');
  const raw = input('weight').value, weight = raw === '' ? null : Number(raw);
  if (weight !== null && (!Number.isFinite(weight) || weight < 0 || weight > 30)) throw '權重必須為 0～30';
  const r = await invoke<Added>('lex_add', { word: selectedWord, reading, weight });
  if (r.rejected.length) throw r.rejected.map(x => x.reason).join('；');
  message(`新增 ${r.added} 筆、更新 ${r.updated} 筆`); await refreshData();
}); };
button('import').onclick = () => void run(async () => {
  const r = await invoke<ImportResult | null>('import_file');
  if (r === null) { message('已取消匯入'); return; }
  message(`匯入完成：新增 ${r.added} 筆、更新 ${r.updated} 筆、拒絕 ${r.rejected} 行` + r.details.map(d => `\n第 ${d.line} 行：${d.reason}`).join(''), r.rejected > 0);
  await refreshData();
});
button('export').onclick = () => void run(async () => { const r = await invoke<string | null>('export_file'); message(r ?? '已取消匯出'); });
el<HTMLFormElement>('settings-form').onsubmit = event => { event.preventDefault(); void run(async () => {
  const settings: Settings = { toggle_key: select('toggle-key').value, default_mode: select('default-mode').value, punct_half: select('punctuation').value === 'half', symbol_table: select('symbol-table').value === 'on', cache: input('cache').checked };
  await invoke('set_settings', { settings }); await loadSettings(); message('輸入設定已儲存');
}); };
input('autocomp-list').onchange = syncAutocomp;
el<HTMLFormElement>('autocomp-form').onsubmit = event => { event.preventDefault(); void run(() => saveAutocomp(true)); };
button('autocomp-save').onclick = () => void run(() => saveAutocomp(false));
void run(refreshData);
// Connectivity is retried even when the user leaves the settings window open.
setInterval(() => { if (!busy) void refreshStatus().catch(() => {}); }, 5000);
