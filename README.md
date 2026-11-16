# TermRoost

A project-based terminal manager for running coding agents (Claude Code, Codex, Grok, …) side by side.

- **Projects** on the left, each with a working directory.
- **Tabs** per project: vertical, renamable (double-click). Each tab holds terminals in horizontal/vertical **splits**.
- **Tasks** per project in the right panel. You can mark them done and link each one to a tab; tabs show a badge with their open-task count.
- **Everything is persisted in SQLite** as it changes: projects, tabs, split layouts, focus, tasks, window geometry and terminal scrollback. After a crash or a normal close, the app reopens as you left it.

## Running

### Linux

```sh
cargo run --release -p termroost-iced
```

Running a build straight from `target/` (not from a package) writes a hidden desktop entry and the icons into `~/.local/share`. KDE and GNOME need that entry to show TermRoost's icon for its window on Wayland. Once a package is installed, TermRoost removes this entry again. Set `TERMROOST_NO_DESKTOP_ENTRY=1` to skip it.

### Windows (native build)

1. Install Rust with [rustup](https://rustup.rs) (default MSVC toolchain).
2. Install the Visual Studio Build Tools with the "Desktop development with C++" workload. SQLite is compiled from source.
3. In the `termroost` folder, run:

   ```powershell
   cargo build --release
   .\target\release\termroost.exe
   ```

Windows 10 1809 or newer is required, for ConPTY. Terminals use `pwsh` if it is installed, otherwise Windows PowerShell, otherwise `cmd`. Release builds do not open a console window.

### Where data lives

| Platform | Location |
|---|---|
| Linux | `~/.local/share/termroost/termroost.db` |
| Windows | `%APPDATA%\termroost\data\termroost.db` |

Set `TERMROOST_DATA_DIR` to use another directory. Only one instance can run per data directory.

TermRoost used to be called *agentdeck*. On first start it moves the old `agentdeck` data folder's database into the folder above. Close the old app first; TermRoost waits and says so if it is still running.

## Themes

Open **Settings** with the ⚙ in the projects header.

- **Interface theme:** click **Use** on a theme. **Dark** (Tokyo Night) and **Light** (GitHub-light style) are built in.
- **Match the system's light/dark setting** (on by default): the app switches between the last light theme and the last dark theme you used, following the OS live.
- **Terminal colors:** match the interface, or always use one theme's terminal colors. For example, you can have dark terminals with a light interface.
- **Custom themes:** **Duplicate** a theme, then edit it.
  - You can change its name, its interface colors (background, text, accent, success, warning, danger) and its terminal colors (foreground, background, cursor, selection and the 16 ANSI colors).
  - Type a `#rrggbb` color, or click a color's square to pick one: a palette of ready-made colors, the colors the theme already uses, and hue/saturation/lightness sliders.
  - Edits apply live and are saved automatically.

Terminals switch colors instantly, including their scrollback. Programs that ask the terminal for its colors get the new ones. Coding agents usually have their own light/dark setting; for example, set Claude Code's `/theme` to match.

## Logging

TermRoost logs with `tracing`, to stderr and to a daily file that keeps the last 7 days:

| Platform | Log location |
|---|---|
| Linux | `~/.local/share/termroost/logs/` |
| Windows | `%APPDATA%\termroost\data\logs\` |

Panics are written to the log with a backtrace. The level comes from `TERMROOST_LOG`, then `RUST_LOG`; the default is `info`. For example, `TERMROOST_LOG=debug` logs every command, and `TERMROOST_LOG=TermRoost=trace` logs every UI message. Keystrokes are never logged.

## Restoring after a restart or crash

Terminal processes cannot survive a crash, and they end when the app closes. On reopen, every terminal gets the following:

- Its **saved scrollback**, with colors. It is saved every 5 seconds and on close, and shown under a "restored session" banner.
- A **fresh shell** in the tab's working directory.
- Its **resume command**, which runs automatically.

Restored tabs start their shells the first time you open them, so dozens of agents don't all launch at once.

### Exact agent sessions, not "continue last"

`--continue` and `resume --last` reopen the most recent conversation in a folder, which is wrong when several agents work in one project. TermRoost watches each terminal's processes instead (every 5 seconds, and once more on shutdown). It finds the coding agent running there and reads the agent's session id from the agent's own records:

| Agent | Where the session id comes from | Restore command |
|---|---|---|
| Claude Code | `~/.claude/sessions/<pid>.json`, rewritten on `/clear` | `claude --resume <id>` |
| Grok | `~/.grok/active_sessions.json` (`pid` → `session_id`) | `grok --resume <id>` |
| Codex | Codex's thread table (`~/.codex/state_*.sqlite`): the newest user thread in the process's folder started after the process, or the id in `codex resume <id>` | `codex resume <id>` |

- **How agents are found:** it works whether you start the agent with **Start agent** or type `claude` yourself. It follows `/clear`, `/new` and `/resume`, and it forgets the agent when you quit it.
- **Missing sessions:** if a session never got a message, so nothing is saved to resume, the agent is started fresh instead.
- **Extra arguments:** arguments set in terminal settings (e.g. `--model opus`) are reused on resume.
- **Codex accuracy:** Codex runs every terminal through one shared daemon and keeps no per-process record, so by default its session is inferred. That is exact with one Codex per folder. For exact tracking, add the optional **Codex hook**. TermRoost offers it once at startup, and you can also add it from Settings → Integrations. Codex only runs hooks you trust, so TermRoost then opens a small Codex window: type `/hooks` there and trust it. The hook goes in `~/.codex/hooks.json`, your other hooks are kept, and it only writes while TermRoost runs.
- **Debugging detection:** `cargo run -p termroost-core --example detect_agents -- <shell pid>…` prints what is detected under any shell.

## Shortcuts

Terminals receive every plain `Ctrl+letter`. App shortcuts use `Ctrl+Shift` or `Alt`.

| Keys | Action |
|---|---|
| Ctrl+Shift+T | New tab |
| Ctrl+Shift+E / Ctrl+Shift+O | Split right / split down |
| Ctrl+Shift+W | Close terminal (the last one in a tab can't be closed; delete the tab) |
| Ctrl+Tab / Ctrl+Shift+Tab, Ctrl+PgDn / Ctrl+PgUp | Next / previous tab |
| Ctrl+Shift+PgDn / Ctrl+Shift+PgUp | Next / previous project |
| Alt+Arrows | Move between terminals |
| Ctrl+Shift+C / Ctrl+Shift+V, Shift+Insert | Copy / paste |
| Right click | Copy the selection, or paste when nothing is selected |
| Shift+PgUp / Shift+PgDn, mouse wheel | Scroll history |
| Ctrl+Shift+K | Show / hide tasks |
| Ctrl+Shift+= / - / 0 | Font size (also in Settings) |
| Shift+Enter | Newline in Claude Code / Codex (sends `ESC CR`) |

## Architecture

Cargo workspace with two crates:

- **`termroost-core`**: the program. No GUI dependencies. It owns the state, the SQLite store (on a writer thread), the terminals (`alacritty_terminal` with PTY / ConPTY) and the autosave + Codex detection thread. A front end uses only this API:
  - `Core::open(Config)`
  - `core.state()`: projects, tabs, panes, tasks
  - `core.dispatch(Command)`: every change
  - `core.terminal(pane)`: returns a `TerminalHandle` for keys, paste, mouse, selection, resize, and `with_content` for a GUI-neutral grid with resolved RGB colors
  - `core.events()`: an `async_channel` receiver. Pass each event to `core.apply_event`.

  No iced, alacritty or rusqlite types appear in the API, so a different GUI can replace the iced one.
- **`termroost-iced`**: the iced 0.14 GUI. It contains a custom terminal widget, the pane grid, the sidebars, the task panel, the dialogs and the settings page. It also owns the themes; it hands the terminal colors to the core with `core.set_palette`. JetBrains Mono and Noto Sans Symbols 2 are bundled (both SIL OFL 1.1; licenses in `crates/termroost-iced/fonts`).

## Packaging (Linux)

The `.deb` and `.rpm` packages install the `termroost` binary, a desktop entry, icons and AppStream metadata.

**Locally:**

```sh
cargo install --locked cargo-deb cargo-generate-rpm   # once
cargo xtask package                                  # or: cargo xtask package deb / rpm
```

The packages land in `dist/`. A binary only runs on systems whose glibc is at least as new as the one it was built against, so locally built packages suit your own machine. For packages to share, use the CI build.

**On GitHub:** push a tag that matches the version, e.g. `git tag v0.1.0 && git push origin v0.1.0`. The *Release packages* workflow builds both packages on Ubuntu 22.04, so they also run on older distributions, and attaches them to the release. You can also run it from the Actions tab to get the packages as a download.

**New version:**
1. Bump `version` in `Cargo.toml`.
2. Add a `<release>` entry to `packaging/linux/io.github.Davenport_Physics.TermRoost.metainfo.xml` (the build checks for it).

**Icons:** they live in `packaging/icons/`. The `termroost-<size>.png` files are the hicolor icons for the packages, `termroost-64.png` is also the window icon, and `termroost.ico` is embedded in the Windows `.exe`.

## Development

```sh
cargo test --workspace          # unit tests + real-PTY/real-DB integration tests, including an abort() crash test
cargo run -p termroost-core --example seed -- /some/dir   # demo data (use with TERMROOST_DATA_DIR)
```

## License

TermRoost is released under the [MIT License](LICENSE).

The bundled fonts keep their own license, the SIL Open Font License 1.1: JetBrains Mono and Noto Sans Symbols 2. Their license texts are in `crates/termroost-iced/fonts/`.
