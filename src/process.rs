//! Reading process state out of /proc.
//!
//! The window manager knows a window's pid and nothing else about how it was
//! started. `/proc/<pid>/cmdline` is the only faithful record of that, which
//! is why relaunch commands are derived from it rather than guessed from the
//! window class.

use std::collections::BTreeSet;
use std::path::PathBuf;

pub trait ProcessInfoProvider {
    /// Full argv of a process, or `None` when it is gone or unreadable.
    fn cmdline(&self, pid: i32) -> Option<Vec<String>>;
    fn cwd(&self, pid: i32) -> Option<PathBuf>;
    /// The process name as the kernel reports it (`/proc/<pid>/comm`).
    fn comm(&self, pid: i32) -> Option<String>;
    fn children(&self, pid: i32) -> Vec<i32>;
    /// Regular files the process currently has open, resolved through
    /// `/proc/<pid>/fd`. Some programs record their session identity in one
    /// of these paths.
    fn open_files(&self, pid: i32) -> Vec<String>;
    /// The program file the process is running (`/proc/<pid>/exe`). Recorded
    /// for diagnosis; never used to decide anything.
    fn exe(&self, pid: i32) -> Option<PathBuf>;

    /// The program a process is running, as `[tui] programs` names it: the
    /// kernel's name for it, or — for a script an interpreter runs, which the
    /// kernel names `node` or `python3` — the script's own name.
    fn program(&self, pid: i32) -> Option<String> {
        let comm = self.comm(pid)?;
        if !is_interpreter(&comm) {
            return Some(comm);
        }
        Some(
            self.cmdline(pid)
                .and_then(|argv| script_name(&argv).map(|(name, _)| name))
                .unwrap_or(comm),
        )
    }

    /// Breadth-first search of the process tree under `pid` for a process
    /// whose program (see [`program`](Self::program)) is in `names`.
    ///
    /// Depth is bounded because a terminal running a shell running a
    /// multiplexer can nest arbitrarily, and the interesting programs live
    /// within a few levels.
    fn find_descendant(&self, pid: i32, names: &BTreeSet<String>, max_depth: usize) -> Option<i32> {
        let mut queue: Vec<(i32, usize)> = self.children(pid).into_iter().map(|c| (c, 1)).collect();
        let mut head = 0;
        while head < queue.len() {
            let (current, depth) = queue[head];
            head += 1;
            if let Some(name) = self.program(current) {
                if names.contains(&name) {
                    return Some(current);
                }
            }
            if depth < max_depth {
                queue.extend(self.children(current).into_iter().map(|c| (c, depth + 1)));
            }
        }
        None
    }
}

/// Interpreters whose process name says nothing about what they run.
///
/// Matched on the name's first `-`-separated part, versions trimmed: Node 26
/// names its main thread, and so the process, `node-MainThread`, and Python
/// appears as `python3.13`.
fn is_interpreter(comm: &str) -> bool {
    let first = comm.split('-').next().unwrap_or(comm);
    let base = first.trim_end_matches(|c: char| c.is_ascii_digit() || c == '.');
    matches!(
        base,
        "node" | "nodejs" | "bun" | "python" | "pypy" | "ruby" | "perl"
    )
}

/// The name of the script an interpreter's argv runs, and where the
/// script's own arguments start.
///
/// `node /…/bin/gemini --resume` → `gemini`; `node /…/bundle/gemini.js` →
/// `gemini`; `python3 -m aider` → `aider`. Code passed inline (`-c`, `-e`)
/// has no name. Flags that take a value are stepped over with it.
pub fn script_name(argv: &[String]) -> Option<(String, usize)> {
    const TAKES_VALUE: &[&str] = &["-r", "--require", "--import", "--loader", "-X", "-W"];
    const INLINE_CODE: &[&str] = &["-c", "-e", "--eval", "-p", "--print"];
    const EXTENSIONS: &[&str] = &[".js", ".mjs", ".cjs", ".ts", ".py", ".rb", ".pl"];
    let mut i = 1;
    while let Some(arg) = argv.get(i) {
        if INLINE_CODE.contains(&arg.as_str()) {
            return None;
        }
        if arg == "-m" {
            let module = argv.get(i + 1)?;
            let name = module.split('.').next()?.to_string();
            return (!name.is_empty()).then_some((name, i + 2));
        }
        if TAKES_VALUE.contains(&arg.as_str()) {
            i += 2;
            continue;
        }
        if arg.starts_with('-') {
            i += 1;
            continue;
        }
        let file = std::path::Path::new(arg).file_name()?.to_str()?;
        let stem = EXTENSIONS
            .iter()
            .find_map(|ext| file.strip_suffix(ext))
            .unwrap_or(file);
        return (!stem.is_empty()).then(|| (stem.to_string(), i + 1));
    }
    None
}

pub struct RealProcessInfo;

impl ProcessInfoProvider for RealProcessInfo {
    fn cmdline(&self, pid: i32) -> Option<Vec<String>> {
        let raw = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
        let argv: Vec<String> = raw
            .split(|&b| b == 0)
            .filter(|s| !s.is_empty())
            .map(|s| String::from_utf8_lossy(s).into_owned())
            .collect();
        if argv.is_empty() {
            None
        } else {
            Some(argv)
        }
    }

    /// `None` for a directory that is not there to reopen a terminal in.
    ///
    /// A directory removed while a process stood in it reads back from the
    /// kernel as `"<path> (deleted)"`, and one inside a private mount
    /// namespace (a sandbox) may not exist from here at all. Relaunching a
    /// terminal at either would fail, so the caller falls back — to the
    /// terminal's own directory, then home.
    fn cwd(&self, pid: i32) -> Option<PathBuf> {
        let path = std::fs::read_link(format!("/proc/{pid}/cwd")).ok()?;
        if path.is_dir() {
            Some(path)
        } else {
            crate::logging::log(format!(
                "capture: pid {pid} stands in {} which no longer exists here; falling back",
                path.display()
            ));
            None
        }
    }

    fn comm(&self, pid: i32) -> Option<String> {
        std::fs::read_to_string(format!("/proc/{pid}/comm"))
            .ok()
            .map(|s| s.trim().to_string())
    }

    fn open_files(&self, pid: i32) -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(format!("/proc/{pid}/fd")) else {
            return Vec::new();
        };
        entries
            .flatten()
            .filter_map(|e| std::fs::read_link(e.path()).ok())
            .map(|p| p.to_string_lossy().into_owned())
            .collect()
    }

    /// The executable's path, even when it has been replaced on disk since
    /// the process started: an upgraded program still reads back as itself,
    /// with the kernel's `" (deleted)"` suffix removed.
    fn exe(&self, pid: i32) -> Option<PathBuf> {
        let path = std::fs::read_link(format!("/proc/{pid}/exe")).ok()?;
        let text = path.to_string_lossy();
        Some(match text.strip_suffix(" (deleted)") {
            Some(stripped) => PathBuf::from(stripped),
            None => path,
        })
    }

    fn children(&self, pid: i32) -> Vec<i32> {
        let mut out = Vec::new();
        let Ok(tasks) = std::fs::read_dir(format!("/proc/{pid}/task")) else {
            return out;
        };
        for task in tasks.flatten() {
            if let Ok(content) = std::fs::read_to_string(task.path().join("children")) {
                out.extend(
                    content
                        .split_whitespace()
                        .filter_map(|p| p.parse::<i32>().ok()),
                );
            }
        }
        out
    }
}

#[cfg(any(test, feature = "testing"))]
pub mod mock {
    use super::*;
    use std::collections::HashMap;

    #[derive(Default)]
    pub struct MockProcessInfo {
        pub cmdlines: HashMap<i32, Vec<String>>,
        pub cwds: HashMap<i32, PathBuf>,
        pub comms: HashMap<i32, String>,
        pub children: HashMap<i32, Vec<i32>>,
        pub open_files: HashMap<i32, Vec<String>>,
        pub exes: HashMap<i32, PathBuf>,
    }

    impl MockProcessInfo {
        /// Register a process; `parent` links it into the tree.
        pub fn add(&mut self, pid: i32, comm: &str, argv: &[&str], cwd: &str) -> &mut Self {
            self.comms.insert(pid, comm.to_string());
            self.cmdlines
                .insert(pid, argv.iter().map(|s| s.to_string()).collect());
            self.cwds.insert(pid, PathBuf::from(cwd));
            self
        }

        pub fn open(&mut self, pid: i32, paths: &[&str]) -> &mut Self {
            self.open_files
                .insert(pid, paths.iter().map(|s| s.to_string()).collect());
            self
        }

        pub fn link(&mut self, parent: i32, child: i32) -> &mut Self {
            self.children.entry(parent).or_default().push(child);
            self
        }
    }

    impl ProcessInfoProvider for MockProcessInfo {
        fn cmdline(&self, pid: i32) -> Option<Vec<String>> {
            self.cmdlines.get(&pid).cloned()
        }
        fn cwd(&self, pid: i32) -> Option<PathBuf> {
            self.cwds.get(&pid).cloned()
        }
        fn comm(&self, pid: i32) -> Option<String> {
            self.comms.get(&pid).cloned()
        }
        fn children(&self, pid: i32) -> Vec<i32> {
            self.children.get(&pid).cloned().unwrap_or_default()
        }
        fn open_files(&self, pid: i32) -> Vec<String> {
            self.open_files.get(&pid).cloned().unwrap_or_default()
        }
        fn exe(&self, pid: i32) -> Option<PathBuf> {
            self.exes.get(&pid).cloned()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::mock::MockProcessInfo;
    use super::*;

    fn names(list: &[&str]) -> BTreeSet<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    /// foot(10) -> fish(11) -> nvim(12)
    fn terminal_tree() -> MockProcessInfo {
        let mut m = MockProcessInfo::default();
        m.add(10, "foot", &["foot"], "/home/user");
        m.add(11, "fish", &["fish"], "/home/user/Work");
        m.add(
            12,
            "nvim",
            &["nvim", "src/main.rs"],
            "/home/user/Work/hyprwake",
        );
        m.link(10, 11).link(11, 12);
        m
    }

    #[test]
    fn finds_a_nested_tui() {
        let m = terminal_tree();
        assert_eq!(
            m.find_descendant(10, &names(&["nvim", "yazi"]), 5),
            Some(12)
        );
    }

    #[test]
    fn finds_the_shell_one_level_down() {
        let m = terminal_tree();
        assert_eq!(
            m.find_descendant(10, &names(&["fish", "bash"]), 5),
            Some(11)
        );
    }

    #[test]
    fn respects_the_depth_bound() {
        let m = terminal_tree();
        // nvim sits two levels below foot; a depth of 1 must not reach it.
        assert_eq!(m.find_descendant(10, &names(&["nvim"]), 1), None);
    }

    #[test]
    fn returns_none_when_nothing_matches() {
        let m = terminal_tree();
        assert_eq!(m.find_descendant(10, &names(&["emacs"]), 5), None);
    }

    #[test]
    fn returns_none_for_unknown_pid() {
        let m = terminal_tree();
        assert_eq!(m.find_descendant(999, &names(&["nvim"]), 5), None);
    }

    #[test]
    fn search_terminates_on_a_cyclic_tree() {
        // /proc can race: a pid may be reported as its own descendant.
        let mut m = MockProcessInfo::default();
        m.add(1, "a", &["a"], "/");
        m.add(2, "b", &["b"], "/");
        m.link(1, 2).link(2, 1);
        assert_eq!(m.find_descendant(1, &names(&["zzz"]), 3), None);
    }

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_script_is_named_by_its_own_name_not_its_interpreters() {
        for (cmdline, want) in [
            (
                vec!["node", "/home/u/.local/bin/gemini", "--yolo"],
                Some(("gemini", 2)),
            ),
            (
                vec!["node", "--no-warnings", "/opt/x/bundle/gemini.js", "-p"],
                Some(("gemini", 3)),
            ),
            (
                vec!["/usr/bin/python3", "/home/u/.local/bin/aider"],
                Some(("aider", 2)),
            ),
            (
                vec!["python3", "-X", "utf8", "-m", "aider.main", "--model", "x"],
                Some(("aider", 5)),
            ),
            (
                vec!["node", "-r", "ts-node/register", "app.ts"],
                Some(("app", 4)),
            ),
            (vec!["python3", "-c", "print(1)"], None),
            (vec!["node"], None),
        ] {
            let got = script_name(&argv(&cmdline));
            assert_eq!(
                got.as_ref().map(|(n, i)| (n.as_str(), *i)),
                want,
                "{cmdline:?}"
            );
        }
    }

    #[test]
    fn an_interpreted_tui_is_found_under_its_terminal() {
        let mut m = MockProcessInfo::default();
        m.add(10, "foot", &["foot"], "/");
        m.add(11, "fish", &["fish"], "/w");
        // Node 26's own name for itself, as /proc/<pid>/comm reports it.
        m.add(
            12,
            "node-MainThread",
            &["node", "/opt/gemini/bundle/gemini.js"],
            "/w",
        );
        m.link(10, 11).link(11, 12);
        assert_eq!(m.program(12).as_deref(), Some("gemini"));
        assert_eq!(m.find_descendant(10, &names(&["gemini"]), 5), Some(12));
        assert_eq!(
            m.find_descendant(10, &names(&["node"]), 5),
            None,
            "a node program is not \"node\""
        );
        // A shell is still found by its own name.
        assert_eq!(m.find_descendant(10, &names(&["fish"]), 5), Some(11));
    }

    #[test]
    fn a_deleted_working_directory_is_not_reported() {
        // The kernel keeps the process standing in the removed directory and
        // names it "<path> (deleted)"; hyprwake once relaunched terminals
        // there.
        let dir = tempfile::TempDir::new().unwrap();
        let gone = dir.path().join("build");
        std::fs::create_dir(&gone).unwrap();
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .current_dir(&gone)
            .spawn()
            .unwrap();
        let pid = child.id() as i32;
        assert_eq!(RealProcessInfo.cwd(pid).as_deref(), Some(gone.as_path()));

        std::fs::remove_dir(&gone).unwrap();
        let raw = std::fs::read_link(format!("/proc/{pid}/cwd")).unwrap();
        assert!(raw.to_string_lossy().ends_with("(deleted)"), "{raw:?}");
        assert_eq!(RealProcessInfo.cwd(pid), None);

        let _ = child.kill();
        let _ = child.wait();
    }

    #[test]
    fn the_real_executable_is_read_and_a_replaced_one_still_named() {
        let me = std::process::id() as i32;
        let exe = RealProcessInfo.exe(me).unwrap();
        assert_eq!(exe, std::env::current_exe().unwrap());
        assert!(!exe.to_string_lossy().ends_with("(deleted)"));
        assert_eq!(RealProcessInfo.exe(-1), None);
    }

    #[test]
    fn real_provider_reads_this_process() {
        let me = std::process::id() as i32;
        let real = RealProcessInfo;
        assert!(real.cmdline(me).is_some_and(|a| !a.is_empty()));
        assert!(real.cwd(me).is_some());
        assert!(real.comm(me).is_some());
        // A process always has at least stdout open.
        assert!(!real.open_files(me).is_empty());
    }
}
