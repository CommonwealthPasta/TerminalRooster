//! Git awareness for sessions: the branch a directory is on, and working
//! trees made beside a repository.
//!
//! [`Repo::discover`] and [`Repo::head`] only read files under `.git`, so a
//! front end can poll them for every visible session. Everything else runs
//! the `git` program.

use std::collections::HashSet;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// The working tree a directory belongs to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Repo {
    /// Top directory of the working tree.
    pub worktree: PathBuf,
    /// The working tree's own git directory: `<worktree>/.git`, or
    /// `<repository>/.git/worktrees/<name>` for a linked worktree.
    pub git_dir: PathBuf,
    /// The git directory shared by all working trees of the repository.
    pub common_dir: PathBuf,
}

/// What a working tree has checked out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Head {
    Branch(String),
    /// No branch; at this commit (abbreviated).
    Detached(String),
}

impl std::fmt::Display for Head {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Head::Branch(b) => f.write_str(b),
            Head::Detached(c) => write!(f, "{c} (detached)"),
        }
    }
}

/// Where a directory stands in git.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Status {
    pub repo: Repo,
    /// `None` when HEAD could not be read.
    pub head: Option<Head>,
}

impl Status {
    /// Looks up the repository holding `dir` and its checked-out branch.
    /// `None` when `dir` is not in a repository or does not exist (such as a
    /// deleted worktree; a parent repository does not count then).
    pub fn of(dir: &Path) -> Option<Status> {
        if !dir.is_dir() {
            return None;
        }
        let repo = Repo::discover(dir)?;
        let head = repo.head();
        Some(Status { repo, head })
    }
}

impl Repo {
    /// The working tree holding `dir`, found by looking for `.git` in it and
    /// its parents. `None` outside a repository.
    pub fn discover(dir: &Path) -> Option<Repo> {
        for dir in dir.ancestors() {
            let dot_git = dir.join(".git");
            let Ok(meta) = std::fs::metadata(&dot_git) else { continue };
            let git_dir = if meta.is_dir() {
                dot_git
            } else {
                // A linked worktree or submodule: `gitdir: <path>`.
                let text = std::fs::read_to_string(&dot_git).ok()?;
                let target = text.lines().next()?.strip_prefix("gitdir:")?.trim();
                normalize(&dir.join(target))
            };
            if !git_dir.join("HEAD").is_file() {
                return None;
            }
            let common_dir = match std::fs::read_to_string(git_dir.join("commondir")) {
                Ok(rel) => normalize(&git_dir.join(rel.trim())),
                Err(_) => git_dir.clone(),
            };
            return Some(Repo { worktree: dir.to_path_buf(), git_dir, common_dir });
        }
        None
    }

    /// The checked-out branch, or the commit when detached.
    pub fn head(&self) -> Option<Head> {
        let text = std::fs::read_to_string(self.git_dir.join("HEAD")).ok()?;
        let text = text.trim();
        if let Some(target) = text.strip_prefix("ref:") {
            let target = target.trim();
            // The reftable backend keeps HEAD elsewhere and leaves this stub.
            if target == "refs/heads/.invalid" {
                return self.head_from_git();
            }
            return Some(Head::Branch(target.strip_prefix("refs/heads/").unwrap_or(target).to_owned()));
        }
        let is_hex = text.len() >= 7 && text.bytes().all(|b| b.is_ascii_hexdigit());
        is_hex.then(|| Head::Detached(text[..7].to_owned()))
    }

    fn head_from_git(&self) -> Option<Head> {
        if let Ok(branch) = git(&self.worktree, &["symbolic-ref", "--short", "-q", "HEAD"]) {
            return Some(Head::Branch(branch.trim().to_owned()));
        }
        let commit = git(&self.worktree, &["rev-parse", "--short", "HEAD"]).ok()?;
        Some(Head::Detached(commit.trim().to_owned()))
    }

    /// Whether this is a linked worktree (made with `git worktree add`).
    pub fn is_linked(&self) -> bool {
        self.git_dir != self.common_dir
    }

    /// The repository's main working tree, when its git directory is the
    /// usual `.git` inside it.
    pub fn main_worktree(&self) -> Option<PathBuf> {
        let parent = self.common_dir.parent()?;
        (self.common_dir.file_name()? == ".git").then(|| parent.to_path_buf())
    }

    /// Branches and working trees, for planning a new worktree.
    pub fn branches(&self) -> Result<Branches, String> {
        let refs = git(&self.worktree, &["for-each-ref", "--format=%(refname)", "refs/heads", "refs/remotes"])?;
        let list = git(&self.worktree, &["worktree", "list", "--porcelain"])?;
        Ok(Branches::parse(&refs, &list))
    }

    /// A free path for a worktree of `branch`, side by side with this working
    /// tree (so on the same drive): `<parent>/<repository>-<branch>`, named
    /// after the main working tree's folder.
    pub fn suggest_worktree_path(&self, branch: &str) -> Option<PathBuf> {
        let parent = self.worktree.parent()?;
        let main = self.main_worktree().unwrap_or_else(|| self.worktree.clone());
        let name = main.file_name()?.to_string_lossy().into_owned();
        let stem = format!("{name}-{}", slug(branch));
        let mut path = parent.join(&stem);
        let mut n = 2;
        while path.exists() {
            path = parent.join(format!("{stem}-{n}"));
            n += 1;
        }
        Some(path)
    }

    /// Where a session working in `dir` (inside this working tree) belongs in
    /// the working tree at `other`: the same subdirectory when it exists
    /// there, else the top.
    pub fn same_place_in(&self, dir: &Path, other: &Path) -> PathBuf {
        match dir.strip_prefix(&self.worktree) {
            Ok(rel) if !rel.as_os_str().is_empty() && other.join(rel).is_dir() => other.join(rel),
            _ => other.to_path_buf(),
        }
    }

    /// Makes a working tree for `branch` at `path`, as `plan` says. `base` is
    /// where a new branch starts (`None`: this working tree's HEAD).
    pub fn add_worktree(&self, path: &Path, branch: &str, plan: &Plan, base: Option<&str>) -> Result<(), String> {
        let path = path.to_string_lossy();
        let path: &str = &path;
        let mut args: Vec<&str> = vec!["worktree", "add"];
        match plan {
            Plan::NewBranch => {
                args.extend(["-b", branch, path]);
                args.extend(base);
            }
            Plan::Existing => args.extend([path, branch]),
            Plan::Track(remote) => args.extend(["--track", "-b", branch, path, remote]),
            Plan::CheckedOut(dir) => {
                return Err(format!("{branch} is already checked out in {}", dir.display()));
            }
        }
        git(&self.worktree, &args).map(drop)
    }

    /// The uncommitted changes in the working tree at `path`: changed files,
    /// and new ones git does not ignore. `git worktree remove` keeps a working
    /// tree with any of these unless forced. (A new folder counts once.)
    pub fn changes(path: &Path) -> Result<usize, String> {
        Ok(git(path, &["status", "--porcelain"])?.lines().filter(|l| !l.trim().is_empty()).count())
    }

    /// Removes the linked working tree at `path`, deleting its folder; `force`
    /// deletes uncommitted changes too. Branches are kept. git runs from the
    /// main working tree, so its own working directory is not in the folder
    /// it deletes (Windows cannot delete a folder a process is working in).
    pub fn remove_worktree(&self, path: &Path, force: bool) -> Result<(), String> {
        let from = self.main_worktree().unwrap_or_else(|| self.common_dir.clone());
        let path = path.to_string_lossy();
        let mut args = vec!["worktree", "remove"];
        if force {
            args.push("--force");
        }
        args.push(&path);
        git(&from, &args).map(drop)
    }

    /// How this working tree's commits stand against the remotes, as of the
    /// last fetch. `unpushed` covers the newest `limit` commits of HEAD, or
    /// of every branch when `all`.
    pub fn tracking(&self, all: bool, limit: usize) -> Result<Tracking, String> {
        let remotes: Vec<String> = git(&self.worktree, &["remote"])?
            .lines()
            .map(str::trim)
            .filter(|r| !r.is_empty())
            .map(Into::into)
            .collect();
        if remotes.is_empty() {
            return Ok(Tracking::default());
        }
        let mut t = Tracking::default();
        if let Some(Head::Branch(branch)) = self.head() {
            let format = "--format=%(upstream:short)%09%(upstream:remotename)";
            let line = git(&self.worktree, &["for-each-ref", format, &format!("refs/heads/{branch}")])?;
            let mut parts = line.trim_end().split('\t');
            let (upstream, remote) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
            // An upstream whose remote branch was deleted fails to count, and counts as none.
            if !upstream.is_empty()
                && let Ok(counts) = git(&self.worktree, &["rev-list", "--left-right", "--count", "HEAD...@{upstream}"])
            {
                let mut n = counts.split_whitespace().map(|n| n.parse().unwrap_or(0));
                (t.ahead, t.behind) = (n.next().unwrap_or(0), n.next().unwrap_or(0));
                t.upstream = Some(upstream.to_owned());
                t.remote = Some(remote.to_owned()).filter(|r| !r.is_empty());
            }
        }
        if t.remote.is_none() {
            t.remote = remotes.iter().find(|r| *r == "origin").or(remotes.first()).cloned();
        }
        // A new repository has no HEAD commit to list from.
        if all || self.head_commit().is_some() {
            let max = format!("--max-count={limit}");
            let tips = if all { "--branches" } else { "HEAD" };
            let out = git(&self.worktree, &["rev-list", tips, "--not", "--remotes", &max])?;
            t.unpushed = out.lines().map(str::trim).filter(|l| !l.is_empty()).map(Into::into).collect();
        }
        Ok(t)
    }

    /// Pushes the checked-out branch. A branch without an upstream is pushed
    /// to the remote [`Repo::tracking`] names, and gets it as its upstream.
    /// Returns what happened, in a sentence.
    pub fn push(&self) -> Result<String, String> {
        let Some(Head::Branch(branch)) = self.head() else {
            return Err("Not on a branch: check one out to push it.".into());
        };
        let t = self.tracking(false, 1)?;
        let Some(remote) = t.remote else { return Err("This repository has no remote to push to.".into()) };
        match &t.upstream {
            Some(upstream) => {
                git_remote(&self.worktree, &["push"])?;
                Ok(format!("Pushed {branch} to {upstream}."))
            }
            None => {
                git_remote(&self.worktree, &["push", "--set-upstream", &remote, &branch])?;
                Ok(format!("Pushed {branch} to {remote}, which it now tracks."))
            }
        }
    }

    /// Pulls the checked-out branch's upstream, rebasing the branch's own
    /// commits on top of it, or merging it when `rebase` is off. Returns what
    /// happened, in a sentence.
    pub fn pull(&self, rebase: bool) -> Result<String, String> {
        let Some(Head::Branch(branch)) = self.head() else {
            return Err("Not on a branch: check one out to pull into it.".into());
        };
        let t = self.tracking(false, 1)?;
        if t.remote.is_none() {
            return Err("This repository has no remote to pull from.".into());
        }
        let Some(upstream) = t.upstream else {
            return Err(format!("{branch} does not track a remote branch yet: push it first."));
        };
        // Whether anything came in shows in HEAD; git's own words vary with
        // the language and with rebasing or merging.
        let before = self.head_commit();
        git_remote(&self.worktree, &["pull", if rebase { "--rebase" } else { "--no-rebase" }])?;
        Ok(if self.head_commit() == before {
            format!("{branch} is up to date with {upstream}.")
        } else if rebase {
            format!("Pulled {upstream} and rebased {branch} on it.")
        } else {
            format!("Pulled {upstream} into {branch}.")
        })
    }

    /// The latest `limit` commits of this working tree's HEAD, or of every
    /// branch and tag when `all`, newest first.
    pub fn log(&self, all: bool, limit: usize) -> Result<Vec<Commit>, String> {
        let max = format!("--max-count={limit}");
        // Unit and record separators keep subjects and names unambiguous.
        let format = "--format=%H%x1f%h%x1f%an%x1f%at%x1f%D%x1f%s%x1e";
        let mut args = vec!["-c", "log.showSignature=false", "log", "--no-color", &max, format];
        if all {
            args.push("--all");
        }
        match git(&self.worktree, &args) {
            Ok(out) => Ok(parse_log(&out)),
            // A new repository: git complains that there are no commits yet.
            Err(_) if self.head_commit().is_none() && !all => Ok(Vec::new()),
            Err(e) => Err(e),
        }
    }

    /// The commit HEAD points at, read from the files under `.git`.
    fn head_commit(&self) -> Option<String> {
        let head = std::fs::read_to_string(self.git_dir.join("HEAD")).ok()?;
        let Some(target) = head.trim().strip_prefix("ref:").map(str::trim) else {
            return Some(head.trim().to_owned());
        };
        if let Ok(id) = std::fs::read_to_string(self.common_dir.join(target)) {
            return Some(id.trim().to_owned());
        }
        let packed = std::fs::read_to_string(self.common_dir.join("packed-refs")).ok()?;
        packed.lines().find_map(|l| l.strip_suffix(target)?.strip_suffix(' ').map(str::to_owned))
    }

    /// A fingerprint of where HEAD and every branch and tag point. It changes
    /// when a commit, checkout, reset, rebase or fetch moves a ref. It only
    /// reads files, so it can be checked often to decide when to run `git log`.
    pub fn ref_stamp(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        let read = |p: &Path| std::fs::read(p).ok();
        read(&self.git_dir.join("HEAD")).hash(&mut h);
        for file in ["packed-refs", "reftable/tables.list"] {
            read(&self.common_dir.join(file)).hash(&mut h);
        }
        let mut dirs = vec![self.common_dir.join("refs")];
        let mut loose = Vec::new();
        while let Some(dir) = dirs.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else { continue };
            for e in entries.flatten() {
                match e.file_type() {
                    Ok(t) if t.is_dir() => dirs.push(e.path()),
                    Ok(_) => loose.push(e.path()),
                    Err(_) => {}
                }
            }
        }
        loose.sort();
        for file in loose {
            file.hash(&mut h);
            read(&file).hash(&mut h);
        }
        h.finish()
    }
}

/// How a working tree's commits stand against its remotes, from what the
/// last fetch (or pull, or push) saw of them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tracking {
    /// Where a push goes: the upstream's remote, else `origin`, else the
    /// first remote. `None` when the repository has no remote.
    pub remote: Option<String>,
    /// The branch the checked-out branch tracks, such as `origin/main`.
    pub upstream: Option<String>,
    /// Commits on the branch but not on its upstream.
    pub ahead: usize,
    /// Commits on the upstream but not on the branch.
    pub behind: usize,
    /// Commits that are on no remote branch at all.
    pub unpushed: HashSet<String>,
}

/// One commit, as `git log` lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Commit {
    pub id: String,
    /// The abbreviated id.
    pub short: String,
    pub author: String,
    /// When it was authored, in seconds since the epoch.
    pub time: i64,
    pub subject: String,
    /// Branches and tags pointing at it (tags as `tag: v1.0`).
    pub refs: Vec<String>,
    /// The working tree's HEAD points at it.
    pub head: bool,
}

/// Parses `git log` output in the format [`Repo::log`] asks for.
fn parse_log(out: &str) -> Vec<Commit> {
    out.split('\u{1e}')
        .filter_map(|record| {
            let mut f = record.trim_start_matches(['\r', '\n']).split('\u{1f}');
            let id = f.next().filter(|id| !id.is_empty())?.to_owned();
            let short = f.next()?.to_owned();
            let author = f.next()?.to_owned();
            let time = f.next()?.parse().unwrap_or(0);
            let decorations = f.next()?;
            let subject = f.next()?.trim_end().to_owned();
            let mut head = false;
            let mut refs = Vec::new();
            for d in decorations.split(", ").filter(|d| !d.is_empty()) {
                if let Some(branch) = d.strip_prefix("HEAD -> ") {
                    head = true;
                    refs.push(branch.to_owned());
                } else if d == "HEAD" {
                    head = true;
                } else if !d.ends_with("/HEAD") {
                    // `origin/HEAD` only repeats the remote's default branch.
                    refs.push(d.to_owned());
                }
            }
            Some(Commit { id, short, author, time, subject, refs, head })
        })
        .collect()
}

/// Branches of a repository and the working trees that have them checked out.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Branches {
    pub local: Vec<String>,
    /// Remote-tracking branches, as `<remote>/<branch>`.
    pub remote: Vec<String>,
    /// Every working tree, the main one first.
    pub worktrees: Vec<Worktree>,
}

/// A working tree of a repository, as `git worktree list` reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Worktree {
    pub path: PathBuf,
    /// What it has checked out; `None` for a bare repository.
    pub head: Option<Head>,
    /// Its folder is gone; `git worktree prune` would forget it.
    pub prunable: bool,
}

impl Worktree {
    /// Whether a session can work in it: it has files, and they exist.
    pub fn usable(&self) -> bool {
        self.head.is_some() && !self.prunable
    }
}

/// How a worktree for a branch name would be made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Plan {
    /// No such branch yet: create it.
    NewBranch,
    /// Check out the existing local branch.
    Existing,
    /// Create the branch tracking this remote-tracking branch (`origin/x`).
    Track(String),
    /// The branch is checked out in this working tree already (git allows a
    /// branch in one working tree only).
    CheckedOut(PathBuf),
}

impl Branches {
    /// Parses `git for-each-ref --format=%(refname)` and `git worktree list --porcelain`.
    fn parse(refs: &str, worktrees: &str) -> Branches {
        let mut out = Branches::default();
        for line in refs.lines().map(str::trim) {
            if let Some(b) = line.strip_prefix("refs/heads/") {
                out.local.push(b.to_owned());
            } else if let Some(r) = line.strip_prefix("refs/remotes/")
                && !r.ends_with("/HEAD")
            {
                out.remote.push(r.to_owned());
            }
        }
        for block in worktrees.split("\n\n") {
            let mut path = None;
            let mut commit = None;
            let mut branch = None;
            let mut prunable = false;
            for line in block.lines() {
                if let Some(p) = line.strip_prefix("worktree ") {
                    // Git for Windows lists `D:/src/app`; use native separators.
                    #[cfg(windows)]
                    let p = p.replace('/', "\\");
                    path = Some(PathBuf::from(p));
                } else if let Some(c) = line.strip_prefix("HEAD ") {
                    commit = Some(c.get(..7).unwrap_or(c).to_owned());
                } else if let Some(b) = line.strip_prefix("branch ") {
                    branch = Some(b.strip_prefix("refs/heads/").unwrap_or(b).to_owned());
                } else if line == "prunable" || line.starts_with("prunable ") {
                    prunable = true;
                }
            }
            if let Some(path) = path {
                let head = branch.map(Head::Branch).or(commit.map(Head::Detached));
                out.worktrees.push(Worktree { path, head, prunable });
            }
        }
        out
    }

    /// What making a worktree for `branch` would do.
    pub fn plan(&self, branch: &str) -> Plan {
        // Also a worktree whose folder is gone: git refuses until it is pruned.
        if let Some(w) = self.worktrees.iter().find(|w| matches!(&w.head, Some(Head::Branch(b)) if b == branch)) {
            return Plan::CheckedOut(w.path.clone());
        }
        if self.local.iter().any(|b| b == branch) {
            return Plan::Existing;
        }
        // Like `git checkout <branch>`: track a remote's branch of that name,
        // preferring origin when several remotes have one.
        let suffix = format!("/{branch}");
        let remotes: Vec<&String> = self
            .remote
            .iter()
            .filter(|r| r.strip_suffix(&suffix).is_some_and(|remote| !remote.is_empty() && !remote.contains('/')))
            .collect();
        let pick = match remotes.as_slice() {
            [one] => Some(*one),
            many => many.iter().copied().find(|r| r.starts_with("origin/")),
        };
        match pick {
            Some(r) => Plan::Track(r.clone()),
            None => Plan::NewBranch,
        }
    }
}

/// Whether `path` is `dir` or inside it, compared like [`same_path`].
pub fn is_within(path: &Path, dir: &Path) -> bool {
    let (path, dir) = (path_key(path), path_key(dir));
    path == dir || path.strip_prefix(&dir).is_some_and(|rest| rest.starts_with('/'))
}

/// A path as a string that compares equal for the same directory.
fn path_key(p: &Path) -> String {
    let s = normalize(p).to_string_lossy().replace('\\', "/");
    let s = s.trim_end_matches('/');
    if cfg!(windows) { s.to_lowercase() } else { s.to_owned() }
}

/// Whether two paths are the same without asking the disk: git writes
/// `D:/src/app`, Windows `d:\src\app\`.
pub fn same_path(a: &Path, b: &Path) -> bool {
    path_key(a) == path_key(b)
}

/// A branch name made safe for a folder name: `feature/login` -> `feature-login`.
pub fn slug(branch: &str) -> String {
    let mut out = String::new();
    for c in branch.trim().chars() {
        let keep = c.is_alphanumeric() || matches!(c, '-' | '_' | '.');
        if keep {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let out = out.trim_matches(['-', '.']);
    if out.is_empty() { "worktree".to_owned() } else { out.to_owned() }
}

/// Resolves `.` and `..` without touching the disk (unlike `canonicalize`,
/// which yields `\\?\` paths on Windows).
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir if matches!(out.components().next_back(), Some(Component::Normal(_))) => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// The git program: `git` on PATH or, on Windows, the one in TerminalRooster's own bash.
fn git_program() -> PathBuf {
    #[cfg(windows)]
    if !crate::session::shell::find_in_path("git.exe")
        && let Some(exe) = crate::session::bundled_bash::install_dir().map(|d| d.join("cmd").join("git.exe"))
        && exe.is_file()
    {
        return exe;
    }
    PathBuf::from("git")
}

/// How long a command that talks to a remote may run. It can wait on the
/// network, or on a login prompt nobody sees: git runs without a console.
const REMOTE_TIMEOUT: Duration = Duration::from_secs(300);

/// git in `dir`, without a console, never asking anything on a terminal.
fn command(dir: &Path, args: &[&str]) -> Command {
    let mut cmd = Command::new(git_program());
    cmd.arg("-C")
        .arg(dir)
        .args(args)
        .stdin(Stdio::null())
        .env("GIT_TERMINAL_PROMPT", "0")
        // A merging pull takes git's message instead of opening an editor.
        .env("GIT_MERGE_AUTOEDIT", "no");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(crate::session::shell::CREATE_NO_WINDOW);
    }
    tracing::debug!(dir = %dir.display(), ?args, "running git");
    cmd
}

fn spawn_error(e: std::io::Error) -> String {
    if e.kind() == std::io::ErrorKind::NotFound {
        "git is not installed (or not on PATH)".into()
    } else {
        format!("could not run git: {e}")
    }
}

/// git's output when it succeeded, else what it said went wrong.
fn outcome(args: &[&str], status: std::process::ExitStatus, stdout: &[u8], stderr: &[u8]) -> Result<String, String> {
    if status.success() {
        return Ok(String::from_utf8_lossy(stdout).into_owned());
    }
    let message = error_message(&String::from_utf8_lossy(stderr));
    Err(if message.is_empty() { format!("git {} failed ({status})", args.join(" ")) } else { message })
}

/// Runs git in `dir`; returns its output, or what it said went wrong.
fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = command(dir, args).output().map_err(spawn_error)?;
    outcome(args, out.status, &out.stdout, &out.stderr)
}

/// Like [`git`], for commands that talk to a remote: stopped after
/// [`REMOTE_TIMEOUT`].
fn git_remote(dir: &Path, args: &[&str]) -> Result<String, String> {
    let mut child = command(dir, args).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(spawn_error)?;
    // Both pipes are read on threads, so a full pipe can't stall git.
    fn drain(pipe: Option<impl Read + Send + 'static>) -> std::thread::JoinHandle<Vec<u8>> {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut pipe) = pipe {
                let _ = pipe.read_to_end(&mut buf);
            }
            buf
        })
    }
    let (stdout, stderr) = (drain(child.stdout.take()), drain(child.stderr.take()));
    let deadline = Instant::now() + REMOTE_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(100)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                // The readers are left alone: a helper git started (ssh, a
                // credential prompt) may still hold the pipes open.
                return Err(format!(
                    "git {} took longer than {} minutes and was stopped. It may be waiting for a login; try it \
                     in a terminal.",
                    args[0],
                    REMOTE_TIMEOUT.as_secs() / 60
                ));
            }
            Err(e) => return Err(format!("could not run git: {e}")),
        }
    };
    let (stdout, stderr) = (stdout.join().unwrap_or_default(), stderr.join().unwrap_or_default());
    outcome(args, status, &stdout, &stderr)
}

/// What went wrong, from git's error output: its last lines ("error: …",
/// "fatal: …"; progress comes before), without the "hint:" advice git
/// appends, which would push the error itself out of view.
fn error_message(stderr: &str) -> String {
    let lines: Vec<&str> = stderr.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with("hint:")).collect();
    lines[lines.len().saturating_sub(3)..].join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn finds_the_branch_of_a_main_worktree_from_a_subdirectory() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("app");
        write(&root.join(".git/HEAD"), "ref: refs/heads/feature/login\n");
        std::fs::create_dir_all(root.join("src/deep")).unwrap();

        let status = Status::of(&root.join("src/deep")).unwrap();
        assert_eq!(status.repo.worktree, root);
        assert_eq!(status.head, Some(Head::Branch("feature/login".into())));
        assert!(!status.repo.is_linked());
        assert_eq!(status.repo.main_worktree(), Some(root.clone()));
        assert_eq!(Status::of(dir.path()), None);
    }

    #[test]
    fn follows_a_linked_worktree_to_its_repository() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("app");
        let linked = dir.path().join("app-fix");
        write(&main.join(".git/HEAD"), "ref: refs/heads/main\n");
        let admin = main.join(".git/worktrees/app-fix");
        write(&admin.join("HEAD"), "0123456789abcdef0123456789abcdef01234567\n");
        write(&admin.join("commondir"), "../..\n");
        // Git for Windows writes forward slashes.
        write(&linked.join(".git"), &format!("gitdir: {}\n", admin.display().to_string().replace('\\', "/")));

        let status = Status::of(&linked).unwrap();
        assert_eq!(status.head, Some(Head::Detached("0123456".into())));
        assert!(status.repo.is_linked());
        assert_eq!(status.repo.common_dir, main.join(".git"));
        assert_eq!(status.repo.main_worktree(), Some(main.clone()));
        // Named after the main working tree, beside the linked one.
        assert_eq!(status.repo.suggest_worktree_path("feature/x"), Some(dir.path().join("app-feature-x")));
    }

    #[test]
    fn relative_gitdir_is_resolved_against_the_dot_git_file() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join(".git/modules/lib/HEAD"), "ref: refs/heads/dev\n");
        write(&dir.path().join("lib/.git"), "gitdir: ../.git/modules/lib\n");
        let repo = Repo::discover(&dir.path().join("lib")).unwrap();
        assert_eq!(repo.git_dir, dir.path().join(".git").join("modules").join("lib"));
        assert_eq!(repo.head(), Some(Head::Branch("dev".into())));
    }

    #[test]
    fn suggested_paths_are_free_and_side_by_side() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("app");
        write(&root.join(".git/HEAD"), "ref: refs/heads/main\n");
        let repo = Repo::discover(&root).unwrap();
        assert_eq!(repo.suggest_worktree_path("fix"), Some(dir.path().join("app-fix")));
        std::fs::create_dir(dir.path().join("app-fix")).unwrap();
        assert_eq!(repo.suggest_worktree_path("fix"), Some(dir.path().join("app-fix-2")));
    }

    #[test]
    fn sessions_keep_their_subdirectory() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("app");
        write(&root.join(".git/HEAD"), "ref: refs/heads/main\n");
        std::fs::create_dir_all(root.join("crates/core")).unwrap();
        let other = dir.path().join("app-fix");
        std::fs::create_dir_all(other.join("crates/core")).unwrap();
        let repo = Repo::discover(&root).unwrap();
        assert_eq!(repo.same_place_in(&root.join("crates/core"), &other), other.join("crates/core"));
        assert_eq!(repo.same_place_in(&root, &other), other);
        std::fs::create_dir_all(root.join("only-here")).unwrap();
        assert_eq!(repo.same_place_in(&root.join("only-here"), &other), other);
    }

    #[test]
    fn same_paths() {
        assert!(same_path(Path::new("/src/app/"), Path::new("/src/./app")));
        assert!(!same_path(Path::new("/src/app"), Path::new("/src/app-x")));
        if cfg!(windows) {
            assert!(same_path(Path::new("D:/src/App"), Path::new(r"d:\src\app\")));
        }
    }

    #[test]
    fn within() {
        let dir = Path::new("/src/app-x");
        assert!(is_within(Path::new("/src/app-x"), dir));
        assert!(is_within(Path::new("/src/app-x/crates/core/"), dir));
        assert!(!is_within(Path::new("/src/app-xy"), dir), "a sibling with a longer name");
        assert!(!is_within(Path::new("/src"), dir));
        if cfg!(windows) {
            assert!(is_within(Path::new(r"d:\Src\APP-X\lib"), Path::new("D:/src/app-x")));
        }
    }

    #[test]
    fn error_messages_leave_out_hints() {
        let rejected = "To ../remote.git\n ! [rejected]        main -> main (fetch first)\n\
                        error: failed to push some refs to '../remote.git'\n\
                        hint: Updates were rejected because the remote contains work that you do not\n\
                        hint: have locally.\n";
        assert_eq!(
            error_message(rejected),
            "To ../remote.git\n! [rejected]        main -> main (fetch first)\nerror: failed to push some refs to \
             '../remote.git'"
        );
        assert_eq!(
            error_message("Counting objects: 1\nfatal: not a git repository\n"),
            "Counting objects: 1\nfatal: not a git repository"
        );
        assert_eq!(error_message("hint: only advice\n"), "");
    }

    #[test]
    fn slugs() {
        assert_eq!(slug("feature/login"), "feature-login");
        assert_eq!(slug("  fix: the  thing "), "fix-the-thing");
        assert_eq!(slug("v1.2"), "v1.2");
        assert_eq!(slug("///"), "worktree");
    }

    #[test]
    fn plans() {
        let refs = "refs/heads/main\nrefs/heads/wip\nrefs/remotes/origin/HEAD\nrefs/remotes/origin/main\n\
                    refs/remotes/origin/shared\nrefs/remotes/fork/shared\nrefs/remotes/fork/only-fork\n";
        let list = "worktree D:/src/app\nHEAD abcdef0123\nbranch refs/heads/main\n\n\
                    worktree D:/src/app-old\nHEAD 0123456789\ndetached\n\n\
                    worktree D:/src/app-gone\nHEAD 0123456789\nbranch refs/heads/gone\nprunable gitdir file points to \
                    non-existent location\n\n";
        let b = Branches::parse(refs, list);
        assert_eq!(b.local, vec!["main", "wip"]);
        let main = Worktree { path: "D:/src/app".into(), head: Some(Head::Branch("main".into())), prunable: false };
        let old =
            Worktree { path: "D:/src/app-old".into(), head: Some(Head::Detached("0123456".into())), prunable: false };
        let gone = Worktree { path: "D:/src/app-gone".into(), head: Some(Head::Branch("gone".into())), prunable: true };
        assert_eq!(b.worktrees, vec![main, old, gone.clone()]);
        assert!(!gone.usable());
        assert_eq!(b.plan("main"), Plan::CheckedOut("D:/src/app".into()));
        assert_eq!(b.plan("gone"), Plan::CheckedOut("D:/src/app-gone".into()));
        assert_eq!(b.plan("wip"), Plan::Existing);
        assert_eq!(b.plan("shared"), Plan::Track("origin/shared".into()));
        assert_eq!(b.plan("only-fork"), Plan::Track("fork/only-fork".into()));
        assert_eq!(b.plan("new"), Plan::NewBranch);
        // "fork/only-fork" names a remote branch; as a local name it is new.
        assert_eq!(b.plan("fork/only-fork"), Plan::NewBranch);
    }

    /// The real thing, when git is installed.
    #[test]
    fn adds_worktrees_with_git() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("app");
        std::fs::create_dir(&root).unwrap();
        let run = |args: &[&str]| git(&root, args);
        if run(&["init", "-q", "-b", "main"]).is_err() {
            eprintln!("git not available; skipping");
            return;
        }
        let who = ["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false"];
        run(&[&who[..], &["commit", "-q", "--allow-empty", "-m", "first"]].concat()).unwrap();
        let repo = Repo::discover(&root).unwrap();

        let path = repo.suggest_worktree_path("feature/one").unwrap();
        let branches = repo.branches().unwrap();
        assert_eq!(branches.plan("feature/one"), Plan::NewBranch);
        repo.add_worktree(&path, "feature/one", &Plan::NewBranch, None).unwrap();
        let status = Status::of(&path).unwrap();
        assert_eq!(status.head, Some(Head::Branch("feature/one".into())));
        assert!(status.repo.is_linked());
        assert_eq!(status.repo.main_worktree(), Some(root.clone()));

        // Now checked out there; a second worktree for it is refused by git.
        let branches = repo.branches().unwrap();
        assert!(matches!(branches.plan("feature/one"), Plan::CheckedOut(_)));
        let again = dir.path().join("again");
        let err = repo.add_worktree(&again, "feature/one", &Plan::Existing, None).unwrap_err();
        assert!(!err.is_empty());

        // An existing branch that isn't checked out anywhere.
        run(&["branch", "two"]).unwrap();
        let two = repo.suggest_worktree_path("two").unwrap();
        assert_eq!(repo.branches().unwrap().plan("two"), Plan::Existing);
        repo.add_worktree(&two, "two", &Plan::Existing, None).unwrap();
        assert_eq!(Status::of(&two).unwrap().head, Some(Head::Branch("two".into())));
    }

    #[test]
    fn parses_the_log() {
        let out = "aaaa1111\u{1f}aaaa1\u{1f}Ann\u{1f}1700000000\u{1f}HEAD -> main, origin/main, origin/HEAD, tag: v1\
                   \u{1f}Add: a thing\u{1e}\n\
                   bbbb2222\u{1f}bbbb2\u{1f}Bo B\u{1f}1690000000\u{1f}\u{1f}Fix: x, y\u{1e}\n";
        let log = parse_log(out);
        assert_eq!(log.len(), 2);
        assert_eq!(log[0].refs, vec!["main", "origin/main", "tag: v1"]);
        assert!(log[0].head);
        assert_eq!((log[0].short.as_str(), log[0].author.as_str(), log[0].time), ("aaaa1", "Ann", 1_700_000_000));
        assert_eq!(log[1].subject, "Fix: x, y");
        assert!(log[1].refs.is_empty() && !log[1].head);
        assert!(parse_log("").is_empty());
    }

    /// The log and its stamp follow new commits, with git installed.
    #[test]
    fn log_follows_commits_with_git() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("app");
        std::fs::create_dir(&root).unwrap();
        let run = |args: &[&str]| git(&root, args);
        if run(&["init", "-q", "-b", "main"]).is_err() {
            eprintln!("git not available; skipping");
            return;
        }
        let who = ["-c", "user.name=Tess", "-c", "user.email=t@t", "-c", "commit.gpgsign=false"];
        let commit = |msg: &str| run(&[&who[..], &["commit", "-q", "--allow-empty", "-m", msg]].concat()).unwrap();
        let repo = Repo::discover(&root).unwrap();
        assert_eq!(repo.log(false, 50).unwrap(), Vec::new(), "no commits yet");

        commit("first");
        let stamp = repo.ref_stamp();
        assert_eq!(repo.ref_stamp(), stamp, "nothing moved");
        let log = repo.log(false, 50).unwrap();
        assert_eq!(log.len(), 1);
        assert_eq!((log[0].subject.as_str(), log[0].author.as_str()), ("first", "Tess"));
        assert_eq!(log[0].refs, vec!["main"]);
        assert!(log[0].head);

        commit("second");
        assert_ne!(repo.ref_stamp(), stamp);
        assert_eq!(repo.log(false, 50).unwrap()[0].subject, "second");
        assert_eq!(repo.log(false, 1).unwrap().len(), 1);

        // A commit on another branch shows only in the log of every branch.
        run(&["switch", "-q", "-c", "side"]).unwrap();
        commit("on side");
        run(&["switch", "-q", "main"]).unwrap();
        let here: Vec<String> = repo.log(false, 50).unwrap().into_iter().map(|c| c.subject).collect();
        assert_eq!(here, vec!["second", "first"]);
        // Commits made within one second come in no particular order.
        let all = repo.log(true, 50).unwrap();
        let find = |subject: &str| all.iter().find(|c| c.subject == subject).unwrap();
        assert_eq!(all.len(), 3);
        assert_eq!(find("on side").refs, vec!["side"]);
        assert!(!find("on side").head && find("second").head);

        // Packed refs still resolve.
        run(&["pack-refs", "--all"]).unwrap();
        assert_eq!(repo.head_commit(), Some(find("second").id.clone()));
    }

    /// Removing a worktree keeps its changes unless forced, with git installed.
    #[test]
    fn removes_worktrees_with_git() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("app");
        std::fs::create_dir(&root).unwrap();
        let run = |args: &[&str]| git(&root, args);
        if run(&["init", "-q", "-b", "main"]).is_err() {
            eprintln!("git not available; skipping");
            return;
        }
        let who = ["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false"];
        run(&[&who[..], &["commit", "-q", "--allow-empty", "-m", "first"]].concat()).unwrap();
        let repo = Repo::discover(&root).unwrap();
        let path = repo.suggest_worktree_path("fix").unwrap();
        repo.add_worktree(&path, "fix", &Plan::NewBranch, None).unwrap();
        assert_eq!(Repo::changes(&path), Ok(0));

        // Removing from the worktree's own Repo works too: git runs in the main one.
        let inside = Repo::discover(&path).unwrap();
        std::fs::write(path.join("notes.txt"), "draft").unwrap();
        assert_eq!(Repo::changes(&path), Ok(1));
        assert!(inside.remove_worktree(&path, false).is_err(), "a new file keeps it");
        assert!(path.join("notes.txt").is_file());

        inside.remove_worktree(&path, true).unwrap();
        assert!(!path.exists());
        let branches = repo.branches().unwrap();
        assert_eq!(branches.worktrees.len(), 1);
        assert!(branches.local.contains(&"fix".to_owned()), "the branch is kept");
    }

    /// Push, pull and tracking against a bare repository standing in for a
    /// remote, with git installed.
    #[test]
    fn pushes_and_pulls_with_git() {
        let dir = tempfile::tempdir().unwrap();
        let remote = dir.path().join("remote.git");
        if git(dir.path(), &["init", "-q", "--bare", "-b", "main", &remote.to_string_lossy()]).is_err() {
            eprintln!("git not available; skipping");
            return;
        }
        // A clone with its own identity, so commits and rebases work anywhere.
        let clone = |name: &str| {
            let path = dir.path().join(name);
            git(dir.path(), &["clone", "-q", &remote.to_string_lossy(), &path.to_string_lossy()]).unwrap();
            for (key, value) in [("user.name", name), ("user.email", "t@t"), ("commit.gpgsign", "false")] {
                git(&path, &["config", key, value]).unwrap();
            }
            git(&path, &["switch", "-q", "-C", "main"]).unwrap();
            Repo::discover(&path).unwrap()
        };
        let commit = |repo: &Repo, msg: &str| {
            git(&repo.worktree, &["commit", "-q", "--allow-empty", "-m", msg]).unwrap();
            repo.head_commit().unwrap()
        };
        let subjects =
            |repo: &Repo| -> Vec<String> { repo.log(false, 50).unwrap().into_iter().map(|c| c.subject).collect() };

        let a = clone("a");
        let one = commit(&a, "one");
        let t = a.tracking(false, 50).unwrap();
        assert_eq!((t.remote.as_deref(), t.upstream.as_deref()), (Some("origin"), None));
        assert_eq!(t.unpushed, HashSet::from([one.clone()]));
        assert!(a.pull(true).unwrap_err().contains("push it first"));

        // The first push sets the upstream.
        assert_eq!(a.push().unwrap(), "Pushed main to origin, which it now tracks.");
        let t = a.tracking(false, 50).unwrap();
        assert_eq!((t.upstream.as_deref(), t.ahead, t.behind), (Some("origin/main"), 0, 0));
        assert!(t.unpushed.is_empty());

        // Someone else pushes; a's own commit is then rejected until it pulls.
        let b = clone("b");
        commit(&b, "two");
        assert_eq!(b.push().unwrap(), "Pushed main to origin/main.");
        let three = commit(&a, "three");
        let t = a.tracking(false, 50).unwrap();
        assert_eq!((t.ahead, t.behind), (1, 0), "a has not fetched yet");
        assert_eq!(t.unpushed, HashSet::from([three]));
        let err = a.push().unwrap_err();
        assert!(err.contains("rejected") && !err.contains("hint:"), "{err}");

        // Rebasing keeps the history a line: three now sits on two.
        assert_eq!(a.pull(true).unwrap(), "Pulled origin/main and rebased main on it.");
        assert_eq!(subjects(&a), vec!["three", "two", "one"]);
        assert_eq!(a.pull(true).unwrap(), "main is up to date with origin/main.");
        a.push().unwrap();

        // Without rebasing, a pull merges.
        b.pull(true).unwrap();
        commit(&b, "four");
        b.push().unwrap();
        commit(&a, "five");
        assert_eq!(a.pull(false).unwrap(), "Pulled origin/main into main.");
        let parents = git(&a.worktree, &["rev-list", "--parents", "-n", "1", "HEAD"]).unwrap();
        assert_eq!(parents.split_whitespace().count(), 3, "a merge commit has two parents");
        let t = a.tracking(false, 50).unwrap();
        assert_eq!((t.ahead, t.behind, t.unpushed.len()), (2, 0, 2));

        // No remote, or no branch: nothing to push.
        let lone = dir.path().join("lone");
        git(dir.path(), &["init", "-q", "-b", "main", &lone.to_string_lossy()]).unwrap();
        let lone = Repo::discover(&lone).unwrap();
        assert_eq!(lone.tracking(false, 50).unwrap(), Tracking::default());
        assert!(lone.push().unwrap_err().contains("no remote"));
        git(&a.worktree, &["switch", "-q", "--detach"]).unwrap();
        assert!(a.push().unwrap_err().contains("Not on a branch"));
    }
}
