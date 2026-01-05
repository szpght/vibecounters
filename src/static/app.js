const listEl = document.getElementById('list');
const dialog = document.getElementById('counter-dialog');
const form = document.getElementById('counter-form');
const addBtn = document.getElementById('add-btn');
const dialogTitle = document.getElementById('dialog-title');
const template = document.getElementById('card-template');

let counters = [];
let editingId = null;

addBtn.addEventListener('click', () => openDialog());
form.addEventListener('reset', () => closeDialog());
form.addEventListener('submit', async (e) => {
  e.preventDefault();
  const data = new FormData(form);
  const payload = {
    title: data.get('title').trim(),
    target: toRfc3339(data.get('target')),
  };
  if (!payload.title || !payload.target) return;

  if (editingId) {
    await fetch(`/api/counters/${editingId}`, {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(payload),
    });
  } else {
    await fetch('/api/counters', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(payload),
    });
  }
  await load();
  closeDialog();
});

function openDialog(counter) {
  dialogTitle.textContent = counter ? 'Edit Counter' : 'New Counter';
  editingId = counter?.id || null;
  form.title.value = counter?.title || '';
  form.target.value = counter ? fromRfc3339(counter.target) : '';
  dialog.showModal();
}

function closeDialog() {
  form.reset();
  dialog.close();
  editingId = null;
}

async function load() {
  const res = await fetch('/api/counters');
  counters = await res.json();
  render();
}

function render() {
  listEl.innerHTML = '';
  const now = Date.now();
  for (const counter of counters) {
    const node = template.content.cloneNode(true);
    const el = node.querySelector('.counter');
    el.dataset.id = counter.id;
    el.querySelector('.title').textContent = counter.title;
    const targetDate = new Date(counter.target);
    el.querySelector('.target').textContent = targetDate.toLocaleString();

    const [daysEl, hoursEl, minutesEl, secondsEl] = ['.days', '.hours', '.minutes', '.seconds']
      .map((sel) => el.querySelector(sel));

    const updateTime = () => {
      if (Number.isNaN(targetDate.getTime())) {
        daysEl.textContent = 'Invalid date';
        hoursEl.textContent = '--h';
        minutesEl.textContent = '--m';
        secondsEl.textContent = '--s';
        return;
      }
      const diffMs = targetDate.getTime() - Date.now();
      const abs = Math.abs(diffMs);
      const days = Math.floor(abs / (1000 * 60 * 60 * 24));
      const hours = Math.floor((abs / (1000 * 60 * 60)) % 24);
      const minutes = Math.floor((abs / (1000 * 60)) % 60);
      const seconds = Math.floor((abs / 1000) % 60);
      const prefix = diffMs >= 0 ? 'in ' : '';
      const suffix = diffMs < 0 ? ' ago' : '';
      daysEl.textContent = `${prefix}${days}d${suffix}`;
      hoursEl.textContent = `${hours.toString().padStart(2, '0')}h`;
      minutesEl.textContent = `${minutes.toString().padStart(2, '0')}m`;
      secondsEl.textContent = `${seconds.toString().padStart(2, '0')}s`;
    };

    updateTime();
    setInterval(updateTime, 1000);

    el.querySelector('.edit').addEventListener('click', () => openDialog(counter));
    el.querySelector('.delete').addEventListener('click', async () => {
      await fetch(`/api/counters/${counter.id}`, { method: 'DELETE' });
      await load();
    });

    listEl.appendChild(node);
  }
}

function toRfc3339(localValue) {
  // datetime-local is local time without zone
  if (!localValue) return '';
  const date = new Date(localValue);
  return Number.isNaN(date.getTime()) ? '' : date.toISOString();
}

function fromRfc3339(value) {
  if (!value) return '';
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return '';
  const pad = (n) => n.toString().padStart(2, '0');
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}T${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

load();

