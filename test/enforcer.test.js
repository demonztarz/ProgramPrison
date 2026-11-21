'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('fs');
const os = require('os');
const path = require('path');
const { spawn } = require('child_process');
const enforcer = require('../enforcer');

// A long-running stand-in program with a chosen process name. Copying /bin/sleep doesn't work
// everywhere (multi-call coreutils refuse to run under another name), so on Linux it's a shell
// script, whose process takes the script's name.
function makeFakeProgram(dir, name) {
  const exe = path.join(dir, name);
  if (process.platform === 'linux') fs.writeFileSync(exe, '#!/bin/sh\nwhile :; do sleep 1; done\n');
  else fs.copyFileSync('/bin/sleep', exe);
  fs.chmodSync(exe, 0o755);
  return exe;
}

test('parseTasklist reads tasklist CSV', () => {
  const out = '"System Idle Process","0","Services","0","8 K"\r\n"Steam.exe","4242","Console","1","120,000 K"\r\n';
  assert.deepEqual(enforcer.parseTasklist(out), [{ pid: 0, names: ['System Idle Process'] }, { pid: 4242, names: ['Steam.exe'] }]);
});

test('parsePs reads ps output, including macOS bundle names', () => {
  const out = '    1 /sbin/launchd\n  777 /Applications/Steam.app/Contents/MacOS/steam_osx\n 900 Discord\n';
  assert.deepEqual(enforcer.parsePs(out), [
    { pid: 1, names: ['launchd'] },
    { pid: 777, names: ['steam_osx', 'Steam'] },
    { pid: 900, names: ['Discord'] },
  ]);
});

test('syncHosts writes, keeps, and removes the managed section in place', () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'pp-'));
  const file = path.join(dir, 'hosts');
  fs.writeFileSync(file, '127.0.0.1 localhost\n');
  const ino = fs.statSync(file).ino;
  let r = enforcer.syncHosts(['reddit.com'], file);
  assert.deepEqual(r, { ok: true, changed: true });
  assert.match(fs.readFileSync(file, 'utf8'), /0\.0\.0\.0 reddit\.com/);
  assert.equal(fs.statSync(file).ino, ino, 'written in place, so file permissions survive');
  assert.deepEqual(enforcer.syncHosts(['reddit.com'], file), { ok: true, changed: false });
  r = enforcer.syncHosts([], file);
  assert.equal(fs.readFileSync(file, 'utf8'), '127.0.0.1 localhost\n');
});

test('syncHosts reports when it needs permission', { skip: process.getuid && process.getuid() === 0 ? 'running as root' : false }, () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'pp-'));
  const file = path.join(dir, 'hosts');
  fs.writeFileSync(file, '127.0.0.1 localhost\n');
  fs.chmodSync(file, 0o444);
  const r = enforcer.syncHosts(['reddit.com'], file);
  assert.equal(r.ok, false);
  assert.equal(r.needsSetup, true);
  assert.equal(enforcer.canWriteHosts(file), false);
  // nothing to block and no section: no write needed, so no error
  assert.equal(enforcer.syncHosts([], file).ok, true);
});

test('closeMatching closes a running program by name and leaves others', { skip: process.platform === 'win32' }, async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'pp-'));
  const a = spawn(makeFakeProgram(dir, 'timewaster'), ['30']);
  const b = spawn(makeFakeProgram(dir, 'keepalive'), ['30']);
  const exited = new Promise((res) => a.on('exit', (code, sig) => res(sig)));
  await new Promise((r) => setTimeout(r, 200));
  const closed = await enforcer.closeMatching(['timewaster']);
  assert.deepEqual(closed, ['timewaster']);
  assert.equal(await exited, 'SIGKILL');
  assert.equal(b.exitCode, null, 'other program still running');
  b.kill();
});
