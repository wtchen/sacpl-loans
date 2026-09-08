## SacPL Loans

MacOS menu bar app to check the status of your Sacramento Public Library loans.

### Run from code

Requires: Rust (stable), Xcode command-line tools, Node 18+.

```sh
npm install
npm run tauri dev
```

### Build

```sh
npm run tauri build
# .app in  src-tauri/target/release/bundle/macos/
# .dmg in  src-tauri/target/release/bundle/dmg/  (mounts with a license-agreement prompt)
```

The default build excludes developer/debug tooling. To create a debug build:

```sh
npm run tauri build -- --features debug-mode
# or for development:
npm run tauri dev -- --features debug-mode
```

In debug builds, Settings ▸ Developer Settings appears; in default builds it
is hidden. The app event log (`app.log`) is written in all builds.

The program runs as a menu bar app (no Dock icon).