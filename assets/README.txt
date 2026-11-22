PROGRAMPRISON - how to use it
=============================

Program Prison blocks distracting websites and programs on a timer or a weekly
schedule, and can lock itself so you can't back out halfway.


GETTING STARTED
---------------
1. Run "Program Prison Setup" to install it (just for your account, no administrator
   prompt). It adds Program Prison to the Start menu. To update later, run a newer
   installer: your blocks carry on. Uninstall from Settings > Apps (not possible while a
   locked block is running).
   Using the single file instead? Move Program Prison.exe somewhere permanent, for
   example Documents\Program Prison. It starts itself at login from wherever it is, so
   don't leave it in Downloads.
2. Double-click it. If Windows SmartScreen warns you, click "More info" and then
   "Run anyway". That only happens the first time.
3. The settings window opens, and an orange bars icon appears in the system tray
   near the clock (click the ^ arrow if you don't see it).


MAKE A BLOCK
------------
A block is a list of things to keep away from you.

1. Click "Create your first block" (later: "+ New block" in the left sidebar).
2. Click the name at the top to rename it, for example "Work".

Websites tab
  - Type a site (reddit.com) or paste a link, then click Add.
    It also blocks the www., m. and mobile. versions.
  - Or use "Quick add" for ready-made lists: Social media, Video & streaming,
    News, Shopping, Gaming.
  - The first time, click "Set up website blocking" in the orange bar and choose
    Yes on the Windows administrator prompt. You only do this once.

Programs tab
  - Type a program name (steam.exe, Discord) and click Add, or pick one from
    "Or pick a program that's running now".
  - For each program choose:
      Close it          - the program is shut down whenever it starts
      Cover its windows - the program keeps running, but its windows are hidden
                          behind a "blocked" screen


TURN IT ON
----------
Right now, for a set time:
  Click Start, pick a length (15 min to 1 day) or type your own under "For",
  or choose "Until" and type an end time like 2026-11-21 17:00.
  Then click "Start block".

Automatically every week:
  Open the Schedule tab and click or drag across the hours it should switch on
  (orange = on). The buttons "Add weekdays 9-5" and "Add evenings 9pm-7am"
  fill common patterns.

While a block is on, the sidebar shows "On" (green) or "Locked" (orange) with a
countdown. "Extend" adds more time.


MAKE IT HARD TO BACK OUT
------------------------
On the Lock tab choose what it takes to stop early, then click "Save lock":

  No lock      - just click Stop
  Random words - type out a chain of random words, one at a time, like a
                 typing race: each word turns green when it's right and
                 Space locks it in; mistakes show in red. The block stops
                 by itself after the last word. Close the window any time:
                 your progress is kept. (Pasting doesn't work.)
  Password     - enter a password (have a friend type it so you don't know it)
  Locked       - no way to stop it; you wait until it ends

While a block is locked you can still ADD websites, programs or hours, but you
can't remove anything, delete the block or quit Program Prison. Restarting the
computer doesn't help: it starts again and carries on where it left off.


EVERYDAY USE
------------
  - Closing the window does NOT stop blocking; Program Prison keeps running in
    the tray.
  - Open the window again: left-click the tray icon, or run the program again.
  - Right-click the tray icon for "Open Program Prison" or "Quit" (Quit is greyed
    out while a block is locked).
  - Settings (bottom of the sidebar):
      "Start Program Prison when I log in" - leave this on
      "Quit Program Prison"                - shut it down completely


EMERGENCY EXIT
--------------
If you really need out, open Task Manager (Ctrl+Shift+Esc), find Program Prison
and click "End task". Nothing restarts it until you next log in.

  - Programs are allowed again straight away. Covered programs stay open and
    their covers disappear.
  - Websites stay blocked until the block's time is up.
  - To stop it starting at your next login: Task Manager > Startup apps >
    disable Program Prison.


GOOD TO KNOW
------------
  - A website that's already open may keep working for about a minute.
    Reload it after that, or restart the browser.
  - If a site still gets through, your browser's "secure DNS" setting may be
    skipping the block. In Chrome or Edge: Settings > Privacy > Use secure DNS
    > turn it off.
  - For full-screen games, use "Close it": games can draw over the cover.
  - If something goes wrong, the file programprison.log in
    %APPDATA%\ProgramPrison records what happened.

More: https://github.com/demonztarz/ProgramPrison
