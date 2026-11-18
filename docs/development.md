# Development

```sh
cargo test --workspace          # unit tests + real-PTY/real-DB integration tests, including an abort() crash test
cargo run -p termroost-core --example seed -- /some/dir   # demo data (use with TERMINALROOSTER_DATA_DIR)
```

## Architecture

Cargo workspace with three crates:

- **`termroost-core`**: the program. No GUI dependencies. It owns the state, the SQLite store (on a writer thread), the terminals (`alacritty_terminal` with PTY / ConPTY), the helper processes that keep terminals running (`host`) and the autosave + Codex detection thread. A front end uses only this API:
  - `Core::open(Config)`
  - `core.state()`: projects, tabs, panes, tasks
  - `core.dispatch(Command)`: every change
  - `core.terminal(pane)`: returns a `TerminalHandle` for keys, paste, mouse, selection, resize, and `with_content` for a GUI-neutral grid with resolved RGB colors
  - `core.events()`: an `async_channel` receiver. Pass each event to `core.apply_event`.

  No iced, alacritty or rusqlite types appear in the API, so a different GUI can replace the iced one.
- **`termroost-iced`**: the iced 0.14 GUI. It contains a custom terminal widget, the pane grid, the sidebars, the task panel, the dialogs and the settings page. It also owns the themes; it hands the terminal colors to the core with `core.set_palette`. JetBrains Mono and Noto Sans Symbols 2 are bundled (both SIL OFL 1.1; licenses in `crates/termroost-iced/fonts`).
- **`termroost-tui`**: the ratatui front end, `terminalrooster-tui`. It draws each terminal's grid into the ratatui buffer. Its palette's default and ANSI colors map back to the outer terminal's own, so programs keep the user's theme.

## Packaging (Linux)

The `.deb` and `.rpm` packages install the `terminalrooster` binary, a desktop entry, icons and AppStream metadata.

**Locally:**

```sh
cargo install --locked cargo-deb cargo-generate-rpm   # once
cargo xtask package                                  # or: cargo xtask package deb / rpm
```

The packages land in `dist/`. A binary only runs on systems whose glibc is at least as new as the one it was built against, so locally built packages suit your own machine. For packages to share, use the CI build.

**On GitHub:** push a tag that matches the version, e.g. `git tag v0.1.0 && git push origin v0.1.0`. The *Release packages* workflow builds both packages on Ubuntu 22.04, so they also run on older distributions, and attaches them to the release. You can also run it from the Actions tab to get the packages as a download.

**New version:**
1. Bump `version` in `Cargo.toml`.
2. Add a `<release>` entry to `packaging/linux/io.github.Davenport_Physics.TerminalRooster.metainfo.xml` (the build checks for it).

**Icons:** they live in `packaging/icons/`. The `terminalrooster-<size>.png` files are the hicolor icons for the packages, `terminalrooster-64.png` is also the window icon, and `terminalrooster.ico` is embedded in the Windows `.exe`.

## Packaging (Windows)

The installer is built with [NSIS](https://nsis.sourceforge.io) from `packaging/windows/terminalrooster.nsi`. It installs `terminalrooster.exe` along with the license files. It also adds Start menu and desktop shortcuts and an *Apps* entry with an uninstaller.

```powershell
winget install NSIS.NSIS    # once; or set MAKENSIS to makensis.exe
cargo xtask package windows
```

The installer lands in `dist/TerminalRooster-<version>-x64-setup.exe`. On Windows, `cargo xtask package` with no format builds the installer. On other platforms it builds the `.deb` and `.rpm`. The installer is not code-signed, so SmartScreen warns the first time it runs.
