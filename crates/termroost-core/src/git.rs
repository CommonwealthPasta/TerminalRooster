//! Git awareness for sessions: the branch a directory is on, and working
//! trees made beside a repository.
//!
//! [`Repo::discover`] and [`Repo::head`] only read files under `.git`, so a
//! front end can poll them for every visible session. Everything else runs
//! the `git` program.

use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};

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

/// Whether two paths are the same without asking the disk: git writes
/// `D:/src/app`, Windows `d:\src\app\`.
pub fn same_path(a: &Path, b: &Path) -> bool {
    let key = |p: &Path| {
        let s = normalize(p).to_string_lossy().replace('\\', "/");
        let s = s.trim_end_matches('/');
        if cfg!(windows) { s.to_lowercase() } else { s.to_owned() }
    };
    key(a) == key(b)
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

/// Runs git in `dir`; returns its output, or what it said went wrong.
fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let mut cmd = Command::new(git_program());
    cmd.arg("-C").arg(dir).args(args).stdin(Stdio::null()).env("GIT_TERMINAL_PROMPT", "0");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(crate::session::shell::CREATE_NO_WINDOW);
    }
    tracing::debug!(dir = %dir.display(), ?args, "running git");
    let out = match cmd.output() {
        Ok(out) => out,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err("git is not installed (or not on PATH)".into());
        }
        Err(e) => return Err(format!("could not run git: {e}")),
    };
    if out.status.success() {
        return Ok(String::from_utf8_lossy(&out.stdout).into_owned());
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    // The last lines say what failed ("fatal: …"); progress comes before.
    let lines: Vec<&str> = stderr.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    let message = lines[lines.len().saturating_sub(3)..].join("\n");
    Err(if message.is_empty() { format!("git {} failed ({})", args.join(" "), out.status) } else { message })
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
}
