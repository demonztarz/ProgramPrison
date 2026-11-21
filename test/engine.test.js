'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const E = require('../engine');

// A fixed local time: Wednesday 2026-11-18 10:30.
const WED_1030 = new Date(2026, 10, 18, 10, 30).getTime();
const MIN = 60000;

function block(extra = {}) {
  return Object.assign(E.newBlock('Test'), { sites: ['reddit.com'], apps: [{ name: 'steam', mode: 'close' }] }, extra);
}

test('normalizeDomain accepts links and strips the noise', () => {
  assert.equal(E.normalizeDomain('https://www.Reddit.com/r/all?x=1'), 'reddit.com');
  assert.equal(E.normalizeDomain('  youtube.com:443 '), 'youtube.com');
  assert.equal(E.normalizeDomain('*.example.co.uk'), 'example.co.uk');
  assert.equal(E.normalizeDomain('news.ycombinator.com/'), 'news.ycombinator.com');
  assert.equal(E.normalizeDomain('user@site.org'), 'site.org');
  assert.equal(E.normalizeDomain('xn--80ak6aa92e.com'), 'xn--80ak6aa92e.com');
  for (const bad of ['', 'localhost', 'not a site', '192.168.0.1', 'a..b.com', '-x.com', 'foo.']) {
    assert.equal(E.normalizeDomain(bad), null, bad);
  }
});

test('expandDomain covers www/m/mobile and related hosts', () => {
  const yt = E.expandDomain('youtube.com');
  for (const h of ['youtube.com', 'www.youtube.com', 'm.youtube.com', 'youtu.be']) assert.ok(yt.includes(h), h);
  assert.ok(E.expandDomain('twitter.com').includes('x.com'));
  assert.ok(!E.expandDomain('a.b.c.example.com').includes('m.a.b.c.example.com'));
});

test('hosts section: added, replaced, removed, idempotent, keeps the rest', () => {
  const original = '127.0.0.1 localhost\n::1 localhost\n';
  const a = E.applyHostsSection(original, ['reddit.com', 'www.reddit.com']);
  assert.ok(a.startsWith(original));
  assert.match(a, /0\.0\.0\.0 reddit\.com\n:: reddit\.com\n/);
  assert.equal(E.applyHostsSection(a, ['reddit.com', 'www.reddit.com']), a, 'same input gives the same file');
  const b = E.applyHostsSection(a, ['x.com']);
  assert.ok(!b.includes('reddit'));
  assert.equal((b.match(new RegExp(E.MARK_BEGIN.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'), 'g')) || []).length, 1);
  assert.equal(E.applyHostsSection(b, []), original);
  // CRLF files and files without a final newline
  const win = '127.0.0.1 localhost\r\n# comment';
  const w = E.applyHostsSection(win, ['x.com']);
  assert.ok(w.includes('\r\n0.0.0.0 x.com\r\n'));
  assert.equal(E.applyHostsSection(w, []), win + '\r\n');
  assert.equal(E.applyHostsSection('', []), '');
});

test('apps: names normalized, system programs refused, matching rules', () => {
  assert.equal(E.normalizeApp('C:\\Games\\Steam\\Steam.EXE').value, 'steam');
  assert.equal(E.normalizeApp('/Applications/Discord.app').value, 'discord');
  assert.ok(E.normalizeApp('explorer.exe').error);
  assert.ok(E.normalizeApp('ProgramPrison.exe').error);
  assert.ok(E.normalizeApp('   ').error);
  assert.ok(E.appMatches(['Steam.exe'], ['steam']));
  assert.ok(!E.appMatches(['steamwebhelper.exe'], ['steam']));
  assert.ok(E.appMatches(['/usr/bin/x', 'Discord'], ['discord']));
  assert.ok(E.appMatches(['averyveryverylo'], ['averyveryverylongname']), 'linux 15-char truncation');
  assert.ok(!E.appMatches(['explorer.exe'], ['explorer']), 'protected names never match');
});

test('cleanApps accepts old string lists and keeps modes', () => {
  assert.deepEqual(E.cleanApps(['Steam.exe', { name: 'discord', mode: 'cover' }, 'steam', 'explorer']), [
    { name: 'steam', mode: 'close' }, { name: 'discord', mode: 'cover' },
  ]);
});

test('blockedApps: cover falls back to close where unsupported, close wins', () => {
  const a = block({ apps: [{ name: 'steam', mode: 'cover' }, { name: 'discord', mode: 'cover' }] });
  const b = block({ apps: [{ name: 'steam', mode: 'close' }] });
  assert.deepEqual(E.blockedApps([a, b], true), { close: ['steam'], cover: ['discord'] });
  assert.deepEqual(E.blockedApps([a], false), { close: ['discord', 'steam'], cover: [] });
});

test('sessions: start, extend, expire', () => {
  const b = block();
  assert.equal(E.isActive(b, WED_1030), false);
  assert.equal(E.startSession(b, WED_1030, WED_1030 + 60 * MIN), null);
  assert.equal(E.isActive(b, WED_1030 + 59 * MIN), true);
  assert.equal(E.activeUntil(b, WED_1030), WED_1030 + 60 * MIN);
  // starting a shorter one never shortens it
  E.startSession(b, WED_1030 + MIN, WED_1030 + 10 * MIN);
  assert.equal(b.session.endsAt, WED_1030 + 60 * MIN);
  assert.equal(E.isActive(b, WED_1030 + 60 * MIN), false);
  assert.equal(E.expireSessions([b], WED_1030 + 61 * MIN), true);
  assert.equal(b.session, null);
});

test('sessions: refuses empty blocks and bad end times', () => {
  const empty = block({ sites: [], apps: [] });
  assert.ok(E.startSession(empty, WED_1030, WED_1030 + 60 * MIN));
  const b = block();
  assert.ok(E.startSession(b, WED_1030, WED_1030 - MIN));
  assert.ok(E.startSession(b, WED_1030, NaN));
  assert.ok(E.startSession(b, WED_1030, WED_1030 + 400 * E.DAY));
});

test('schedule: active in painted hours, runs merge with sessions', () => {
  const b = block();
  const wed = 3 * 24;
  b.schedule[wed + 10] = true;
  b.schedule[wed + 11] = true;
  assert.equal(E.isActive(b, WED_1030), true);
  assert.equal(E.activeUntil(b, WED_1030), new Date(2026, 10, 18, 12, 0).getTime());
  assert.equal(E.isActive(b, new Date(2026, 10, 18, 12, 0).getTime()), false);
  // a session that runs past the scheduled hours, then more schedule after it
  b.schedule[wed + 13] = true;
  E.startSession(b, WED_1030, new Date(2026, 10, 18, 13, 15).getTime());
  assert.equal(E.activeUntil(b, WED_1030), new Date(2026, 10, 18, 14, 0).getTime());
  // every hour scheduled: on forever
  b.schedule.fill(true);
  assert.equal(E.activeUntil(b, WED_1030), Infinity);
});

test('stopBlock turns off the current run but not the next scheduled one', () => {
  const b = block();
  const wed = 3 * 24;
  b.schedule[wed + 10] = true;
  b.schedule[wed + 15] = true;
  E.stopBlock(b, WED_1030);
  assert.equal(E.isActive(b, WED_1030 + MIN), false);
  assert.equal(E.isActive(b, new Date(2026, 10, 18, 15, 5).getTime()), true);
  // a plain timer just ends
  const t = block();
  E.startSession(t, WED_1030, WED_1030 + 60 * MIN);
  E.stopBlock(t, WED_1030 + MIN);
  assert.equal(t.unlockedUntil, 0);
  assert.equal(E.isActive(t, WED_1030 + 2 * MIN), false);
  // starting a new block clears the stop
  E.startSession(b, WED_1030 + 2 * MIN, WED_1030 + 20 * MIN);
  assert.equal(E.isActive(b, WED_1030 + 3 * MIN), true);
});

test('locks: edits that loosen a locked block are refused, additions allowed', () => {
  const b = block({ lock: { type: 'random', length: 100 } });
  b.schedule[3 * 24 + 10] = true;
  const now = WED_1030;
  assert.equal(E.isLocked(b, now), true);
  assert.ok(E.checkEdit(b, E.applyEdit(b, { sites: [] }), now));
  assert.ok(E.checkEdit(b, E.applyEdit(b, { apps: [] }), now));
  assert.ok(E.checkEdit(b, E.applyEdit(b, { apps: [{ name: 'steam', mode: 'cover' }] }), now), 'close -> cover loosens');
  assert.ok(E.checkEdit(b, E.applyEdit(b, { schedule: new Array(168).fill(false) }), now));
  assert.equal(E.checkEdit(b, E.applyEdit(b, { sites: ['reddit.com', 'x.com'], name: 'Renamed' }), now), null);
  assert.equal(E.checkEdit(b, E.applyEdit(b, { schedule: new Array(168).fill(true) }), now), null);
  // not locked when inactive, or with no lock
  const off = block({ lock: { type: 'locked' } });
  assert.equal(E.checkEdit(off, E.applyEdit(off, { sites: [] }), now), null);
  const open = block({ lock: { type: 'none' } });
  open.schedule.fill(true);
  assert.equal(E.checkEdit(open, E.applyEdit(open, { sites: [] }), now), null);
});

test('buildLock: validation and only-stronger while locked', () => {
  const b = block();
  assert.deepEqual(E.buildLock(b, { type: 'random', length: 300 }, 0, false).lock, { type: 'random', length: 300 });
  assert.ok(E.buildLock(b, { type: 'random', length: 5 }, 0, false).error);
  assert.ok(E.buildLock(b, { type: 'password', password: '' }, 0, false).error);
  const pw = E.buildLock(b, { type: 'password', password: 'hunter22' }, 0, false).lock;
  assert.ok(pw.salt && pw.hash && !('password' in pw));
  assert.ok(E.checkPassword(pw, 'hunter22'));
  assert.ok(!E.checkPassword(pw, 'hunter23'));
  assert.ok(!E.checkPassword(pw, undefined));
  b.lock = pw;
  assert.equal(E.buildLock(b, { type: 'password', password: '' }, 0, false).lock, pw, 'empty keeps the old password');
  // locked: weaker refused, Locked accepted, same allowed
  assert.ok(E.buildLock(b, { type: 'none' }, 0, true).error);
  assert.ok(E.buildLock(b, { type: 'password', password: 'new1' }, 0, true).error);
  assert.deepEqual(E.buildLock(b, { type: 'locked' }, 0, true).lock, { type: 'locked' });
  b.lock = { type: 'random', length: 200 };
  assert.ok(E.buildLock(b, { type: 'random', length: 50 }, 0, true).error);
  assert.ok(E.buildLock(b, { type: 'random', length: 400 }, 0, true).lock);
});

test('clock jump forward extends running sessions only', () => {
  const b = block();
  E.startSession(b, WED_1030, WED_1030 + 60 * MIN);
  const later = block();
  later.session = { startedAt: WED_1030 + 120 * MIN, endsAt: WED_1030 + 180 * MIN };
  const jump = E.adjustForClockJump([b, later], WED_1030 + MIN, WED_1030 + MIN + 5 * 3600000, 3000);
  assert.ok(jump > 4 * 3600000);
  assert.equal(b.session.endsAt, WED_1030 + 60 * MIN + jump);
  assert.equal(later.session.endsAt, WED_1030 + 180 * MIN);
  // normal ticks and small drift do nothing
  assert.equal(E.adjustForClockJump([b], 1000, 4000, 3000), 0);
  assert.equal(E.adjustForClockJump([b], 1000, 61000, 3000), 0);
  assert.equal(E.adjustForClockJump([b], 10 * 3600000, 1000, 3000), 0, 'backwards is ignored');
});

test('repairState survives junk and old formats', () => {
  assert.deepEqual(E.repairState(null), E.defaultState());
  const s = E.repairState({ blocks: [{ name: 'x', sites: ['https://Reddit.com', 'junk!'], apps: ['steam.exe'], schedule: [true], lock: { type: 'password' }, session: { endsAt: 5 } }, 'nonsense'] });
  assert.equal(s.blocks.length, 2);
  assert.deepEqual(s.blocks[0].sites, ['reddit.com']);
  assert.deepEqual(s.blocks[0].apps, [{ name: 'steam', mode: 'close' }]);
  assert.equal(s.blocks[0].schedule.length, 168);
  assert.equal(s.blocks[0].schedule[0], true);
  assert.equal(s.blocks[0].lock.type, 'locked', 'a password lock without a hash becomes Locked, not open');
});

test('publicBlock never exposes the password hash', () => {
  const b = block({ lock: { type: 'password', ...E.hashPassword('abcd') } });
  const p = E.publicBlock(b, WED_1030);
  assert.deepEqual(p.lock, { type: 'password' });
  assert.ok(!JSON.stringify(p).includes(b.lock.hash));
});

test('randomText uses the unambiguous alphabet', () => {
  const t = E.randomText(500);
  assert.equal(t.length, 500);
  assert.match(t, /^[a-km-zA-HJ-NP-Z2-9]+$/);
  assert.doesNotMatch(t, /[0O1lI]/);
});
