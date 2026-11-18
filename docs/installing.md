# Installing and running

## Linux

```sh
cargo run --release -p termroost-iced
```

To build `.deb` and `.rpm` packages, see [Packaging (Linux)](development.md#packaging-linux).

Running a build straight from `target/` (not from a package) writes a hidden desktop entry and the icons into `~/.local/share`. KDE and GNOME need that entry to show Terminal Rooster's icon for its window on Wayland. Such a build runs under its own id, `io.github.Davenport_Physics.TerminalRooster.Devel`, so its entry never hides an installed package's menu entry, and the two show up separately in the taskbar. Set `TERMINALROOSTER_NO_DESKTOP_ENTRY=1` to skip writing it.

## Windows

### Installer

Run `TerminalRooster-<version>-x64-setup.exe` (see [Packaging (Windows)](development.md#packaging-windows) to build it). It installs for the current user only, so it needs no administrator rights. It installs to `%LOCALAPPDATA%\Programs\TerminalRooster` and adds a Start menu shortcut, plus an optional desktop shortcut. Uninstall it from *Settings → Apps*. Uninstalling keeps your data.

### Building it yourself

1. Install Rust with [rustup](https://rustup.rs) (default MSVC toolchain).
2. Install the Visual Studio Build Tools with the "Desktop development with C++" workload. SQLite is compiled from source.
3. In the repository folder, run:

   ```powershell
   cargo build --release
   .\target\release\terminalrooster.exe
   ```

Windows 10 1809 or newer is required, for ConPTY. Terminals use `pwsh` if it is installed, otherwise Windows PowerShell, otherwise `cmd`. Release builds do not open a console window.

### Bash on Windows

Settings → Terminal lists the shells it finds (Git Bash, MSYS2, each WSL distribution, PowerShell, cmd), for the default shell and for each project. Without any bash installed, **Terminal Rooster bash → Install and use** downloads a pinned Git for Windows release (bash with coreutils, vim, less, ssh and git), checks its SHA-256, and unpacks it with Windows' own `curl.exe` and `tar.exe` into `%LOCALAPPDATA%\TerminalRooster\data\bash`. Nothing outside that folder changes. Delete the folder to remove it.

## Where data lives

| Platform | Location |
|---|---|
| Linux | `~/.local/share/terminalrooster/terminalrooster.db` |
| Windows | `%APPDATA%\TerminalRooster\data\terminalrooster.db` |

Set `TERMINALROOSTER_DATA_DIR` to use another directory. Only one instance can run per data directory.

## Logs

Terminal Rooster logs with `tracing`, to stderr and to a daily file that keeps the last 7 days:

| Platform | Log location |
|---|---|
| Linux | `~/.local/share/terminalrooster/logs/` |
| Windows | `%APPDATA%\TerminalRooster\data\logs\` |

Panics are written to the log with a backtrace. The level comes from `TERMINALROOSTER_LOG`, then `RUST_LOG`; the default is `info`. For example, `TERMINALROOSTER_LOG=debug` logs every command, and `TERMINALROOSTER_LOG=terminalrooster=trace` logs every UI message. Keystrokes are never logged.
