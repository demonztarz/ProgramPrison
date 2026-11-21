'use strict';
let info = null;

function fmt(ms) {
  const s = Math.max(0, Math.ceil(ms / 1000));
  const d = Math.floor(s / 86400);
  const h = Math.floor((s % 86400) / 3600);
  const m = Math.floor((s % 3600) / 60);
  if (d) return `${d} d ${h} h left`;
  if (h) return `${h} h ${String(m).padStart(2, '0')} min left`;
  return `${m}:${String(s % 60).padStart(2, '0')} left`;
}

function render() {
  if (!info) return;
  document.getElementById('app').textContent = `${info.app} is blocked`;
  document.getElementById('block').textContent = `by “${info.block}”`;
  document.getElementById('left').textContent = info.until ? fmt(info.until - Date.now()) : 'Blocked on a schedule';
}

window.cover.onInfo((i) => { info = i; render(); });
setInterval(render, 1000);
