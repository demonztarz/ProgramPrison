# ProgramPrison

A desktop blocker for distracting websites and programs, for Windows, macOS and Linux. Put
sites and programs in a **block**, switch it on with a timer or a weekly schedule, and lock it so
you can't back out halfway.

> Alpha. It works and is tested on every push, but expect rough edges. Please report them.

## Download

Every push builds installers with GitHub Actions (**Actions → Build → Artifacts**):

| Platform | File |
|---|---|
| Windows | `ProgramPrison-<version>-setup.exe` (installer) or `ProgramPrison-<version>-portable.exe` |
| macOS | `ProgramPrison-<version>-mac-*.dmg` (unsigned: right-click → Open the first time) |
| Linux | `ProgramPrison-<version>-linux-x86_64.AppImage` |

Pushing a tag such as `v0.1.0` publishes the same files as a GitHub Release. The builds aren't
code-signed, so Windows SmartScreen will warn: choose **More info → Run anyway**.

## What it does

- **Websites**: type a domain or paste a link. `example.com` also blocks `www.`, `m.` and
  `mobile.` versions, plus known aliases (`youtu.be` for YouTube, `x.com` for Twitter, …).
  Quick-add lists for social media, video, news, shopping and gaming. Works in every browser.
- **Programs**: type a name (`steam.exe`, `Discord`) or pick one that's running. For each, choose:
  - **Close it**: the program is closed whenever it starts.
  - **Cover its windows** (Windows): an overlay the exact size of each of the program's windows
    sits on top of it and follows it when it moves, resizes or minimizes. The program keeps
    running underneath. Switching to it brings the cover to the front, so keys go to the cover.
- **When**: start a block *for* a time (15 min to a year) or *until* a date and time, or paint the
  hours of the week it should switch on by itself.
- **Locks**: what it takes to stop a block early.
  - *No lock*: stop any time.
  - *Random text*: type N random characters by hand. Copy and paste are off, and a new text is
    shown after every wrong try.
  - *Password*: have someone else set it. Stored as a salted scrypt hash.
  - *Locked*: no way out until time runs out.
  While a block is locked you can **add** sites, programs and hours, but not remove them,
  weaken the lock, delete the block, or quit the app.
- **Hard to get around**:
  - Starts at login.
  - Closing the window keeps it running in the tray.
  - Undoes hand edits to the hosts file within seconds.
  - Pushes running blocks back if the clock is moved forward.
  - Website blocks stay in the hosts file even if the app is force-closed.
  - Blocks come back as soon as the app starts again.

## How it works

**Websites:** ProgramPrison writes its blocked domains to the system **hosts file**, between
marker comments, pointing them at `0.0.0.0`. When nothing is blocked, the section is removed.

The first time a block includes websites, the app asks for an administrator password **once**.
This lets your user account edit the hosts file, so ProgramPrison never has to run as
administrator. Each system uses its own tool for this:

- Windows: `icacls`
- macOS: `chmod +a`
- Linux: `setfacl`, from the `acl` package

**Programs:** ProgramPrison checks the process list every 3 seconds, and only while a block that
includes programs is on. On Windows it reads the list straight from the system rather than
starting `tasklist` each time. It also watches for any program coming to the front, so a
blocked program that was just opened is dealt with immediately.

**Covers:** on Windows, ProgramPrison asks the system to send an event when a covered program's
windows move, resize, minimize, close or come to the front. It reacts to those events, so it
does no work while nothing changes. Covering isn't available on macOS or Linux (on Wayland it
isn't possible at all), so there "cover" programs are closed instead.

## Limits

This version doesn't defend against:

- someone with administrator rights who edits the hosts file and then kills ProgramPrison
- someone who uninstalls ProgramPrison
- someone who deletes its settings file while it isn't running

Other limits:

- A page that's already open can keep working for a minute, because browsers cache addresses
  and keep connections open. Reloading after that is blocked.
- Browsers using **DNS over HTTPS** may skip the hosts file, depending on their settings.
- Hosts files can't do wildcards: subdomains other than `www.`, `m.` and `mobile.` need adding
  separately.
- Exclusive-fullscreen games draw over covers, so set them to **Close it**.
- On Ubuntu 24.04 and newer, the AppImage may not start because of a system restriction on app
  sandboxes. Start it with `--no-sandbox`, or install it as a `.deb` once that build exists.

## Development

```bash
npm install
npm start          # run the app
npm test           # unit tests
npm run test:e2e   # drives the real app (on Linux: xvfb-run -a npm run test:e2e)
npm run icons      # redraw the icons
npm run dist       # build installers for this OS
```

The end-to-end test uses a scratch hosts file and data folder, so it never touches the real
ones. It works through the app as a user would:

1. Creates a block and adds websites and a program.
2. Starts the block and checks that the hosts file is written and the program is closed. On
   Windows, it checks instead that Notepad is covered and the cover follows Notepad when it
   moves.
3. Tries each lock.
4. Paints the schedule.
5. Checks that quitting is refused while the block is locked.
6. Force-kills the app, restarts it, and checks that blocking resumes.

Code layout:

| File | What |
|---|---|
| `engine.js` | All blocking rules: schedules, sessions, locks, domains, hosts file text. No OS calls; unit tested. |
| `enforcer.js` | Hosts file, process list, closing programs, the one-time permission step. |
| `win32.js` | Windows API calls through [koffi](https://koffi.dev): process list, window positions, window events. |
| `covers.js` | Cover windows for blocked programs (Windows). |
| `main.js` | Electron main process: state, timers, tray, start at login, IPC. |
| `ui/` | The window (plain HTML/CSS/JS) and the cover page. |
