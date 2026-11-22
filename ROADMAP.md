# Roadmap

Planned features and ideas. Nothing here is promised by a date. Suggestions are welcome as
issues.

## Planned

### Random characters lock (bring back as an option)

Before version 0.2, the "random text" lock made you type a string of random letters and digits,
for example `7Xp7aDtkpc zdLXbrg7ks V7GjmbyyjX…`. It was replaced by the **random words** lock,
which is easier to read but still tedious to type. The idea is to offer both:

- **Random words** (current): a chain of short English words; capital letters and extra spaces
  don't matter.
- **Random characters** (returning): letters and digits in groups of ten, with upper and lower
  case counting. Much harder to type, for people who want a stronger lock.

Notes for whoever builds it:

- Add a new lock type (for example `characters`) next to `random` in `src/engine.rs` rather
  than changing `random`, so existing word locks stay as they are.
- Use the old alphabet without look-alike characters
  (`abcdefghjkmnpqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789`: no 0/O, 1/l/I), and its range of
  20 to 2000 characters.
- Matching ignores spaces between the groups but is case-sensitive.
- Lock strength order while a block is locked, weakest to strongest: none → words →
  characters → password → locked. Switching up is allowed; switching down is not.
- The old code is in the git history: `random_text()` in `src/engine.rs` before the
  "Random-text lock now asks for a chain of random words" commit.

### Browser extension for website blocking

Today websites are blocked through the system hosts file, which applies to the whole PC. That
causes a clash: blocking `discord.com` also cuts off the Discord desktop app, which then can't
recognise Program Prison and stops showing the Discord status. (0.3 works around it by connecting
to Discord before a block starts, but that doesn't help if the PC starts up during such a block.)

A small Program Prison extension for Chrome, Edge and Firefox would block sites inside the
browsers only, leaving desktop apps such as Discord untouched. It would also allow:

- blocking all subdomains of a site (the hosts file can't do wildcards), or single pages
  (`youtube.com/shorts`)
- a proper "blocked by Program Prison" page with the time left, instead of a connection error
- blocking that DNS-over-HTTPS settings can't bypass

Notes for whoever builds it:

- The extension talks to the background blocker through native messaging (a small host
  registered per browser) or the existing local connection, and asks which sites are blocked
  right now.
- Keep the hosts file as the fallback for browsers without the extension, and let each website
  choose "browsers only" (extension) or "whole PC" (hosts file).
- Locks apply: while a block is locked, the extension can't be switched off from Program
  Prison, and Program Prison notices if it's removed from a browser and falls back to the hosts
  file.

## Ideas

- Tray icon on macOS and Linux (today you reopen Program Prison from your apps).
- Code-signed builds, so Windows SmartScreen and macOS don't warn on first run.
- Covering windows on macOS (today "cover" falls back to closing there).
- A friendlier "blocked" page in the browser instead of a connection error.
