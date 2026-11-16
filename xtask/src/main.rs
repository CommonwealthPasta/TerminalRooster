//! Build automation for TerminalRooster.
//!
//! ```text
//! cargo xtask package [deb|rpm|all|windows]   build release packages into dist/
//! ```
//!
//! With no format, Windows builds the installer and other systems build the
//! `.deb` and `.rpm` (`all`).
//!
//! Linux packaging builds the release binary, stages every installed file under
//! `target/release/packaging-root/` (binary, desktop entry, AppStream metadata and the
//! hicolor icons from `packaging/icons/`), then runs `cargo deb`
//! and `cargo generate-rpm`, whose file lists live in
//! `crates/termroost-iced/Cargo.toml`.
//!
//! The Windows installer is built by NSIS from `packaging/windows/terminalrooster.nsi`.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

const APP_ID: &str = "io.github.Davenport_Physics.TerminalRooster";
/// hicolor sizes shipped; each comes from `packaging/icons/terminalrooster-<size>.png`.
const ICON_SIZES: [u32; 10] = [16, 24, 32, 48, 64, 96, 128, 192, 256, 512];

type Result<T> = std::result::Result<T, String>;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["package"] if cfg!(windows) => package_windows(),
        ["package"] | ["package", "all"] => package(true, true),
        ["package", "deb"] => package(true, false),
        ["package", "rpm"] => package(false, true),
        ["package", "windows"] => package_windows(),
        _ => {
            eprintln!("usage: cargo xtask package [deb|rpm|all|windows]");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("workspace root").to_path_buf()
}

fn cargo() -> Command {
    let mut cmd = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    cmd.current_dir(root());
    cmd
}

fn run(cmd: &mut Command) -> Result<()> {
    eprintln!("$ {cmd:?}");
    let status = cmd.status().map_err(|e| format!("could not run {:?}: {e}", cmd.get_program()))?;
    if status.success() { Ok(()) } else { Err(format!("{:?} failed ({status})", cmd.get_program())) }
}

/// Fails early with install instructions when a cargo subcommand is missing.
fn require_tool(subcommand: &str, crate_name: &str) -> Result<()> {
    let ok = cargo().args([subcommand, "--version"]).output().is_ok_and(|o| o.status.success());
    if ok {
        Ok(())
    } else {
        Err(format!("`cargo {subcommand}` is not installed; run: cargo install --locked {crate_name}"))
    }
}

/// The workspace version, from `[workspace.package]`.
fn version() -> Result<String> {
    let text = std::fs::read_to_string(root().join("Cargo.toml")).map_err(|e| e.to_string())?;
    let section = text.split("[workspace.package]").nth(1).ok_or("no [workspace.package] in Cargo.toml")?;
    section
        .lines()
        .find_map(|l| l.trim().strip_prefix("version = \"").and_then(|v| v.strip_suffix('"')))
        .map(str::to_owned)
        .ok_or_else(|| "no version in [workspace.package]".into())
}

fn package(deb: bool, rpm: bool) -> Result<()> {
    if deb {
        require_tool("deb", "cargo-deb")?;
    }
    if rpm {
        require_tool("generate-rpm", "cargo-generate-rpm")?;
    }
    let root = root();
    let version = version()?;
    let metainfo = root.join(format!("packaging/linux/{APP_ID}.metainfo.xml"));
    let metainfo_text = std::fs::read_to_string(&metainfo).map_err(|e| format!("{}: {e}", metainfo.display()))?;
    if !metainfo_text.contains(&format!("<release version=\"{version}\"")) {
        return Err(format!(
            "{} has no <release version=\"{version}\"> entry; add one for this version",
            metainfo.display()
        ));
    }

    run(cargo().args(["build", "--release", "--locked", "-p", "termroost-iced"]))?;
    stage(&root)?;

    let dist = root.join("dist");
    std::fs::create_dir_all(&dist).map_err(|e| e.to_string())?;
    if deb {
        // Already stripped by the release profile (debug info only; symbols kept).
        run(cargo().args(["deb", "-p", "termroost-iced", "--no-build", "--no-strip", "--output"]).arg(&dist))?;
    }
    if rpm {
        run(cargo().args(["generate-rpm", "-p", "crates/termroost-iced", "--output"]).arg(&dist))?;
    }
    eprintln!("\npackages in {}:", dist.display());
    let mut files: Vec<_> = std::fs::read_dir(&dist).map_err(|e| e.to_string())?.flatten().map(|e| e.path()).collect();
    files.sort();
    for f in files.iter().filter(|f| f.to_string_lossy().contains(&version)) {
        eprintln!("  {}", f.display());
    }
    Ok(())
}

/// Builds `dist/TerminalRooster-<version>-<arch>-setup.exe` with NSIS.
fn package_windows() -> Result<()> {
    if !cfg!(windows) {
        return Err("the Windows installer packages the native terminalrooster.exe; build it on Windows".into());
    }
    let makensis = find_makensis()?;
    let root = root();
    let version = version()?;
    let quad = version_quad(&version)?;
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        other => other,
    };

    run(cargo().args(["build", "--release", "--locked", "-p", "termroost-iced"]))?;

    let dist = root.join("dist");
    std::fs::create_dir_all(&dist).map_err(|e| e.to_string())?;
    let out = dist.join(format!("TerminalRooster-{version}-{arch}-setup.exe"));
    run(Command::new(&makensis)
        .arg("/V2")
        .arg(format!("/DVERSION={version}"))
        .arg(format!("/DVERSION_QUAD={quad}"))
        .arg(format!("/DROOT={}", root.display()))
        .arg(format!("/DOUTFILE={}", out.display()))
        .arg(root.join("packaging/windows/terminalrooster.nsi")))?;
    eprintln!("\ninstaller: {}", out.display());
    Ok(())
}

/// `makensis` from `MAKENSIS`, `PATH` or the default NSIS install folders.
fn find_makensis() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("MAKENSIS") {
        return Ok(path.into());
    }
    if Command::new("makensis").arg("/VERSION").output().is_ok_and(|o| o.status.success()) {
        return Ok("makensis".into());
    }
    ["ProgramFiles(x86)", "ProgramFiles"]
        .iter()
        .filter_map(std::env::var_os)
        .map(|dir| Path::new(&dir).join("NSIS").join("makensis.exe"))
        .find(|p| p.is_file())
        .ok_or_else(|| "NSIS is not installed; run `winget install NSIS.NSIS` or set MAKENSIS to makensis.exe".into())
}

/// Windows version resources need four numbers: `0.2.0-beta.1` becomes `0.2.0.0`.
fn version_quad(version: &str) -> Result<String> {
    let core = version.split(['-', '+']).next().unwrap_or(version);
    let parts: Vec<&str> = core.split('.').collect();
    if parts.len() != 3 || parts.iter().any(|p| p.parse::<u16>().is_err()) {
        return Err(format!("version {version} is not MAJOR.MINOR.PATCH"));
    }
    Ok(format!("{core}.0"))
}

/// Lays out every installed file under `target/release/packaging-root`.
fn stage(root: &Path) -> Result<()> {
    let stage = root.join("target/release/packaging-root");
    if stage.exists() {
        std::fs::remove_dir_all(&stage).map_err(|e| e.to_string())?;
    }
    let copy = |from: PathBuf, to: &str| -> Result<()> {
        let to = stage.join(to);
        std::fs::create_dir_all(to.parent().unwrap()).map_err(|e| e.to_string())?;
        std::fs::copy(&from, &to).map_err(|e| format!("copying {}: {e}", from.display()))?;
        Ok(())
    };
    let exe = if cfg!(windows) { "terminalrooster.exe" } else { "terminalrooster" };
    copy(root.join("target/release").join(exe), "usr/bin/terminalrooster")?;
    copy(root.join(format!("packaging/linux/{APP_ID}.desktop")), &format!("usr/share/applications/{APP_ID}.desktop"))?;
    copy(
        root.join(format!("packaging/linux/{APP_ID}.metainfo.xml")),
        &format!("usr/share/metainfo/{APP_ID}.metainfo.xml"),
    )?;

    for size in ICON_SIZES {
        let src = root.join(format!("packaging/icons/terminalrooster-{size}.png"));
        let icon = image::open(&src).map_err(|e| format!("{}: {e}", src.display()))?;
        if (icon.width(), icon.height()) != (size, size) {
            return Err(format!("{} is {}x{}, expected {size}x{size}", src.display(), icon.width(), icon.height()));
        }
        copy(src, &format!("usr/share/icons/hicolor/{size}x{size}/apps/{APP_ID}.png"))?;
    }
    eprintln!("staged files in {}", stage.display());
    Ok(())
}
