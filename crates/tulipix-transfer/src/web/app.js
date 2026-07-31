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
  // Leaving the PIN view with the camera still running would keep the indicator
  // lit on the phone for the rest of the session.
  if (next !== 'pin') stopScan();
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

  // Straight to the queue if something is waiting in it. Anyone who arrived
  // here from the share sheet was interrupted by the PIN, and putting them on
  // the download list afterwards asks them to find their own files again.
  if (res.ok) {
    say('Scanning the QR instead skips this step.', false);
    if (queue.length > 0) { show('upload'); return; }
    show('files'); refresh();
  }
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
// Zero takes the wide left slot and DELETE the narrow right one — the reverse
// of the usual arrangement. Under a thumb reaching for the bottom row, the wide
// key is the one that gets hit, and it should not be the destructive one.
const zero = document.createElement('button');
zero.type = 'button'; zero.className = 'key wide'; zero.textContent = '0';
zero.addEventListener('click', () => press('0'));
$('keypad').appendChild(zero);
const del = document.createElement('button');
del.type = 'button'; del.className = 'key'; del.textContent = 'DEL';
del.addEventListener('click', () => press('del'));
$('keypad').appendChild(del);
drawPin();

document.addEventListener('keydown', (e) => {
  if (view !== 'pin') return;
  if (/^[0-9]$/.test(e.key)) press(e.key);
  else if (e.key === 'Backspace') press('del');
});

// ---- QR scanner -------------------------------------------------------------
// The camera is gated on a secure context, so over plain http:// there is no
// scanner to offer and the button is replaced by a line pointing at the phone's
// own camera app — which reads the same code and needs no permission from us.
// BarcodeDetector is the decoder; nothing is bundled to stand in for it, because
// a QR library is larger than the rest of this page put together.

const scanBtn = $('scanBtn');
const scanStage = $('scanStage');
const scanVideo = $('scanVideo');
const scanNote = $('scanNote');
let scanStream = null;
let scanTimer = null;
let detector = null;

function scanSay(text, bad) {
  scanNote.textContent = text;
  scanNote.classList.toggle('bad', !!bad);
  scanNote.hidden = !text;
}

const canScan = window.isSecureContext
  && 'BarcodeDetector' in window
  && !!navigator.mediaDevices?.getUserMedia;

if (!canScan) {
  scanBtn.hidden = true;
  scanSay(
    window.isSecureContext
      ? 'Point your phone’s camera app at the code on the desktop — this browser cannot read QR codes itself.'
      : 'Point your phone’s camera app at the code on the desktop. In-page scanning needs a secure connection.',
    false,
  );
}

function stopScan() {
  if (scanTimer) { clearInterval(scanTimer); scanTimer = null; }
  if (scanStream) { for (const t of scanStream.getTracks()) t.stop(); scanStream = null; }
  scanVideo.srcObject = null;
  scanStage.hidden = true;
  scanBtn.hidden = false;
}

// The code carries a full URL. Only its `k` is used, and it is replayed against
// *this* origin — a QR photographed from someone else's desktop must not be able
// to bounce this page somewhere else.
function useCode(raw) {
  let key = null;
  try { key = new URL(raw, location.origin).searchParams.get('k'); } catch { /* not a URL */ }
  if (!key) { scanSay('That code is not a Tulipix pairing code.', true); return false; }
  stopScan();
  location.replace('/?k=' + encodeURIComponent(key));
  return true;
}

async function startScan() {
  scanSay('', false);
  try {
    detector = detector || new window.BarcodeDetector({ formats: ['qr_code'] });
    scanStream = await navigator.mediaDevices.getUserMedia({
      video: { facingMode: 'environment' },
      audio: false,
    });
  } catch {
    scanSay('The camera could not be opened. Allow camera access, or enter the PIN.', true);
    return;
  }
  scanVideo.srcObject = scanStream;
  try { await scanVideo.play(); } catch { /* autoplay is muted+inline, so this is rare */ }
  scanBtn.hidden = true;
  scanStage.hidden = false;

  // Polled rather than per-frame: decoding at 60fps drains a phone for no gain,
  // and a code held in front of a camera is there for far longer than 250ms.
  scanTimer = setInterval(async () => {
    if (!scanStream || scanVideo.readyState < 2) return;
    let found = [];
    try { found = await detector.detect(scanVideo); } catch { return; }
    for (const hit of found) if (hit.rawValue && useCode(hit.rawValue)) return;
  }, 250);
}

scanBtn.addEventListener('click', startScan);
$('scanStop').addEventListener('click', () => { stopScan(); scanSay('', false); });

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

// ---- installed app ----------------------------------------------------------

// The worker is what makes this installable, and it is what catches the share
// sheet. Registered after load so it never competes with the first paint, and
// failure is silent on purpose: without it the page is exactly what it was
// before — a web page that works — and a red banner about a service worker is
// not something anyone can act on from a phone.
if ('serviceWorker' in navigator) {
  window.addEventListener('load', () => {
    navigator.serviceWorker.register('/sw.js', { scope: '/' }).catch(() => {});
  });
}

// Files handed over by the OS share sheet. The worker parked them in a cache
// and sent us back here with a count; we turn them into the same queue entries
// the file picker makes, so everything downstream — the ticks, the progress
// bars, Send — is the code that was already there.
async function drainShare() {
  const flag = new URLSearchParams(location.search).get('share');
  if (!flag) return false;
  history.replaceState({}, '', location.pathname);

  if (flag === 'empty') {
    show('upload');
    return true;
  }

  let box;
  try { box = await caches.open('tulipix-share-v1'); } catch { return false; }
  const keys = await box.keys();
  const files = [];
  for (const key of keys) {
    const res = await box.match(key);
    if (!res) continue;
    const name = decodeURIComponent(res.headers.get('x-name') || 'shared');
    const type = res.headers.get('content-type') || 'application/octet-stream';
    files.push(new File([await res.blob()], name, { type }));
    await box.delete(key);
  }
  if (files.length === 0) return false;

  enqueue(files);
  show('upload');
  return true;
}

// The QR's pairing key sets the cookie before this script runs, so the first
// call decides which view opens: 200 means we are already in.
(async function boot() {
  let paired = false;
  try {
    const res = await fetch('/api/files');
    paired = res.ok;
    if (paired) render(await res.json());
  } catch { /* fall through to the PIN form */ }

  // A share always wins the opening view — someone who just shared a photo is
  // not here to browse what the desktop is offering. Unpaired, the PIN screen
  // still comes first; the queue survives it and is waiting on the other side.
  const shared = await drainShare();
  if (!paired) { show('pin'); return; }
  if (!shared) show('files');
})();

// Drop ?k= from the address bar once it has been spent — a single-use key left
// in the URL only invites a pointless reload.
if (location.search.includes('k=')) {
  history.replaceState({}, '', location.pathname);
}
