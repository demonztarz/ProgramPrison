# ProgramPrison

A desktop blocker for distracting websites and programs, for Windows, macOS and Linux. Put
sites and programs in a **block**, switch it on with a timer or a weekly schedule, and lock it so
you can't back out halfway.

Written in Rust. The background blocker uses about **5 MB of memory** and no CPU worth
measuring. The whole app is a single file of around 10 MB, with no runtime to install.

> Alpha. It works and is tested on every push, but expect rough edges. Please report them.

## Download

Every push builds the app with GitHub Actions (**Actions → Build → Artifacts**):

| Platform | File |
|---|---|
| Windows | `ProgramPrison-<version>-windows-x64.exe`: a single file, no installer |
| macOS | `ProgramPrison-<version>-macos.zip`: unzip and move `ProgramPrison.app` to Applications |
| Linux | `ProgramPrison-<version>-linux-x64.tar.gz` |

Pushing a tag such as `v0.2.0` publishes the same files as a GitHub Release. The builds aren't
code-signed. On Windows, SmartScreen will warn: choose **More info → Run anyway**. On macOS,
right-click the app and choose **Open** the first time.

## What it does

- **Websites**: type a domain or paste a link. `example.com` also blocks `www.`, `m.` and
  `mobile.` versions, plus known aliases (`youtu.be` for YouTube, `x.com` for Twitter, …).
  Quick-add lists for social media, video, news, shopping and gaming. Works in every browser.
- **Programs**: type a name (`steam.exe`, `Discord`) or pick one that's running. For each, choose:
  - **Close it**: the program is closed whenever it starts.
  - **Cover its windows** (Windows): a cover the exact size of each of the program's windows sits
    on top of it and follows it when it moves, resizes or minimizes. The program keeps running
    underneath. Switching to it brings the cover to the front; if Windows won't allow that, the
    program is minimized instead.
- **When**: start a block *for* a time (15 minutes up to a year) or *until* a date and time, or
  paint the hours of the week it should switch on by itself.
- **Locks**: what it takes to stop a block early.
  - *No lock*: stop any time.
  - *Random words*: type out a chain of N random words by hand (for example "acorn pluck tidy
    gecko…"). Pasting is blocked, and a new chain appears after every wrong try.
  - *Password*: have someone else set it. Stored as a salted scrypt hash.
  - *Locked*: no way out until time runs out.
  While a block is locked you can **add** sites, programs and hours, but you can't remove them,
  weaken the lock, delete the block, quit ProgramPrison or turn off start-at-login.
- **Hard to get around**:
  - Starts at login.
  - Undoes hand edits to the hosts file within seconds.
  - Pushes running blocks back if the clock is moved forward.
  - Website blocks stay in the hosts file even if ProgramPrison is force-closed.
  - Blocks resume the moment it starts again.
  - It can still be ended from Task Manager in an emergency (see [Emergency exit](#emergency-exit)).

## How to use it

1. **Put the program somewhere permanent** (for example `Documents\ProgramPrison`) and run it.
   It starts itself at login from wherever it is. The window opens, and an orange bars icon
   appears in the system tray (click **^** near the clock if you don't see it).
2. **Make a block**: **Create your first block** (later: **+ New block**). Click its name at
   the top to rename it.
3. **Websites tab**: type a site or paste a link, then **Add**, or use **Quick add**. The first
   time, click **Set up website blocking** in the orange bar and answer **Yes** to the
   administrator prompt. That's needed once only.
4. **Programs tab**: type a name (`steam.exe`, `Discord`) and **Add**, or pick a running one.
   Choose **Close it** or **Cover its windows** for each.
5. **Turn it on**:
   - **Start** → pick a length, or **Until** a time → **Start block**.
   - Or paint hours on the **Schedule** tab to make it switch on by itself every week.
6. **Lock it** (optional): on the **Lock** tab choose Random words, Password or Locked, then
   **Save lock**.

Closing the window keeps blocking. Left-click the tray icon (or run the program again) to
reopen it. Right-click the tray icon for **Quit**, which is disabled while a locked block is on.

### Emergency exit

If you really need out, ProgramPrison can always be ended from **Task Manager** (Ctrl+Shift+Esc;
on macOS, Activity Monitor → Force Quit). Nothing restarts it until you next log in.

- **Programs** are allowed again straight away.
- **Websites stay blocked** until the block's time is up: they're listed in the system hosts
  file, which ProgramPrison cleans up the next time it runs after the block has ended.
- To keep it from starting again at your next login, open Task Manager → **Startup apps** and
  disable ProgramPrison.

## How it's built

ProgramPrison is one program that runs in two modes:

- **The background blocker** (`programprison --service`) has no window. It starts at login and
  enforces blocks.
  - **Windows:** everything runs on one thread around the Windows message loop. That includes the
    tray icon, the timer and the covers, which are plain native windows painted with GDI. Windows
    sends an event when a covered program's window moves, resizes, minimizes, closes or comes to
    the front, so covers follow instantly and nothing is polled.
  - **macOS and Linux:** a loop that sleeps between checks.
- **The settings window** (`programprison`, built with [egui](https://github.com/emilk/egui))
  only runs while it's open. Closing it leaves the blocker running. Every change goes through
  the blocker, which enforces the lock rules. The window talks to the blocker over a local
  connection that only this user's programs can authenticate to.

**Websites:** ProgramPrison writes the blocked domains to the system **hosts file**, between
marker comments, pointing them at `0.0.0.0`. When nothing is blocked, the section is removed.

The first time, the app asks for an administrator password **once**. That lets your user account
edit the hosts file, so ProgramPrison never has to run as administrator. Each system uses its
own tool for this:

- Windows: `icacls`
- macOS: `chmod +a`
- Linux: `setfacl`, from the `acl` package

**Programs:** the process list is read directly from the system, every 3 seconds and only while
a block that includes programs is on:

- Windows: Toolhelp
- macOS: libproc
- Linux: `/proc`

On Windows, a program that comes to the front is checked immediately.

## Limits

This version doesn't defend against:

- someone with administrator rights who edits the hosts file and then kills ProgramPrison
- someone who deletes it or its settings file while it isn't running

Other limits:

- A page that's already open can keep working for a minute, because browsers cache addresses
  and keep connections open.
- Browsers using **DNS over HTTPS** may skip the hosts file, depending on their settings.
- Hosts files can't do wildcards: subdomains other than `www.`, `m.` and `mobile.` need adding
  separately.
- Exclusive-fullscreen games draw over covers, so set them to **Close it**.
- macOS and Linux have no tray icon yet. Open ProgramPrison from your apps; **Settings → Quit
  ProgramPrison** stops the blocker.

## Development

```bash
cargo run                          # the settings window (starts the blocker if needed)
cargo run -- --service             # just the background blocker
cargo test                         # everything below
cargo build --release              # target/release/programprison
```

The tests use a scratch hosts file and data folder, never the real ones.

- **Unit tests** cover the blocking rules: schedules, sessions, locks, domains and the hosts
  file text.
- **`tests/service.rs`** starts the real background blocker and works through a full session:
  1. Blocks a website and a program, and checks that the hosts file is written and the program
     is closed. On Windows, it checks instead that Notepad is covered and the cover follows
     Notepad when it moves.
  2. Undoes a hand edit to the hosts file.
  3. Tries each lock and the schedule.
  4. Checks that quitting is refused while the block is locked.
  5. Force-kills the blocker, restarts it, and checks that blocking resumes.
- **`tests/gui.rs`** clicks through the real settings window, headless: it creates a block, adds
  websites and a program, locks it, starts it, and unlocks it by typing the random words. It
  also checks that pasting the text is refused.

Code layout:

| File | What |
|---|---|
| `src/engine.rs` | All blocking rules. No system calls; unit tested. |
| `src/service.rs` | The background blocker: state, requests from the window, enforcement. |
| `src/platform/windows.rs` | Windows: message loop, window-event hooks, covers, tray, process list. |
| `src/platform/unix.rs` | macOS and Linux loop. |
| `src/hosts.rs`, `src/procs.rs` | Hosts file; listing and closing processes. |
| `src/ipc.rs` | Window ↔ blocker protocol. |
| `src/gui.rs` | The settings window. |
| `src/autostart.rs` | Start at login (Run key, LaunchAgent, XDG autostart). |

## Roadmap

What's planned next, including bringing back the random-characters lock as an option, is in
[ROADMAP.md](ROADMAP.md).

## Credits

The word list for the random-words lock is the [EFF short diceware list](https://www.eff.org/dice)
by the Electronic Frontier Foundation, used under [CC BY 3.0 US](https://creativecommons.org/licenses/by/3.0/us/).
