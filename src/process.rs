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

    /// Breadth-first search of the process tree under `pid` for a process
    /// whose `comm` is in `names`.
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
            if let Some(name) = self.comm(current) {
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

    fn cwd(&self, pid: i32) -> Option<PathBuf> {
        std::fs::read_link(format!("/proc/{pid}/cwd")).ok()
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
