use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum ProcessError {
    #[error("process {0} not found")]
    NotFound(u32),
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
}

#[derive(Debug, Clone)]
pub struct ChildProcess {
    pub pid: u32,
    pub cwd: PathBuf,
    pub cmdline: String,
}

pub trait ProcessInfoProvider {
    fn get_cwd(&self, pid: u32) -> Result<PathBuf, ProcessError>;
    fn get_children(&self, pid: u32) -> Result<Vec<ChildProcess>, ProcessError>;
}

pub struct RealProcessInfo;

impl ProcessInfoProvider for RealProcessInfo {
    fn get_cwd(&self, pid: u32) -> Result<PathBuf, ProcessError> {
        std::fs::read_link(format!("/proc/{pid}/cwd")).map_err(|_| ProcessError::NotFound(pid))
    }

    fn get_children(&self, pid: u32) -> Result<Vec<ChildProcess>, ProcessError> {
        let mut children = Vec::new();
        let tasks_dir = format!("/proc/{pid}/task");
        let tasks = std::fs::read_dir(&tasks_dir).map_err(|_| ProcessError::NotFound(pid))?;

        for task in tasks.flatten() {
            let children_file = task.path().join("children");
            if let Ok(content) = std::fs::read_to_string(&children_file) {
                for child_pid_str in content.split_whitespace() {
                    if let Ok(child_pid) = child_pid_str.parse::<u32>() {
                        let cwd = self.get_cwd(child_pid).unwrap_or_default();
                        let cmdline = read_cmdline(child_pid);
                        children.push(ChildProcess {
                            pid: child_pid,
                            cwd,
                            cmdline,
                        });
                    }
                }
            }
        }
        Ok(children)
    }
}

fn read_cmdline(pid: u32) -> String {
    std::fs::read(format!("/proc/{pid}/cmdline"))
        .ok()
        .map(|bytes| {
            bytes
                .split(|&b| b == 0)
                .filter_map(|s| std::str::from_utf8(s).ok())
                .next()
                .unwrap_or("")
                .to_string()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    struct MockProcessInfo {
        cwds: HashMap<u32, PathBuf>,
        children: HashMap<u32, Vec<ChildProcess>>,
    }

    impl ProcessInfoProvider for MockProcessInfo {
        fn get_cwd(&self, pid: u32) -> Result<PathBuf, ProcessError> {
            self.cwds
                .get(&pid)
                .cloned()
                .ok_or(ProcessError::NotFound(pid))
        }

        fn get_children(&self, pid: u32) -> Result<Vec<ChildProcess>, ProcessError> {
            Ok(self.children.get(&pid).cloned().unwrap_or_default())
        }
    }

    #[test]
    fn test_mock_process_info() {
        let parent_pid: u32 = 1000;
        let child_pid: u32 = 1001;
        let child_cwd = PathBuf::from("/home/user/projects");

        let mut cwds = HashMap::new();
        cwds.insert(parent_pid, PathBuf::from("/home/user"));
        cwds.insert(child_pid, child_cwd.clone());

        let child = ChildProcess {
            pid: child_pid,
            cwd: child_cwd.clone(),
            cmdline: "bash".to_string(),
        };

        let mut children_map = HashMap::new();
        children_map.insert(parent_pid, vec![child]);

        let mock = MockProcessInfo {
            cwds,
            children: children_map,
        };

        // Verify get_cwd returns the correct path for the parent
        let parent_cwd = mock.get_cwd(parent_pid).expect("parent cwd should exist");
        assert_eq!(parent_cwd, PathBuf::from("/home/user"));

        // Verify get_children returns correct child data
        let result = mock
            .get_children(parent_pid)
            .expect("should return children");
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].pid, child_pid);
        assert_eq!(result[0].cwd, child_cwd);
        assert_eq!(result[0].cmdline, "bash");
    }

    #[test]
    fn test_mock_cwd_not_found() {
        let mock = MockProcessInfo {
            cwds: HashMap::new(),
            children: HashMap::new(),
        };

        let result = mock.get_cwd(99999);
        assert!(result.is_err());

        match result.unwrap_err() {
            ProcessError::NotFound(pid) => assert_eq!(pid, 99999),
            other => panic!("expected NotFound, got {other}"),
        }
    }

    #[test]
    fn test_mock_children_empty_for_unknown_pid() {
        let mock = MockProcessInfo {
            cwds: HashMap::new(),
            children: HashMap::new(),
        };

        // get_children returns Ok(empty) for unknown PIDs (not an error)
        let result = mock.get_children(99999).expect("should return empty vec");
        assert!(result.is_empty());
    }
}
