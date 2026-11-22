# Program Prison: notes for working on this repo

## Windows comes first

Windows is the main platform; macOS and Linux are secondary.

- Design, build and check every feature for Windows first. A feature may ship on Windows
  before macOS and Linux catch up (as covering windows does), never the other way round.
- In CI, the `windows` job tests and builds the `.exe` on its own and must stay independent:
  don't make it wait for, or depend on, the macOS or Linux jobs.
- Before pushing, check that the Windows code compiles from Linux:
  `cargo clippy --target x86_64-pc-windows-gnu --all-targets`
- When reporting a push, lead with the Windows result and link to its download.

## Names

- The name users see is **Program Prison** (with a space).
- Internal identifiers stay `ProgramPrison`/`programprison`: the settings folder, the
  hosts-file markers, the registry Run value, window class names and the program file
  inside builds. Renaming them would lose people's existing blocks or leave old hosts-file
  entries behind.

## Testing

- `cargo test` runs unit tests, the background blocker end to end (`tests/service.rs`; on
  Windows it covers Notepad), a fake-Discord test (`tests/discord.rs`, Unix) and a headless
  click-through of the window (`tests/gui.rs`).
- Tests use a scratch hosts file and data folder (`PROGRAMPRISON_HOSTS`,
  `PROGRAMPRISON_DATA`) and must never touch the real ones or show anything on a real Discord.
