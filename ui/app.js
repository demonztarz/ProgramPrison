'use strict';
const api = window.prison;
const $ = (id) => document.getElementById(id);

let state = null;
let selectedId = null;
let tab = 'sites';
let schedule = null; // local copy while painting the grid
let lockedShown = null; // id|locked the panels were last drawn for

const DAYS = [['Mon', 1], ['Tue', 2], ['Wed', 3], ['Thu', 4], ['Fri', 5], ['Sat', 6], ['Sun', 0]];

// ---------- helpers ----------

function el(tag, props = {}, ...kids) {
  const e = document.createElement(tag);
  for (const [k, v] of Object.entries(props)) {
    if (k === 'class') e.className = v;
    else if (k === 'text') e.textContent = v;
    else if (k.startsWith('on')) e.addEventListener(k.slice(2), v);
    else if (v === true) e.setAttribute(k, '');
    else if (v !== false && v != null) e.setAttribute(k, v);
  }
  for (const k of kids) if (k != null) e.append(k);
  return e;
}

function sel() {
  return state && state.blocks.find((b) => b.id === selectedId);
}

let toastTimer = 0;
function toast(msg, bad) {
  const t = $('toast');
  t.textContent = msg;
  t.className = 'toast' + (bad ? ' bad' : '');
  t.hidden = false;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => { t.hidden = true; }, bad ? 5000 : 2500);
}

function fmtLeft(ms) {
  const s = Math.max(0, Math.ceil(ms / 1000));
  const d = Math.floor(s / 86400);
  const h = Math.floor((s % 86400) / 3600);
  const m = Math.floor((s % 3600) / 60);
  const sec = s % 60;
  if (d) return `${d} d ${h} h`;
  if (h) return `${h}:${String(m).padStart(2, '0')}:${String(sec).padStart(2, '0')}`;
  return `${m}:${String(sec).padStart(2, '0')}`;
}

function fmtTime(t) {
  const d = new Date(t);
  const today = new Date();
  const time = d.toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' });
  if (d.toDateString() === today.toDateString()) return time;
  return d.toLocaleDateString([], { weekday: 'short', month: 'short', day: 'numeric' }) + ' ' + time;
}

function statusText(b) {
  const s = b.status;
  if (s.active) {
    const left = s.forever ? 'always on' : `${fmtLeft(s.until - Date.now())} left`;
    return { cls: s.locked ? 'locked' : 'on', text: s.locked ? `Locked · ${left}` : `On · ${left}` };
  }
  if (s.unlockedUntil) return { cls: 'off', text: `Off · skipping schedule until ${fmtTime(s.unlockedUntil)}` };
  return { cls: 'off', text: s.scheduled ? 'Off · scheduled' : 'Off' };
}

async function mutate(promise, okMsg) {
  const r = await promise;
  if (r && r.state) setState(r.state);
  if (r && r.error) toast(r.error, true);
  else if (okMsg) toast(okMsg);
  return r || {};
}

function update(changes) {
  const b = sel();
  return mutate(api.updateBlock({ id: b.id, ...changes }));
}

function setState(s) {
  state = s;
  if (!state.blocks.some((b) => b.id === selectedId)) selectedId = state.blocks.length ? state.blocks[0].id : null;
  render();
}

// ---------- rendering ----------

function render() {
  renderSidebar();
  renderBanner();
  const b = sel();
  $('empty').hidden = !!b;
  $('editor').hidden = !b;
  if (!b) return;
  if (document.activeElement !== $('blockName')) $('blockName').value = b.name;
  renderStatus();
  renderSites(b);
  renderApps(b);
  if (!painting) renderGrid(b);
  const key = `${b.id}|${b.status.locked}|${b.lock.type}`;
  if (key !== lockedShown) { lockedShown = key; renderLock(b); }
  $('sitesCount').textContent = b.sites.length || '';
  $('appsCount').textContent = b.apps.length || '';
  const hours = b.schedule.filter(Boolean).length;
  $('schedCount').textContent = hours ? `${hours} h` : '';
  $('lockCount').textContent = { none: '', random: 'text', password: 'password', locked: 'locked' }[b.lock.type];
}

function renderSidebar() {
  const list = $('blockList');
  list.replaceChildren(...state.blocks.map((b) => {
    const st = statusText(b);
    return el('li', { class: 'block-item' + (b.id === selectedId ? ' selected' : ''), 'data-id': b.id, onclick: () => { selectedId = b.id; render(); } },
      el('span', { class: 'dot ' + st.cls }),
      el('span', { class: 'block-text' }, el('span', { class: 'block-name', text: b.name }), el('span', { class: 'block-sub', text: st.text })));
  }));
  const n = state.closedToday;
  $('statsLine').textContent = n ? `${n} program${n === 1 ? '' : 's'} closed today` : '';
}

// Countdown text changes every second; update it in place so clicks aren't lost.
function renderTimes() {
  if (!state) return;
  for (const li of $('blockList').children) {
    const b = state.blocks.find((x) => x.id === li.dataset.id);
    if (!b) continue;
    const st = statusText(b);
    li.querySelector('.block-sub').textContent = st.text;
    li.querySelector('.dot').className = 'dot ' + st.cls;
  }
  if (sel()) renderStatus();
}

function renderBanner() {
  const h = state.hosts;
  const needsSites = state.blocks.some((b) => b.sites.length);
  const show = h.needsSetup || (!h.writable && needsSites);
  $('hostsBanner').hidden = !show;
  if (show) {
    $('hostsBannerText').textContent = h.needsSetup && state.blocks.some((b) => b.status.active && b.sites.length)
      ? "Websites aren't being blocked yet: ProgramPrison needs permission to edit the system hosts file, once."
      : 'Website blocking needs a one-time permission to edit the system hosts file.';
  }
}

function renderStatus() {
  const b = sel();
  const st = statusText(b);
  const pill = $('statusPill');
  pill.textContent = st.text;
  pill.className = 'pill ' + st.cls;
  $('startBtn').textContent = b.status.active ? 'Extend' : 'Start';
  $('stopBtn').hidden = !b.status.active;
  $('stopBtn').disabled = b.status.locked && b.lock.type === 'locked';
  $('deleteBtn').disabled = b.status.locked;
  const note = $('lockNote');
  if (b.status.locked) {
    note.hidden = false;
    note.textContent = b.lock.type === 'locked'
      ? 'Locked. You can add websites, programs and hours, but nothing can be removed or stopped until it ends.'
      : 'Locked. You can add websites, programs and hours, but removing anything needs the unlock ' + (b.lock.type === 'password' ? 'password.' : 'text.');
  } else note.hidden = true;
}

function renderSites(b) {
  const locked = b.status.locked;
  $('siteList').replaceChildren(...(b.sites.length ? b.sites.map((s) => el('li', {},
    el('span', { class: 'item-name', text: s }),
    el('button', { class: 'icon-btn', title: locked ? 'Locked' : 'Remove', 'aria-label': `Remove ${s}`, disabled: locked, text: '×', onclick: () => update({ sites: b.sites.filter((x) => x !== s) }) }),
  )) : [el('li', { class: 'empty-row', text: 'No websites yet.' })]));
  const chips = state.presets.map((p) => {
    const missing = p.sites.filter((s) => !b.sites.includes(s));
    return el('button', { class: 'chip', type: 'button', disabled: !missing.length, title: p.sites.join(', '), text: (missing.length ? '+ ' : '✓ ') + p.name,
      onclick: () => update({ sites: b.sites.concat(missing) }) });
  });
  $('presetChips').replaceChildren(...chips);
}

function renderApps(b) {
  const locked = b.status.locked;
  $('appList').replaceChildren(...(b.apps.length ? b.apps.map((a) => {
    const select = el('select', { class: 'mode', 'aria-label': `What to do with ${a.name}` },
      el('option', { value: 'close', text: 'Close it' }),
      el('option', { value: 'cover', text: state.coverSupported ? 'Cover its windows' : 'Cover (Windows only)', disabled: !state.coverSupported || (locked && a.mode === 'close') }));
    select.value = a.mode;
    select.addEventListener('change', () => update({ apps: b.apps.map((x) => (x.name === a.name ? { ...x, mode: select.value } : x)) }));
    return el('li', {},
      el('span', { class: 'item-name', text: a.name }),
      select,
      el('button', { class: 'icon-btn', title: locked ? 'Locked' : 'Remove', 'aria-label': `Remove ${a.name}`, disabled: locked, text: '×', onclick: () => update({ apps: b.apps.filter((x) => x.name !== a.name) }) }));
  }) : [el('li', { class: 'empty-row', text: 'No programs yet.' })]));
}

// ---------- schedule grid ----------

let painting = null; // { value }

function buildGrid() {
  const g = $('grid');
  g.append(el('div', { class: 'corner' }));
  for (let h = 0; h < 24; h++) g.append(el('div', { class: 'hour', text: h % 3 === 0 ? (h === 0 ? '12a' : h < 12 ? `${h}a` : h === 12 ? '12p' : `${h - 12}p`) : '' }));
  for (const [label, jsDay] of DAYS) {
    g.append(el('div', { class: 'day', text: label }));
    for (let h = 0; h < 24; h++) g.append(el('div', { class: 'cell', 'data-slot': jsDay * 24 + h, title: `${label} ${h}:00–${h + 1}:00` }));
  }
  g.addEventListener('pointerdown', (e) => {
    const c = e.target.closest('.cell');
    if (!c) return;
    e.preventDefault();
    const b = sel();
    schedule = b.schedule.slice();
    const slot = Number(c.dataset.slot);
    const value = !schedule[slot];
    if (!value && b.status.locked) { toast('The block is locked: hours can be added but not removed.', true); return; }
    painting = { value };
    paint(slot);
  });
  g.addEventListener('pointerover', (e) => {
    if (!painting) return;
    const c = e.target.closest('.cell');
    if (c) paint(Number(c.dataset.slot));
  });
  window.addEventListener('pointerup', () => {
    if (!painting) return;
    painting = null;
    update({ schedule });
  });
}

function paint(slot) {
  schedule[slot] = painting.value;
  $('grid').querySelector(`[data-slot="${slot}"]`).classList.toggle('on', painting.value);
}

function renderGrid(b) {
  const now = new Date();
  const cur = now.getDay() * 24 + now.getHours();
  for (const c of $('grid').querySelectorAll('.cell')) {
    const slot = Number(c.dataset.slot);
    c.classList.toggle('on', b.schedule[slot]);
    c.classList.toggle('now', slot === cur);
  }
  $('schedClear').disabled = b.status.locked;
}

function addSlots(fn) {
  const b = sel();
  const s = b.schedule.slice();
  for (let i = 0; i < s.length; i++) if (fn(Math.floor(i / 24), i % 24)) s[i] = true;
  update({ schedule: s });
}

// ---------- lock panel ----------

function renderLock(b) {
  const locked = b.status.locked;
  for (const r of document.querySelectorAll('input[name=lock]')) {
    r.checked = r.value === b.lock.type;
    r.disabled = locked && r.value !== 'locked' && r.value !== b.lock.type;
  }
  $('randomLength').value = b.lock.type === 'random' ? b.lock.length : 200;
  $('passwordInput').value = '';
  $('passwordInput').placeholder = b.lock.type === 'password' ? 'Leave empty to keep the current password' : 'New password';
  $('passwordInput').disabled = locked;
  $('randomLength').disabled = false;
}

async function saveLock() {
  const b = sel();
  const type = (document.querySelector('input[name=lock]:checked') || {}).value;
  if (type === 'locked' && !b.status.locked && !confirm('With "Locked" there is no way to stop this block early, not even by quitting or restarting. Continue?')) return;
  const r = await mutate(api.setLock({ id: b.id, type, length: Number($('randomLength').value), password: $('passwordInput').value }), 'Lock saved.');
  if (!r.error) { lockedShown = null; render(); }
}

// ---------- start / stop ----------

function localInput(t) {
  const d = new Date(t);
  d.setSeconds(0, 0);
  return new Date(d.getTime() - d.getTimezoneOffset() * 60000).toISOString().slice(0, 16);
}

function startEnd() {
  const mode = document.querySelector('input[name=startMode]:checked').value;
  if (mode === 'until') {
    const v = $('untilInput').value;
    return v ? new Date(v).getTime() : NaN;
  }
  const mins = (Number($('customH').value) || 0) * 60 + (Number($('customM').value) || 0);
  return Date.now() + mins * 60000;
}

function lockSummary(b) {
  if (b.lock.type === 'none') return 'No lock: you can stop it any time.';
  if (b.lock.type === 'random') return `Stopping early means typing ${b.lock.length} random characters.`;
  if (b.lock.type === 'password') return 'Stopping early needs the password.';
  return "Locked: it can't be stopped early, at all.";
}

function updateStartSummary() {
  const b = sel();
  const end = startEnd();
  const parts = [];
  if (b.sites.length) parts.push(`${b.sites.length} website${b.sites.length > 1 ? 's' : ''}`);
  if (b.apps.length) parts.push(`${b.apps.length} program${b.apps.length > 1 ? 's' : ''}`);
  const what = parts.length ? parts.join(' and ') : 'nothing yet (add websites or programs first)';
  $('startSummary').textContent = Number.isFinite(end) && end > Date.now()
    ? `Blocks ${what} until ${fmtTime(end)} (${fmtLeft(end - Date.now())}). ${lockSummary(b)}`
    : 'Pick when the block should end.';
  $('startError').textContent = '';
}

function openStart() {
  const b = sel();
  $('startName').textContent = `“${b.name}”`;
  $('untilInput').value = localInput(Date.now() + 3600000);
  $('untilInput').min = localInput(Date.now());
  updateStartSummary();
  $('startDialog').showModal();
}

async function openStop() {
  const b = sel();
  $('stopName').textContent = `“${b.name}”`;
  $('stopError').textContent = '';
  const body = $('stopBody');
  const ch = await api.challenge(b.id);
  $('stopConfirm').hidden = false;
  stopType = ch.type;
  if (ch.type === 'none') {
    body.replaceChildren(el('p', { text: b.sites.length || b.apps.length ? 'The websites and programs in this block will be allowed again.' : '' }));
  } else if (ch.type === 'locked') {
    body.replaceChildren(el('p', { text: `This block is locked${ch.until && Number.isFinite(ch.until) ? ` until ${fmtTime(ch.until)}` : ''}. It can't be stopped early.` }));
    $('stopConfirm').hidden = true;
  } else if (ch.type === 'password') {
    body.replaceChildren(el('p', { text: 'Enter the unlock password.' }), el('input', { id: 'stopAnswer', type: 'password', autocomplete: 'off' }));
  } else if (ch.type === 'random') {
    const area = el('textarea', { id: 'stopAnswer', rows: 5, spellcheck: 'false', autocomplete: 'off', placeholder: 'Type the text above exactly' });
    for (const ev of ['paste', 'drop', 'copy', 'cut', 'contextmenu']) area.addEventListener(ev, (e) => e.preventDefault());
    const counter = el('div', { class: 'muted small', text: `0 / ${ch.text.length}` });
    area.addEventListener('input', () => { counter.textContent = `${area.value.replace(/\s+/g, '').length} / ${ch.text.length}`; });
    body.replaceChildren(
      el('p', { text: `To stop early, type these ${ch.text.length} characters exactly. Upper and lower case count.` }),
      el('div', { class: 'challenge', text: ch.text.replace(/(.{10})/g, '$1 ').trim() }),
      area, counter);
  } else {
    $('stopDialog').close();
    return;
  }
  if (!$('stopDialog').open) $('stopDialog').showModal();
  const a = $('stopAnswer');
  if (a) a.focus();
}
let stopType = null;

// ---------- settings ----------

function openSettings() {
  $('startAtLogin').checked = state.settings.startAtLogin;
  const h = state.hosts;
  $('hostsInfo').textContent = h.writable
    ? `Ready. ProgramPrison can edit ${h.path}.`
    : `Not set up. ProgramPrison needs permission to edit ${h.path}. You'll be asked for an administrator password once.`;
  $('settingsHostsSetup').hidden = h.writable;
  $('settingsError').textContent = '';
  $('versionLine').textContent = `ProgramPrison ${state.version}`;
  $('settingsDialog').showModal();
}

async function setupHosts(btn) {
  btn.disabled = true;
  const old = btn.textContent;
  btn.textContent = 'Waiting for permission…';
  const r = await mutate(api.setupHosts(), 'Website blocking is ready.');
  btn.disabled = false;
  btn.textContent = old;
  if ($('settingsDialog').open) { $('settingsError').textContent = r.error || ''; if (!r.error) openSettings(); }
}

// ---------- wiring ----------

function wire() {
  for (const t of document.querySelectorAll('.tab')) {
    t.addEventListener('click', () => {
      tab = t.dataset.tab;
      for (const x of document.querySelectorAll('.tab')) x.classList.toggle('active', x === t);
      for (const p of document.querySelectorAll('.panel')) p.hidden = p.dataset.panel !== tab;
      if (tab === 'apps') loadRunning();
    });
  }
  document.querySelector('.tab[data-tab=sites]').classList.add('active');

  const newBlock = async () => {
    const r = await mutate(api.createBlock());
    if (r.id) { selectedId = r.id; render(); $('blockName').select(); }
  };
  $('newBlock').addEventListener('click', newBlock);
  $('emptyNew').addEventListener('click', newBlock);

  $('blockName').addEventListener('change', () => update({ name: $('blockName').value }));
  $('blockName').addEventListener('keydown', (e) => { if (e.key === 'Enter') $('blockName').blur(); });

  $('siteForm').addEventListener('submit', (e) => {
    e.preventDefault();
    const b = sel();
    const raw = $('siteInput').value.split(/[\s,]+/).filter(Boolean);
    if (!raw.length) return;
    update({ sites: b.sites.concat(raw) }).then((r) => {
      if (r.error) return;
      const nb = sel();
      const bad = raw.filter((x) => !nb.sites.length || !isValidSite(x));
      if (bad.length) toast(`Not a website address: ${bad.join(', ')}`, true);
      $('siteInput').value = bad.join(' ');
    });
  });

  $('appForm').addEventListener('submit', (e) => {
    e.preventDefault();
    addApp($('appInput').value);
  });
  $('runningSelect').addEventListener('change', () => {
    if ($('runningSelect').value) addApp($('runningSelect').value);
    $('runningSelect').value = '';
  });
  $('refreshRunning').addEventListener('click', loadRunning);

  $('schedWork').addEventListener('click', () => addSlots((d, h) => d >= 1 && d <= 5 && h >= 9 && h < 17));
  $('schedEvening').addEventListener('click', () => addSlots((_d, h) => h >= 21 || h < 7));
  $('schedClear').addEventListener('click', () => update({ schedule: new Array(168).fill(false) }));

  $('saveLock').addEventListener('click', saveLock);
  for (const r of document.querySelectorAll('input[name=lock]')) r.addEventListener('change', () => {
    if (r.value === 'password') $('passwordInput').focus();
  });

  $('startBtn').addEventListener('click', openStart);
  for (const c of document.querySelectorAll('#durationChips button')) {
    c.addEventListener('click', () => {
      const m = Number(c.dataset.min);
      $('customH').value = Math.floor(m / 60);
      $('customM').value = m % 60;
      document.querySelector('input[name=startMode][value=for]').checked = true;
      updateStartSummary();
    });
  }
  for (const id of ['customH', 'customM']) $(id).addEventListener('input', () => { document.querySelector('input[name=startMode][value=for]').checked = true; updateStartSummary(); });
  $('untilInput').addEventListener('input', () => { document.querySelector('input[name=startMode][value=until]').checked = true; updateStartSummary(); });
  for (const r of document.querySelectorAll('input[name=startMode]')) r.addEventListener('change', updateStartSummary);
  $('startCancel').addEventListener('click', () => $('startDialog').close());
  $('startForm').addEventListener('submit', async (e) => {
    e.preventDefault();
    const b = sel();
    const end = startEnd();
    if (b.lock.type === 'locked' && end - Date.now() > 24 * 3600000 && !confirm(`This locked block will run until ${fmtTime(end)} and can't be stopped. Start it?`)) return;
    const r = await api.startBlock(b.id, end);
    if (r.state) setState(r.state);
    if (r.error) { $('startError').textContent = r.error; return; }
    $('startDialog').close();
    toast(`“${b.name}” is on.`);
  });

  $('stopBtn').addEventListener('click', openStop);
  $('stopCancel').addEventListener('click', () => $('stopDialog').close());
  $('stopForm').addEventListener('submit', async (e) => {
    e.preventDefault();
    const b = sel();
    const a = $('stopAnswer');
    const r = await api.stopBlock(b.id, a ? a.value : '');
    if (r.state) setState(r.state);
    if (r.error) {
      if (stopType === 'random') await openStop(); // a new text each try
      $('stopError').textContent = r.error;
      return;
    }
    $('stopDialog').close();
    toast(`“${b.name}” stopped.`);
  });

  $('deleteBtn').addEventListener('click', async () => {
    const b = sel();
    if (!confirm(`Delete “${b.name}”?`)) return;
    await mutate(api.deleteBlock(b.id), 'Block deleted.');
  });

  $('openSettings').addEventListener('click', openSettings);
  $('settingsClose').addEventListener('click', () => $('settingsDialog').close());
  $('startAtLogin').addEventListener('change', async () => {
    const r = await mutate(api.updateSettings({ startAtLogin: $('startAtLogin').checked }));
    if (r.error) { $('startAtLogin').checked = true; $('settingsError').textContent = r.error; }
  });
  $('hostsSetup').addEventListener('click', (e) => setupHosts(e.currentTarget));
  $('settingsHostsSetup').addEventListener('click', (e) => setupHosts(e.currentTarget));
}

function isValidSite(s) {
  const b = sel();
  const norm = String(s).trim().toLowerCase().replace(/^[a-z]+:\/\//, '').split(/[/?#]/)[0].replace(/^www\./, '').replace(/:\d+$/, '');
  return b.sites.includes(norm);
}

function addApp(name) {
  const b = sel();
  name = String(name || '').trim();
  if (!name) return;
  update({ apps: b.apps.concat([{ name, mode: 'close' }]) }).then((r) => {
    if (r.error) return;
    const key = name.split(/[\\/]/).pop().toLowerCase().replace(/\.(exe|app)$/, '').trim();
    if (!sel().apps.some((a) => a.name === key)) toast(`“${name}” can't be blocked: it's part of the system or of ProgramPrison.`, true);
    else $('appInput').value = '';
  });
}

let runningLoaded = 0;
async function loadRunning() {
  if (Date.now() - runningLoaded < 5000) return;
  runningLoaded = Date.now();
  const names = await api.runningApps();
  const s = $('runningSelect');
  s.replaceChildren(el('option', { value: '', text: "Or pick a program that's running now…" }), ...names.map((n) => el('option', { value: n, text: n })));
}

buildGrid();
wire();
// The main process sends state every few seconds while the window is open; only redraw when
// something actually changed, so open dropdowns and hover states aren't disturbed.
let lastKey = '';
api.onState((s) => {
  const key = JSON.stringify({ ...s, now: 0 });
  if (key === lastKey) return;
  lastKey = key;
  setState(s);
});
api.getState().then(setState);
setInterval(renderTimes, 1000);
