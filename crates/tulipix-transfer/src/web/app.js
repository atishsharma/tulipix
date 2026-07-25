// The phone side. Three views behind one cookie: PIN, download, upload.
//
// The session token lives in an HttpOnly cookie the server sets, so there is
// nothing here to store, clear or leak — a 401 from any call is the single
// signal that we are back to the PIN form.

const $ = (id) => document.getElementById(id);

const views = { pin: $('viewPin'), files: $('viewFiles'), upload: $('viewUpload') };
let view = 'pin';
let poll = null;

function show(next) {
  view = next;
  for (const [name, el] of Object.entries(views)) el.hidden = name !== next;
  $('tabs').hidden = next === 'pin';
  $('chip').hidden = next === 'pin';
  for (const tab of document.querySelectorAll('.tab')) {
    tab.setAttribute('aria-selected', String(tab.dataset.view === next));
  }
  if (next === 'pin') stopPolling(); else startPolling();
}

function bytes(n) {
  if (n < 1024) return n + ' B';
  const units = ['KB', 'MB', 'GB', 'TB'];
  let v = n / 1024, i = 0;
  while (v >= 1024 && i < units.length - 1) { v /= 1024; i++; }
  return (v < 10 ? v.toFixed(1) : Math.round(v)) + ' ' + units[i];
}

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

// ---- download ---------------------------------------------------------------

async function refresh() {
  let res;
  try { res = await fetch('/api/files'); } catch { return; }
  if (res.status === 401) return show('pin');
  if (!res.ok) return;
  render(await res.json());
}

function render(items) {
  const list = $('files');
  list.replaceChildren();
  $('filesEmpty').hidden = items.length > 0;
  $('filesLabel').textContent =
    items.length === 1 ? 'Shared right now — 1 file' : `Shared right now — ${items.length} files`;

  for (const item of items) {
    const li = document.createElement('li');
    li.className = 'file';

    const kind = document.createElement('span');
    kind.className = 'file-kind';
    kind.textContent = item.kind;

    const text = document.createElement('span');
    text.className = 'file-text';
    const name = document.createElement('span');
    name.className = 'file-name';
    name.textContent = item.name;
    const meta = document.createElement('span');
    meta.className = 'file-meta';
    meta.textContent = bytes(item.bytes);
    text.append(name, meta);

    // A plain link, so the browser's own download manager owns the transfer and
    // a resume after a screen lock is its problem, not ours.
    const get = document.createElement('a');
    get.className = 'get';
    get.href = '/dl/' + item.id;
    get.setAttribute('download', item.name);
    get.textContent = 'Get';

    li.append(kind, text, get);
    list.appendChild(li);
  }
}

// ---- upload -----------------------------------------------------------------

const rows = new Map(); // File -> {pct, bar, meta}

function rowFor(file) {
  if (rows.has(file)) return rows.get(file);
  $('sendingLabel').hidden = false;

  const wrap = document.createElement('div');
  wrap.className = 'up';
  const head = document.createElement('div');
  head.className = 'up-head';
  const name = document.createElement('span');
  name.className = 'up-name';
  name.textContent = file.name;
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
  meta.textContent = bytes(file.size);

  wrap.append(head, track, meta);
  $('uploads').prepend(wrap);

  const row = { pct, bar, meta };
  rows.set(file, row);
  return row;
}

function upload(file) {
  const row = rowFor(file);
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

async function uploadAll(files) {
  for (const f of files) {
    // Sequential: parallel writes to one disk are slower, and one active write
    // keeps the progress display honest.
    try {
      await upload(f);
    } catch (e) {
      const row = rowFor(f);
      row.bar.dataset.state = 'failed';
      row.pct.dataset.state = 'failed';
      row.pct.textContent = 'Failed';
      row.meta.textContent = String(e);
    }
  }
}

$('pickBtn').addEventListener('click', () => $('picker').click());
$('picker').addEventListener('change', (e) => {
  uploadAll([...e.target.files]);
  e.target.value = ''; // so picking the same file again re-sends it
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
  if (e.dataTransfer?.files?.length) uploadAll([...e.dataTransfer.files]);
});

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
