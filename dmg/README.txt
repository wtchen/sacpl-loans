SacPL Loans — How to install and run (unsigned release)
=======================================================

This build is not signed with an Apple Developer certificate, so macOS shows
a security warning on first launch. It is a normal macOS app; the steps below
are just Apple's standard procedure for unsigned apps.

Install
-------
1. Double-click this disk image. If you see a license agreement, click
   "Agree" to mount it.
2. Drag "SacPL Loans.app" into the "Applications" folder shortcut shown in
   this window (or anywhere you like).
3. Eject this disk image (drag its icon to the Trash, or press Cmd+E).

First launch
------------
4. Open "SacPL Loans" from your Applications folder:
   - Right-click (or Control-click) the app and choose "Open", then click
     "Open" in the dialog. Do this once — macOS remembers your choice.
   - On newer macOS versions you may instead see "SacPL Loans cannot be
     opened because Apple cannot check it..." — dismiss it, then open
     System Settings ▸ Privacy & Security and click "Open Anyway".

If the app was moved from the internet and still won't start, you can clear
the quarantine flag from Terminal:

    xattr -dr com.apple.quarantine "/Applications/SacPL Loans.app"

Using the app
-------------
SacPL Loans lives in the menu bar (no Dock icon): click the open-book icon
to open the loans panel. Sign in once with your library ID and PIN — the
session is restored automatically from then on.

License
-------
See LICENSE.txt in this folder (CDDL-1.0).