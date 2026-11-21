'use strict';
// Talks to the operating system: the hosts file (websites) and the process list (apps).

const fs = require('fs');
const os = require('os');
const path = require('path');
const { execFile } = require('child_process');
const engine = require('./engine');

// On Windows the process list comes straight from the system instead of running tasklist.
let win32 = null;
if (process.platform === 'win32') {
  try { win32 = require('./win32'); } catch (e) { console.error('Windows API unavailable, using tasklist:', e.message); }
}

const HOSTS_PATH = process.platform === 'win32'
  ? path.join(process.env.SystemRoot || 'C:\\Windows', 'System32', 'drivers', 'etc', 'hosts')
  : '/etc/hosts';

function run(cmd, args, opts = {}) {
  return new Promise((resolve) => {
    execFile(cmd, args, { windowsHide: true, timeout: 20000, maxBuffer: 32 * 1024 * 1024, ...opts }, (err, stdout, stderr) => {
      resolve({ err, code: err ? (typeof err.code === 'number' ? err.code : -1) : 0, stdout: String(stdout || ''), stderr: String(stderr || '') });
    });
  });
}

// ---------- websites ----------

function canWriteHosts(hostsPath = HOSTS_PATH) {
  try {
    fs.closeSync(fs.openSync(hostsPath, 'r+'));
    return true;
  } catch {
    return false;
  }
}

// Writes the managed section for these domains. Writes in place (no rename) so that the
// permission granted on the file by grantHostsAccess survives.
function syncHosts(domains, hostsPath = HOSTS_PATH) {
  let current = '';
  try {
    current = fs.readFileSync(hostsPath, 'utf8');
  } catch (e) {
    if (e.code !== 'ENOENT') return { ok: false, error: e.message };
  }
  const next = engine.applyHostsSection(current, domains);
  if (next === current) return { ok: true, changed: false };
  try {
    fs.writeFileSync(hostsPath, next, 'utf8');
  } catch (e) {
    return { ok: false, needsSetup: e.code === 'EACCES' || e.code === 'EPERM', error: e.message };
  }
  flushDns();
  return { ok: true, changed: true };
}

function flushDns() {
  if (process.platform === 'win32') run('ipconfig', ['/flushdns']);
  else if (process.platform === 'darwin') run('dscacheutil', ['-flushcache']);
  else run('resolvectl', ['flush-caches']);
}

// One-time step that asks for the administrator password once, so the app can then keep the
// hosts file up to date without running as administrator.
async function grantHostsAccess() {
  const user = os.userInfo().username;
  let r;
  if (process.platform === 'win32') {
    const account = process.env.USERDOMAIN ? `${process.env.USERDOMAIN}\\${user}` : user;
    const inner = `attrib -r "${HOSTS_PATH}" & icacls "${HOSTS_PATH}" /grant "${account}:M"`;
    const ps = `$p = Start-Process -FilePath cmd.exe -ArgumentList '/c', '${inner.replace(/'/g, "''")}' -Verb RunAs -Wait -WindowStyle Hidden -PassThru; exit $p.ExitCode`;
    r = await run('powershell.exe', ['-NoProfile', '-NonInteractive', '-Command', ps], { timeout: 120000 });
  } else if (process.platform === 'darwin') {
    const shell = `/bin/chmod +a 'user:${user} allow write,append' /private/etc/hosts`;
    const script = `do shell script "${shell.replace(/\\/g, '\\\\').replace(/"/g, '\\"')}" with administrator privileges`;
    r = await run('osascript', ['-e', script], { timeout: 120000 });
  } else {
    const which = await run('sh', ['-c', 'command -v setfacl']);
    if (which.code !== 0) {
      return { ok: false, error: 'The setfacl tool is missing. Install the "acl" package (for example: sudo apt install acl), then try again.' };
    }
    r = await run('pkexec', [which.stdout.trim(), '-m', `u:${user}:rw`, HOSTS_PATH], { timeout: 120000 });
  }
  if (canWriteHosts()) return { ok: true };
  const detail = (r.stderr || r.stdout || (r.err && r.err.message) || '').trim();
  return { ok: false, error: detail ? `Permission was not granted (${detail.split('\n')[0]}).` : 'Permission was not granted.' };
}

// ---------- apps ----------

function parseTasklist(out) {
  const procs = [];
  for (const line of out.split(/\r?\n/)) {
    const m = line.match(/^"([^"]*)","(\d+)"/);
    if (m) procs.push({ pid: Number(m[2]), names: [m[1]] });
  }
  return procs;
}

function parsePs(out) {
  const procs = [];
  for (const line of out.split('\n')) {
    const m = line.match(/^\s*(\d+)\s+(.+?)\s*$/);
    if (!m) continue;
    const comm = m[2];
    const names = [path.posix.basename(comm)];
    const bundle = comm.match(/\/([^/]+)\.app\//); // macOS: /Applications/Steam.app/Contents/MacOS/steam_osx
    if (bundle) names.push(bundle[1]);
    procs.push({ pid: Number(m[1]), names });
  }
  return procs;
}

async function listProcesses() {
  if (win32) return win32.listProcesses();
  if (process.platform === 'win32') return parseTasklist((await run('tasklist', ['/fo', 'csv', '/nh'])).stdout);
  return parsePs((await run('ps', ['-A', '-o', 'pid=,comm='])).stdout);
}

// Names to offer in the "pick a running program" list.
async function runningAppNames() {
  const names = new Set();
  for (const p of await listProcesses()) {
    const n = p.names[p.names.length - 1];
    if (engine.normalizeApp(n).value) names.add(process.platform === 'win32' ? n : engine.appKey(n));
  }
  return [...names].sort((a, b) => a.localeCompare(b, undefined, { sensitivity: 'base' }));
}

async function killPid(pid) {
  if (process.platform === 'win32') {
    const r = await run('taskkill', ['/F', '/T', '/PID', String(pid)]);
    return r.code === 0;
  }
  try {
    process.kill(pid, 'SIGKILL');
    return true;
  } catch {
    return false;
  }
}

const ownPids = new Set([process.pid, process.ppid]);

// Closes every running process matching the list. Returns the names that were closed.
async function closeMatching(apps, procs) {
  if (!apps.length) return [];
  const list = procs || (await listProcesses());
  const closed = [];
  for (const p of list) {
    if (ownPids.has(p.pid) || p.pid <= 4) continue;
    if (!engine.appMatches(p.names, apps)) continue;
    if (await killPid(p.pid)) closed.push(p.names[p.names.length - 1]);
  }
  return closed;
}

module.exports = { win32, killPid, HOSTS_PATH, canWriteHosts, syncHosts, grantHostsAccess, listProcesses, runningAppNames, closeMatching, parseTasklist, parsePs };
