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

## Ideas

- Tray icon on macOS and Linux (today you reopen ProgramPrison from your apps).
- Code-signed builds, so Windows SmartScreen and macOS don't warn on first run.
- Covering windows on macOS (today "cover" falls back to closing there).
- A friendlier "blocked" page in the browser instead of a connection error.
