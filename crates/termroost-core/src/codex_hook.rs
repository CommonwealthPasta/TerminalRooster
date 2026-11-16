//! Optional Codex hook that reports exactly which conversation (thread) a
//! Codex session is in.
//!
//! Codex has no per-process session record (see [`crate::detect`]), but it
//! runs user hooks with the thread id. With the user's consent TermRoost adds
//! a `SessionStart` and a `UserPromptSubmit` hook to `~/.codex/hooks.json`.
//! The hook copies Codex's event JSON into `<data dir>/codex-hooks/`, only
//! while TermRoost runs (a `.running` marker), and TermRoost matches each
//! report to the Codex terminal in that folder that just submitted a prompt or
//! just started.
//!
//! Codex requires new hooks to be trusted once (`/hooks` inside Codex) before
//! they run; TermRoost cannot do that for the user.

use std::io;
use std::path::{Path, PathBuf};
#[cfg(all(test, unix))]
use std::time::UNIX_EPOCH;
use std::time::{Duration, SystemTime};

use serde::Deserialize;
use serde_json::{Value, json};

use crate::model::PaneId;

/// Marks TermRoost's hook entries so they can be found and removed.
pub const MARKER: &str = "termroost-codex-hook";
const EVENTS: [&str; 2] = ["SessionStart", "UserPromptSubmit"];
const RUNNING: &str = ".running";

/// State of the Codex integration.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CodexHookStatus {
    /// A Codex configuration directory exists.
    pub codex_found: bool,
    /// The file holding TermRoost's hook, if installed.
    pub installed_in: Option<PathBuf>,
    /// The file the hook is (or would be) added to.
    pub hooks_file: PathBuf,
}

pub(crate) fn reports_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("codex-hooks")
}

fn hooks_json(codex_home: &Path) -> PathBuf {
    codex_home.join("hooks.json")
}

pub(crate) fn status(codex_home: &Path) -> CodexHookStatus {
    let file = hooks_json(codex_home);
    let installed = std::fs::read_to_string(&file).is_ok_and(|t| t.contains(MARKER));
    CodexHookStatus {
        codex_found: codex_home.join("config.toml").is_file() || codex_home.join("auth.json").is_file(),
        installed_in: installed.then(|| file.clone()),
        hooks_file: file,
    }
}

/// POSIX hook command. The reports directory is passed as `$1`; `$0` is the marker.
fn posix_command(dir: &Path) -> String {
    let quoted = format!("'{}'", dir.display().to_string().replace('\'', r"'\''"));
    format!(
        r#"sh -c 'd="$1"; [ -f "$d/{RUNNING}" ] || {{ cat >/dev/null; exit 0; }}; f="$d/${{TERMROOST_PANE_ID:-x}}-$$-$(date +%s)"; cat > "$f.tmp" && mv "$f.tmp" "$f.json"; exit 0' {MARKER} {quoted}"#
    )
}

/// Windows hook command: PowerShell with an encoded script, so it works the
/// same whichever shell Codex uses to start it.
fn windows_command(dir: &Path) -> String {
    let d = dir.display().to_string().replace('\'', "''");
    let script = format!(
        "$d = '{d}'\n$in = [Console]::In.ReadToEnd()\n\
         if (-not (Test-Path (Join-Path $d '{RUNNING}'))) {{ exit 0 }}\n\
         $p = if ($env:TERMROOST_PANE_ID) {{ $env:TERMROOST_PANE_ID }} else {{ 'x' }}\n\
         $f = Join-Path $d ('{{0}}-{{1}}-{{2}}' -f $p, $PID, [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds())\n\
         [IO.File]::WriteAllText($f + '.tmp', $in)\nMove-Item -Force ($f + '.tmp') ($f + '.json')\nexit 0\n"
    );
    let utf16: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    format!("powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -EncodedCommand {}", base64(&utf16))
}

fn base64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n =
            (chunk[0] as u32) << 16 | (*chunk.get(1).unwrap_or(&0) as u32) << 8 | *chunk.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn handler(dir: &Path) -> Value {
    json!({
        "type": "command",
        "command": posix_command(dir),
        "commandWindows": windows_command(dir),
        "timeout": 10,
        "async": true
    })
}

fn is_ours(group: &Value) -> bool {
    group["hooks"]
        .as_array()
        .is_some_and(|hs| hs.iter().any(|h| h["command"].as_str().is_some_and(|c| c.contains(MARKER))))
}

fn invalid(msg: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg)
}

/// Adds TermRoost's hook to `hooks.json` (created if missing). Existing hooks
/// are kept; the original file is backed up once.
pub(crate) fn install(codex_home: &Path, data_dir: &Path) -> io::Result<PathBuf> {
    let file = hooks_json(codex_home);
    let dir = reports_dir(data_dir);
    std::fs::create_dir_all(&dir)?;
    let mut root: Value = match std::fs::read_to_string(&file) {
        Ok(text) => {
            let backup = codex_home.join("hooks.json.termroost-backup");
            if !backup.exists() {
                std::fs::write(&backup, &text)?;
            }
            serde_json::from_str(&text).map_err(|e| invalid(format!("{} is not valid JSON: {e}", file.display())))?
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => json!({}),
        Err(e) => return Err(e),
    };
    let obj = root.as_object_mut().ok_or_else(|| invalid(format!("{} is not a JSON object", file.display())))?;
    let hooks = obj.entry("hooks").or_insert_with(|| json!({}));
    let hooks = hooks.as_object_mut().ok_or_else(|| invalid("\"hooks\" is not an object".into()))?;
    for event in EVENTS {
        let groups = hooks.entry(event).or_insert_with(|| json!([]));
        let groups = groups.as_array_mut().ok_or_else(|| invalid(format!("\"hooks.{event}\" is not an array")))?;
        groups.retain(|g| !is_ours(g));
        groups.push(json!({ "hooks": [handler(&dir)] }));
    }
    std::fs::create_dir_all(codex_home)?;
    std::fs::write(&file, serde_json::to_string_pretty(&root).expect("json") + "\n")?;
    tracing::info!(file = %file.display(), "installed Codex hook");
    Ok(file)
}

/// Removes TermRoost's hook entries; deletes `hooks.json` if nothing else is left.
pub(crate) fn remove(codex_home: &Path) -> io::Result<()> {
    let file = hooks_json(codex_home);
    let text = match std::fs::read_to_string(&file) {
        Ok(t) => t,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    let mut root: Value =
        serde_json::from_str(&text).map_err(|e| invalid(format!("{} is not valid JSON: {e}", file.display())))?;
    if let Some(hooks) = root.get_mut("hooks").and_then(Value::as_object_mut) {
        for event in EVENTS {
            if let Some(groups) = hooks.get_mut(event).and_then(Value::as_array_mut) {
                groups.retain(|g| !is_ours(g));
                if groups.is_empty() {
                    hooks.remove(event);
                }
            }
        }
    }
    let empty = root
        .as_object()
        .is_some_and(|o| o.iter().all(|(k, v)| k == "hooks" && v.as_object().is_some_and(|h| h.is_empty())));
    if empty {
        std::fs::remove_file(&file)?;
    } else {
        std::fs::write(&file, serde_json::to_string_pretty(&root).expect("json") + "\n")?;
    }
    tracing::info!(file = %file.display(), "removed Codex hook");
    Ok(())
}

/// Lets the hook write reports while TermRoost runs.
pub(crate) fn set_running(data_dir: &Path, running: bool) {
    let dir = reports_dir(data_dir);
    let marker = dir.join(RUNNING);
    if running {
        let _ = std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(&marker, b""));
    } else {
        let _ = std::fs::remove_file(marker);
    }
}

/// One event reported by the hook.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct HookReport {
    pub session_id: String,
    pub cwd: PathBuf,
    pub event: String,
    pub at: SystemTime,
    /// `TERMROOST_PANE_ID` as the hook saw it. Codex's shared daemon may run
    /// hooks with another terminal's environment, so this is only a hint.
    pub env_pane: Option<PaneId>,
}

#[derive(Deserialize)]
struct Payload {
    session_id: String,
    cwd: PathBuf,
    #[serde(default)]
    hook_event_name: String,
}

/// Reads and deletes the reports written since the last call; reports older
/// than `since` (e.g. written while TermRoost was not running) are dropped.
pub(crate) fn drain(data_dir: &Path, since: SystemTime) -> Vec<HookReport> {
    let dir = reports_dir(data_dir);
    let Ok(entries) = std::fs::read_dir(&dir) else { return Vec::new() };
    let now = SystemTime::now();
    let mut out = Vec::new();
    for e in entries.flatten() {
        let path = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        let modified = e.metadata().and_then(|m| m.modified()).unwrap_or(now);
        if name.ends_with(".tmp") {
            // A hook killed mid-write leaves these behind.
            if now.duration_since(modified).unwrap_or_default() > Duration::from_secs(60) {
                let _ = std::fs::remove_file(&path);
            }
            continue;
        }
        let Some(stem) = name.strip_suffix(".json") else { continue };
        let report = std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str::<Payload>(&t).ok());
        let _ = std::fs::remove_file(&path);
        let Some(p) = report else { continue };
        if modified < since {
            continue;
        }
        // `<pane or x>-<pid>-<time>`; the file time is the event time.
        let env_pane = stem.split('-').next().and_then(|s| s.parse().ok()).map(PaneId);
        out.push(HookReport { session_id: p.session_id, cwd: p.cwd, event: p.hook_event_name, at: modified, env_pane });
    }
    out.sort_by_key(|r| r.at);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_reference() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn install_merges_and_remove_restores() {
        let tmp = tempfile::tempdir().unwrap();
        let codex = tmp.path().join("codex");
        let data = tmp.path().join("data");
        std::fs::create_dir_all(&codex).unwrap();
        assert!(!status(&codex).codex_found);
        std::fs::write(codex.join("config.toml"), "model = \"x\"\n").unwrap();
        assert!(status(&codex).codex_found);
        assert_eq!(status(&codex).installed_in, None);

        // A user hook that must survive.
        let mine = json!({"hooks": {"SessionStart": [{"matcher": "startup", "hooks": [{"type": "command", "command": "echo hi"}]}]}});
        std::fs::write(codex.join("hooks.json"), mine.to_string()).unwrap();

        let file = install(&codex, &data).unwrap();
        install(&codex, &data).unwrap(); // idempotent
        assert_eq!(status(&codex).installed_in, Some(file.clone()));
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(v["hooks"]["SessionStart"].as_array().unwrap().len(), 2);
        assert_eq!(v["hooks"]["UserPromptSubmit"].as_array().unwrap().len(), 1);
        assert_eq!(v["hooks"]["SessionStart"][0]["hooks"][0]["command"], "echo hi");
        assert!(codex.join("hooks.json.termroost-backup").is_file());

        remove(&codex).unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(v, mine);
        assert_eq!(status(&codex).installed_in, None);

        // Without user hooks, removing deletes the file we created.
        std::fs::remove_file(&file).unwrap();
        install(&codex, &data).unwrap();
        remove(&codex).unwrap();
        assert!(!file.exists());
    }

    #[test]
    fn invalid_hooks_json_is_left_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let codex = tmp.path().join("codex");
        std::fs::create_dir_all(&codex).unwrap();
        std::fs::write(codex.join("hooks.json"), "{ broken").unwrap();
        assert!(install(&codex, &tmp.path().join("data")).is_err());
        assert_eq!(std::fs::read_to_string(codex.join("hooks.json")).unwrap(), "{ broken");
    }

    /// Runs the real POSIX hook command the way Codex would.
    #[cfg(unix)]
    #[test]
    fn posix_hook_writes_reports_only_while_running() {
        use std::io::Write;
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path().join("da'ta dir"); // quotes and spaces survive
        std::fs::create_dir_all(reports_dir(&data)).unwrap();
        let run = |pane: Option<&str>| {
            let mut cmd = std::process::Command::new("sh");
            cmd.arg("-c").arg(posix_command(&reports_dir(&data))).stdin(std::process::Stdio::piped());
            cmd.env_remove("TERMROOST_PANE_ID");
            if let Some(p) = pane {
                cmd.env("TERMROOST_PANE_ID", p);
            }
            let mut child = cmd.spawn().unwrap();
            child
                .stdin
                .take()
                .unwrap()
                .write_all(br#"{"session_id":"01a1e20f-eb7a-7971-a81c-602f1b0f42f0","cwd":"/w","hook_event_name":"UserPromptSubmit","prompt":"hi"}"#)
                .unwrap();
            assert!(child.wait().unwrap().success());
        };
        run(Some("7"));
        assert!(drain(&data, UNIX_EPOCH).is_empty(), "nothing is written while TermRoost is not running");
        set_running(&data, true);
        run(Some("7"));
        run(None);
        let reports = drain(&data, UNIX_EPOCH);
        assert_eq!(reports.len(), 2);
        assert!(
            reports.iter().all(|r| r.session_id == "01a1e20f-eb7a-7971-a81c-602f1b0f42f0" && r.cwd == Path::new("/w"))
        );
        assert_eq!(reports.iter().filter(|r| r.env_pane == Some(PaneId(7))).count(), 1);
        assert!(drain(&data, UNIX_EPOCH).is_empty(), "reports are consumed");
        set_running(&data, false);
    }
}
