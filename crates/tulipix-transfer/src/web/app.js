// The phone side. Three views behind one cookie: PIN, download, upload.
//
// The session token lives in an HttpOnly cookie the server sets, so there is
// nothing here to store, clear or leak — a 401 from any call is the single
// signal that we are back to the PIN form.
//
// Both lists work the same way: tick rows, then press the one button at the
// bottom. Nothing moves on the tap that selects it — picking a file used to
// start its upload immediately, which made a mis-tap unrecoverable.

const $ = (id) => document.getElementById(id);

const views = { pin: $('viewPin'), files: $('viewFiles'), upload: $('viewUpload') };
let view = 'pin';
let poll = null;

function show(next) {
  view = next;
  for (const [name, el] of Object.entries(views)) el.hidden = name !== next;
  const paired = next !== 'pin';
  $('tabs').hidden = !paired;
  $('chip').hidden = !paired;
  $('addr').hidden = !paired;
  $('themeBtn').hidden = !paired;
  for (const tab of document.querySelectorAll('.tab')) {
    tab.setAttribute('aria-selected', String(tab.dataset.view === next));
  }
  syncBars();
  if (next === 'pin') stopPolling(); else startPolling();
}

function bytes(n) {
  if (n < 1024) return n + ' B';
  const units = ['KB', 'MB', 'GB', 'TB'];
  let v = n / 1024, i = 0;
  while (v >= 1024 && i < units.length - 1) { v /= 1024; i++; }
  return (v < 10 ? v.toFixed(1) : Math.round(v)) + ' ' + units[i];
}

const plural = (n, word) => `${n} ${word}${n === 1 ? '' : 's'}`;

// ---- theme ------------------------------------------------------------------
// The phone's own setting is the default; the toggle writes an override that
// beats it in both directions and survives a reload.

const themeBtn = $('themeBtn');

function systemDark() {
  return window.matchMedia('(prefers-color-scheme: dark)').matches;
}

function applyTheme(mode) {
  if (mode === 'light' || mode === 'dark') {
    document.documentElement.setAttribute('data-theme', mode);
  } else {
    document.documentElement.removeAttribute('data-theme');
  }
  const dark = mode ? mode === 'dark' : systemDark();
  // The glyph advertises where the button goes, not where we are.
  themeBtn.dataset.next = dark ? 'light' : 'dark';
}

let theme = null;
try { theme = localStorage.getItem('tulipix-theme'); } catch { /* private mode */ }
applyTheme(theme);

themeBtn.addEventListener('click', () => {
  const dark = theme ? theme === 'dark' : systemDark();
  theme = dark ? 'light' : 'dark';
  try { localStorage.setItem('tulipix-theme', theme); } catch { /* private mode */ }
  applyTheme(theme);
});

// The address bar in the header: whatever host this page was actually fetched
// from, which is the desktop's address by definition.
$('addr').textContent = location.host;

// ---- PIN --------------------------------------------------------------------

const boxes = $('pinBoxes');
const note = $('pinNote');
let entered = '';
let busy = false;

function drawPin() {
  boxes.replaceChildren();
  for (let i = 0; i < 6; i++) {
    const b = document.createElement('div');
    b.className = 'pin-box';
    if (entered[i]) { b.textContent = entered[i]; b.classList.add('filled'); }
    else if (i === entered.length) { b.classList.add('active'); }
    boxes.appendChild(b);
  }
}

function say(text, bad) {
  note.textContent = text;
  note.classList.toggle('bad', !!bad);
}

async function press(ch) {
  if (busy) return;
  if (ch === 'del') entered = entered.slice(0, -1);
  else if (entered.length < 6) entered += ch;
  drawPin();
  if (entered.length !== 6) { say('Scanning the QR instead skips this step.', false); return; }

  busy = true;
  let res;
  try {
    res = await fetch('/auth', { method: 'POST', body: entered });
  } catch {
    busy = false; entered = ''; drawPin();
    say('The desktop stopped answering. Is sharing still on?', true);
    return;
  }
  busy = false;
  entered = '';
  drawPin();

  if (res.ok) { say('Scanning the QR instead skips this step.', false); show('files'); refresh(); }
  else if (res.status === 429) say('Too many wrong PINs. Restart sharing on the desktop.', true);
  // The PIN was right and the limit is what stopped it, so saying "wrong PIN"
  // here would send someone off retyping a PIN that works.
  else if (res.status === 409) say('Ten devices are already paired. Forget one on the desktop.', true);
  else say('That PIN did not work.', true);
}

for (const n of ['1', '2', '3', '4', '5', '6', '7', '8', '9']) {
  const k = document.createElement('button');
  k.type = 'button'; k.className = 'key'; k.textContent = n;
  k.addEventListener('click', () => press(n));
  $('keypad').appendChild(k);
}
const del = document.createElement('button');
del.type = 'button'; del.className = 'key wide'; del.textContent = 'DELETE';
del.addEventListener('click', () => press('del'));
$('keypad').appendChild(del);
const zero = document.createElement('button');
zero.type = 'button'; zero.className = 'key'; zero.textContent = '0';
zero.addEventListener('click', () => press('0'));
$('keypad').appendChild(zero);
drawPin();

document.addEventListener('keydown', (e) => {
  if (view !== 'pin') return;
  if (/^[0-9]$/.test(e.key)) press(e.key);
  else if (e.key === 'Backspace') press('del');
});

// ---- one row with a tick box ------------------------------------------------
// Shared by both lists: same shape, same hit target, and the whole row toggles
// rather than only the 18px box.

function tickRow(opts) {
  const { name, size, kind, checked, onToggle, tag } = opts;
  const li = document.createElement(tag || 'li');
  li.className = 'file';

  const box = document.createElement('input');
  box.type = 'checkbox';
  box.checked = checked;

  const kindEl = document.createElement('span');
  kindEl.className = 'file-kind';
  kindEl.textContent = kind;

  const text = document.createElement('span');
  text.className = 'file-text';
  const nameEl = document.createElement('span');
  nameEl.className = 'file-name';
  nameEl.textContent = name;
  const meta = document.createElement('span');
  meta.className = 'file-meta';
  meta.textContent = size;
  text.append(nameEl, meta);

  li.append(box, kindEl, text);
  li.addEventListener('click', (e) => {
    // The box fires its own change; anywhere else in the row we flip it.
    if (e.target !== box) box.checked = !box.checked;
    onToggle(box.checked);
  });
  box.addEventListener('click', (e) => e.stopPropagation());
  box.addEventListener('change', () => onToggle(box.checked));
  return li;
}

// ---- download ---------------------------------------------------------------

let shared = [];               // what the desktop is offering
const picked = new Set();      // ids ticked for download

async function refresh() {
  let res;
  try { res = await fetch('/api/files'); } catch { return; }
  if (res.status === 401) return show('pin');
  if (!res.ok) return;
  render(await res.json());
}

function render(items) {
  shared = items;
  // Drop ticks for files the desktop has since stopped sharing.
  const live = new Set(items.map((i) => i.id));
  for (const id of [...picked]) if (!live.has(id)) picked.delete(id);

  const list = $('files');
  list.replaceChildren();
  $('filesEmpty').hidden = items.length > 0;
  $('filesLabel').textContent = `Shared right now — ${plural(items.length, 'file')}`;

  for (const item of items) {
    list.appendChild(tickRow({
      name: item.name,
      size: bytes(item.bytes),
      kind: item.kind,
      checked: picked.has(item.id),
      onToggle: (on) => { on ? picked.add(item.id) : picked.delete(item.id); syncBars(); },
    }));
  }
  syncBars();
}

// A plain link per file, so the browser's own download manager owns each
// transfer and a resume after a screen lock is its problem, not ours. Several
// of them are clicked in turn with a beat between.
//
// ponytail: sequential synthetic clicks. Some mobile browsers only honour the
// first of a burst and prompt for the rest — if that turns out to bite, the fix
// is a server-side zip endpoint, not a shorter delay.
function getSelected() {
  const wanted = shared.filter((i) => picked.has(i.id));
  wanted.forEach((item, n) => {
    setTimeout(() => {
      const a = document.createElement('a');
      a.href = '/dl/' + item.id;
      a.setAttribute('download', item.name);
      a.rel = 'noopener';
      document.body.appendChild(a);
      a.click();
      a.remove();
    }, n * 350);
  });
}

$('getBtn').addEventListener('click', getSelected);

$('filesAll').addEventListener('change', (e) => {
  picked.clear();
  if (e.target.checked) for (const i of shared) picked.add(i.id);
  render(shared);
});

// ---- upload -----------------------------------------------------------------

let queue = [];                // {file, id, send} — chosen, not yet sent
const rows = new Map();        // queue id -> {pct, bar, meta}
let nextId = 1;
let sending = false;

function drawQueue() {
  const list = $('queue');
  list.replaceChildren();
  for (const q of queue) {
    list.appendChild(tickRow({
      name: q.file.name,
      size: bytes(q.file.size),
      kind: (q.file.name.split('.').pop() || '?').slice(0, 4).toUpperCase(),
      checked: q.send,
      onToggle: (on) => { q.send = on; syncBars(); },
    }));
  }
  $('queueBar').hidden = queue.length === 0;
  $('queueAll').checked = queue.length > 0 && queue.every((q) => q.send);
  syncBars();
}

function enqueue(files) {
  for (const file of files) queue.push({ file, id: nextId++, send: true });
  drawQueue();
}

function rowFor(q) {
  if (rows.has(q.id)) return rows.get(q.id);
  $('sendingBar').hidden = false;

  const wrap = document.createElement('div');
  wrap.className = 'up';
  const head = document.createElement('div');
  head.className = 'up-head';
  const name = document.createElement('span');
  name.className = 'up-name';
  name.textContent = q.file.name;
  const pct = document.createElement('span');
  pct.className = 'up-pct';
  pct.textContent = 'Waiting';
  head.append(name, pct);

  const track = document.createElement('div');
  track.className = 'track';
  const bar = document.createElement('div');
  bar.className = 'bar';
  track.appendChild(bar);

  const meta = document.createElement('div');
  meta.className = 'up-meta';
  meta.textContent = bytes(q.file.size);

  wrap.append(head, track, meta);
  $('uploads').prepend(wrap);

  const row = { pct, bar, meta, wrap };
  rows.set(q.id, row);
  return row;
}

function upload(q) {
  const file = q.file;
  const row = rowFor(q);
  return new Promise((resolve, reject) => {
    const xhr = new XMLHttpRequest();
    xhr.open('PUT', '/upload/' + encodeURIComponent(file.name));
    // XHR rather than fetch: only XHR reports upload progress.
    xhr.upload.onprogress = (e) => {
      if (!e.lengthComputable) return;
      const at = Math.round((e.loaded / e.total) * 100);
      row.bar.style.width = at + '%';
      row.pct.textContent = at + '%';
      row.meta.textContent = bytes(file.size) + ' · ' + bytes(e.loaded) + ' sent';
    };
    xhr.onload = () => {
      if (xhr.status === 401) { show('pin'); reject('unauthorised'); return; }
      if (xhr.status === 200) {
        row.bar.style.width = '100%';
        row.bar.dataset.state = 'done';
        row.pct.dataset.state = 'done';
        row.pct.textContent = 'Done';
        row.meta.textContent = bytes(file.size) + ' · arrived';
        resolve();
      } else {
        reject(xhr.status === 507 ? 'no room on the desktop' : 'failed (' + xhr.status + ')');
      }
    };
    xhr.onerror = () => reject('network');
    xhr.send(file); // the File is the body: no FormData, no multipart
  });
}

async function sendSelected() {
  if (sending) return;
  const going = queue.filter((q) => q.send);
  if (going.length === 0) return;

  sending = true;
  syncBars();
  // Out of the queue as it starts: a file cannot be sitting in "ready to send"
  // and be halfway to the desktop at the same time.
  queue = queue.filter((q) => !q.send);
  drawQueue();

  for (const q of going) {
    // Sequential: parallel writes to one disk are slower, and one active write
    // keeps the progress display honest.
    try {
      await upload(q);
    } catch (e) {
      const row = rowFor(q);
      row.bar.dataset.state = 'failed';
      row.pct.dataset.state = 'failed';
      row.pct.textContent = 'Failed';
      row.meta.textContent = String(e);
    }
  }
  sending = false;
  syncBars();
}

$('sendBtn').addEventListener('click', sendSelected);

$('queueAll').addEventListener('change', (e) => {
  for (const q of queue) q.send = e.target.checked;
  drawQueue();
});

$('queueClear').addEventListener('click', () => { queue = []; drawQueue(); });

// Only what has finished. A row still moving bytes has nowhere else to report
// from, so clearing it would lose the transfer's only progress display.
$('clearDone').addEventListener('click', () => {
  for (const [id, row] of [...rows]) {
    if (row.bar.dataset.state) { row.wrap.remove(); rows.delete(id); }
  }
  $('sendingBar').hidden = rows.size === 0;
});

$('pickBtn').addEventListener('click', () => $('picker').click());
$('picker').addEventListener('change', (e) => {
  enqueue([...e.target.files]);
  e.target.value = ''; // so picking the same file again re-queues it
});

const drop = $('drop');
for (const type of ['dragenter', 'dragover']) {
  drop.addEventListener(type, (e) => { e.preventDefault(); drop.classList.add('over'); });
}
for (const type of ['dragleave', 'drop']) {
  drop.addEventListener(type, () => drop.classList.remove('over'));
}
drop.addEventListener('drop', (e) => {
  e.preventDefault();
  if (e.dataTransfer?.files?.length) enqueue([...e.dataTransfer.files]);
});

// ---- the two bottom bars ----------------------------------------------------

function syncBars() {
  const n = picked.size;
  $('getBar').hidden = view !== 'files' || shared.length === 0;
  $('getBtn').disabled = n === 0;
  $('getLabel').textContent = n === 0 ? 'Get' : `Get ${plural(n, 'file')}`;
  $('filesAll').checked = shared.length > 0 && n === shared.length;

  const m = queue.filter((q) => q.send).length;
  $('sendBar').hidden = view !== 'upload' || queue.length === 0;
  $('sendBtn').disabled = m === 0 || sending;
  $('sendLabel').textContent =
    sending ? 'Sending…' : (m === 0 ? 'Send' : `Send ${plural(m, 'file')}`);
}

// ---- polling ----------------------------------------------------------------

let lastCount = -1;

async function tick() {
  let res;
  try { res = await fetch('/api/status'); } catch { return; }
  if (res.status === 401) return show('pin');
  if (!res.ok) return;
  const s = await res.json();
  // Only redraw when the tray actually changed — a re-render mid-scroll on a
  // phone is worse than a slightly stale count.
  if (s.files !== lastCount) { lastCount = s.files; if (view === 'files') refresh(); }
}

function startPolling() {
  if (poll) return;
  poll = setInterval(tick, 3000);
}
function stopPolling() {
  if (!poll) return;
  clearInterval(poll);
  poll = null;
}

document.addEventListener('visibilitychange', () => {
  if (document.hidden) stopPolling();
  else if (view !== 'pin') { startPolling(); tick(); }
});

for (const tab of document.querySelectorAll('.tab')) {
  tab.addEventListener('click', () => {
    show(tab.dataset.view);
    if (tab.dataset.view === 'files') refresh();
  });
}

// The QR's pairing key sets the cookie before this script runs, so the first
// call decides which view opens: 200 means we are already in.
(async function boot() {
  try {
    const res = await fetch('/api/files');
    if (res.ok) { show('files'); render(await res.json()); return; }
  } catch { /* fall through to the PIN form */ }
  show('pin');
})();

// Drop ?k= from the address bar once it has been spent — a single-use key left
// in the URL only invites a pointless reload.
if (location.search.includes('k=')) {
  history.replaceState({}, '', location.pathname);
}
