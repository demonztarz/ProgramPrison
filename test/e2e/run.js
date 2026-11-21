'use strict';
// Drives the real app: creates a block, blocks a website and a program, locks it, unlocks it,
// paints the schedule, checks the quit guard and that state survives a restart.
// Linux/macOS: uses a scratch hosts file and a copy of `sleep` as the "blocked program".
// Windows: also checks that a blocked Notepad window gets covered and the cover follows it.

const { _electron: electron } = require('playwright-core');
const assert = require('node:assert/strict');
const fs = require('fs');
const os = require('os');
const path = require('path');
const { spawn, execFileSync } = require('child_process');

const ROOT = path.join(__dirname, '..', '..');
const SHOTS = process.env.E2E_SHOTS || path.join(ROOT, 'test-results');
const isWin = process.platform === 'win32';

const work = fs.mkdtempSync(path.join(os.tmpdir(), 'pp-e2e-'));
const dataDir = path.join(work, 'data');
const hostsFile = path.join(work, 'hosts');
fs.writeFileSync(hostsFile, '127.0.0.1 localhost\n');
fs.mkdirSync(SHOTS, { recursive: true });

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
async function waitFor(fn, what, timeout = 10000) {
  const end = Date.now() + timeout;
  let last;
  while (Date.now() < end) {
    try { last = await fn(); if (last) return last; } catch (e) { last = e; }
    await sleep(150);
  }
  throw new Error(`Timed out waiting for ${what}${last instanceof Error ? ': ' + last.message : ''}`);
}
const hosts = () => fs.readFileSync(hostsFile, 'utf8');

async function launch() {
  const args = [ROOT];
  if (process.getuid && process.getuid() === 0) args.push('--no-sandbox');
  const app = await electron.launch({
    executablePath: require('electron'),
    args,
    env: { ...process.env, PROGRAMPRISON_TEST: '1', PROGRAMPRISON_DATA: dataDir, PROGRAMPRISON_HOSTS: hostsFile, PROGRAMPRISON_TICK_MS: '500' },
  });
  const page = await app.firstWindow();
  await page.waitForLoadState('domcontentloaded');
  page.on('pageerror', (e) => { console.error('PAGE ERROR', e); process.exitCode = 1; });
  page.on('console', (m) => { if (m.type() === 'error') console.error('console:', m.text()); });
  page.on('dialog', (d) => d.accept()); // confirm() prompts
  return { app, page };
}

function startFakeProgram(name) {
  if (isWin) {
    return spawn('notepad.exe', [], { detached: false });
  }
  const exe = path.join(work, name);
  fs.copyFileSync('/bin/sleep', exe);
  fs.chmodSync(exe, 0o755);
  return spawn(exe, ['60']);
}

const exited = (child) => new Promise((res) => {
  if (child.exitCode !== null || child.signalCode !== null) res(true);
  else child.on('exit', () => res(true));
});

async function step(name, fn) {
  process.stdout.write(`• ${name} … `);
  await fn();
  console.log('ok');
}

(async () => {
  let { app, page } = await launch();
  try {
    await step('empty state is shown', async () => {
      await page.locator('#emptyNew').waitFor({ state: 'visible' });
      await page.screenshot({ path: path.join(SHOTS, '01-empty.png') });
    });

    await step('create and rename a block', async () => {
      await page.click('#emptyNew');
      await page.locator('#editor').waitFor({ state: 'visible' });
      await page.fill('#blockName', 'Deep work');
      await page.press('#blockName', 'Enter');
      await waitFor(async () => (await page.locator('.block-item .block-name').first().textContent()) === 'Deep work', 'renamed block in sidebar');
    });

    await step('add websites by link and preset', async () => {
      await page.fill('#siteInput', 'https://www.reddit.com/r/all');
      await page.click('#siteForm button');
      await page.locator('#siteList li', { hasText: 'reddit.com' }).waitFor();
      await page.fill('#siteInput', 'not a site');
      await page.click('#siteForm button');
      await page.locator('#toast.bad').waitFor({ state: 'visible' });
      await page.fill('#siteInput', '');
      await page.click('#presetChips button:has-text("Video")');
      await page.locator('#siteList li', { hasText: 'youtube.com' }).waitFor();
      assert.equal(await page.textContent('#sitesCount'), String(1 + 9));
    });

    const progName = isWin ? 'notepad' : 'timewaster';
    await step('add a program and refuse a system one', async () => {
      await page.click('.tab[data-tab=apps]');
      await page.fill('#appInput', isWin ? 'notepad.exe' : progName);
      await page.click('#appForm button');
      await page.locator('#appList li', { hasText: progName }).waitFor();
      await page.fill('#appInput', isWin ? 'explorer.exe' : 'systemd');
      await page.click('#appForm button');
      await page.locator('#toast.bad').waitFor({ state: 'visible' });
      await page.screenshot({ path: path.join(SHOTS, '02-programs.png') });
    });

    if (isWin) {
      await step('cover is offered on Windows; choose it for Notepad', async () => {
        assert.equal(await app.evaluate(() => global.__prison.coverSupported()), true, 'covering available');
        await page.selectOption('#appList li:has-text("notepad") select', 'cover');
        await waitFor(async () => (await app.evaluate(() => global.__prison.state().blocks[0].apps[0].mode)) === 'cover', 'mode saved');
      });
    }

    let prog;
    await step('start for 30 minutes: hosts file written, status On', async () => {
      prog = startFakeProgram(progName);
      await sleep(isWin ? 1500 : 300);
      await page.click('#startBtn');
      await page.click('#durationChips button[data-min="30"]');
      assert.match(await page.textContent('#startSummary'), /10 websites and 1 program/);
      await page.click('#startConfirm');
      await waitFor(() => /0\.0\.0\.0 reddit\.com/.test(hosts()) && /0\.0\.0\.0 youtu\.be/.test(hosts()), 'hosts entries');
      assert.match(await page.textContent('#statusPill'), /^On · (29|30):/);
      await page.screenshot({ path: path.join(SHOTS, '03-running.png') });
    });

    if (isWin) {
      await step('Notepad window is covered, and the cover follows it', async () => {
        const snap = await waitFor(async () => {
          const s = await app.evaluate(() => global.__prison.covers());
          return s && s.length && s[0].visible ? s : null;
        }, 'a visible cover', 15000);
        assert.equal(prog.exitCode, null, 'notepad is covered, not closed');
        const rect = winRect(snap[0].target);
        assertNear(snap[0].bounds, rect, 'cover matches the window');
        moveWindow(snap[0].target, rect.x + 120, rect.y + 80, 700, 500);
        await waitFor(async () => {
          const s = await app.evaluate(() => global.__prison.covers());
          const r = winRect(s[0].target);
          return near(s[0].bounds, r) && r.width > 0;
        }, 'cover to follow the moved window', 5000);
        await page.screenshot({ path: path.join(SHOTS, '04-covered.png') });
      });
    } else {
      await step('blocked program is closed', async () => {
        await waitFor(() => exited(prog), 'program to be closed', 5000);
        assert.equal(prog.signalCode, 'SIGKILL');
        await waitFor(async () => /closed today/.test(await page.textContent('#statsLine')), 'stats line');
      });
    }

    await step('hand edits to the hosts file are undone', async () => {
      fs.writeFileSync(hostsFile, '127.0.0.1 localhost\n');
      await waitFor(() => /0\.0\.0\.0 reddit\.com/.test(hosts()), 'section restored', 15000);
    });

    await step('random-text lock: removing is refused, typing the text stops it', async () => {
      await page.click('.tab[data-tab=lock]');
      await page.check('input[name=lock][value=random]');
      await page.fill('#randomLength', '20');
      await page.click('#saveLock');
      await page.locator('#lockNote').waitFor({ state: 'visible' });
      await page.click('.tab[data-tab=sites]');
      assert.equal(await page.locator('#siteList li button').first().isDisabled(), true);
      // even a direct request is refused by the main process
      const r = await page.evaluate(async () => {
        const s = await window.prison.getState();
        return window.prison.updateBlock({ id: s.blocks[0].id, sites: [] });
      });
      assert.equal(r.ok, false);
      assert.match(r.error, /locked/);
      await page.screenshot({ path: path.join(SHOTS, '05-locked.png') });

      await page.click('#stopBtn');
      await page.locator('#stopDialog .challenge').waitFor();
      await page.fill('#stopAnswer', 'wrong text');
      await page.click('#stopConfirm');
      await waitFor(async () => /doesn't match/.test(await page.textContent('#stopError')), 'mismatch error');
      await page.screenshot({ path: path.join(SHOTS, '06-unlock.png') });
      await waitFor(async () => (await page.textContent('#stopDialog .challenge')).replace(/\s/g, '').length === 20, 'fresh challenge');
      const text = (await page.textContent('#stopDialog .challenge')).replace(/\s/g, '');
      await page.locator('#stopAnswer').pressSequentially(text);
      await page.click('#stopConfirm');
      await waitFor(async () => /^Off/.test(await page.textContent('#statusPill')), 'block stopped');
      await waitFor(() => !/reddit/.test(hosts()), 'hosts section removed');
      if (isWin) await waitFor(async () => (await app.evaluate(() => global.__prison.covers())).length === 0, 'covers removed');
    });

    await step('schedule grid: drag paints hours', async () => {
      await page.click('.tab[data-tab=schedule]');
      const a = page.locator('.cell[data-slot="24"]'); // Monday 00:00 (well away from "now" on most days)
      const b = page.locator('.cell[data-slot="27"]');
      const ba = await a.boundingBox();
      const bb = await b.boundingBox();
      await page.mouse.move(ba.x + 5, ba.y + 5);
      await page.mouse.down();
      await page.mouse.move(bb.x + 5, bb.y + 5, { steps: 8 });
      await page.mouse.up();
      await waitFor(async () => (await app.evaluate(() => global.__prison.state().blocks[0].schedule.slice(24, 28).every(Boolean))), 'four hours saved');
      await page.click('#schedWork');
      await waitFor(async () => (await app.evaluate(() => global.__prison.state().blocks[0].schedule.filter(Boolean).length)) >= 40, 'weekday preset');
      await page.screenshot({ path: path.join(SHOTS, '07-schedule.png') });
      await page.click('#schedClear');
      await waitFor(async () => (await app.evaluate(() => global.__prison.state().blocks[0].schedule.filter(Boolean).length)) === 0, 'cleared');
    });

    await step('password lock: wrong password refused, right one stops it', async () => {
      await page.click('.tab[data-tab=lock]');
      await page.check('input[name=lock][value=password]');
      await page.fill('#passwordInput', 'letmeout');
      await page.click('#saveLock');
      await waitFor(async () => (await app.evaluate(() => global.__prison.state().blocks[0].lock.type)) === 'password', 'password saved');
      assert.ok(!fs.readFileSync(path.join(dataDir, 'state.json'), 'utf8').includes('letmeout'), 'password not stored in plain text');
      await page.click('#startBtn');
      await page.click('#durationChips button[data-min="15"]');
      await page.click('#startConfirm');
      await waitFor(async () => /^Locked/.test(await page.textContent('#statusPill')), 'locked');
      await page.click('#stopBtn');
      await page.fill('#stopAnswer', 'nope');
      await page.click('#stopConfirm');
      await waitFor(async () => /Wrong password/.test(await page.textContent('#stopError')), 'wrong password');
      await page.fill('#stopAnswer', 'letmeout');
      await page.click('#stopConfirm');
      await waitFor(async () => /^Off/.test(await page.textContent('#statusPill')), 'stopped');
    });

    await step('"Locked": no stop, no delete, quitting is refused', async () => {
      await page.click('.tab[data-tab=lock]');
      await page.check('input[name=lock][value=locked]');
      await page.click('#saveLock');
      await page.click('#startBtn');
      await page.click('#durationChips button[data-min="60"]');
      await page.click('#startConfirm');
      await waitFor(async () => /^Locked/.test(await page.textContent('#statusPill')), 'locked');
      assert.equal(await page.locator('#stopBtn').isDisabled(), true);
      assert.equal(await page.locator('#deleteBtn').isDisabled(), true);
      const r = await page.evaluate(async () => {
        const s = await window.prison.getState();
        return window.prison.stopBlock(s.blocks[0].id, '');
      });
      assert.equal(r.ok, false);
      const del = await page.evaluate(async () => window.prison.deleteBlock((await window.prison.getState()).blocks[0].id));
      assert.equal(del.ok, false);
      await app.evaluate(({ app: a }) => { setTimeout(() => a.quit(), 0); });
      await sleep(1500);
      assert.equal(await app.evaluate(() => global.__prison.state().blocks.length), 1, 'still running after quit request');
      await page.screenshot({ path: path.join(SHOTS, '08-hard-locked.png') });
    });

    await step('state survives a restart (even a forced one) and blocking resumes', async () => {
      app.process().kill('SIGKILL');
      await sleep(500);
      fs.writeFileSync(hostsFile, '127.0.0.1 localhost\n');
      ({ app, page } = await launch());
      await page.locator('.block-item').first().waitFor();
      assert.match(await page.textContent('#statusPill'), /^Locked/);
      await waitFor(() => /0\.0\.0\.0 reddit\.com/.test(hosts()), 'hosts restored after restart');
    });

    if (prog && prog.exitCode === null) prog.kill();
    console.log('\nAll end-to-end checks passed. Screenshots in', SHOTS);
  } catch (e) {
    console.log('FAILED');
    console.error(e);
    try { await page.screenshot({ path: path.join(SHOTS, 'failure.png') }); } catch {}
    process.exitCode = 1;
  } finally {
    try { app.process().kill('SIGKILL'); } catch {}
    if (isWin) { try { execFileSync('taskkill', ['/F', '/IM', 'notepad.exe']); } catch {} }
  }
})();

// ---------- Windows helpers (PowerShell + user32) ----------

function ps(script) {
  return execFileSync('powershell.exe', ['-NoProfile', '-NonInteractive', '-Command', script], { encoding: 'utf8' }).trim();
}
const PINVOKE = `Add-Type -Namespace W -Name U -MemberDefinition '[DllImport("dwmapi.dll")] public static extern int DwmGetWindowAttribute(System.IntPtr h, int a, int[] r, int s); [DllImport("user32.dll")] public static extern bool MoveWindow(System.IntPtr h, int x, int y, int w, int hh, bool r); [DllImport("user32.dll")] public static extern int GetDpiForWindow(System.IntPtr h);';`;
function winRect(hwnd) {
  const out = ps(`${PINVOKE} $r = New-Object int[] 4; [void][W.U]::DwmGetWindowAttribute([System.IntPtr]${hwnd}, 9, $r, 16); $d = [W.U]::GetDpiForWindow([System.IntPtr]${hwnd}); "$($r[0]) $($r[1]) $($r[2]) $($r[3]) $d"`);
  const [l, t, r, b, dpi] = out.split(/\s+/).map(Number);
  const k = (dpi || 96) / 96; // cover bounds are in DIPs
  return { x: l / k, y: t / k, width: (r - l) / k, height: (b - t) / k };
}
function moveWindow(hwnd, x, y, w, h) {
  ps(`${PINVOKE} [void][W.U]::MoveWindow([System.IntPtr]${hwnd}, ${Math.round(x)}, ${Math.round(y)}, ${w}, ${h}, $true)`);
}
function near(a, b) {
  return a && Math.abs(a.x - b.x) <= 3 && Math.abs(a.y - b.y) <= 3 && Math.abs(a.width - b.width) <= 3 && Math.abs(a.height - b.height) <= 3;
}
function assertNear(a, b, msg) {
  assert.ok(near(a, b), `${msg}: cover ${JSON.stringify(a)} vs window ${JSON.stringify(b)}`);
}
