//! Per-session resource sampling.
//!
//! Walks the process tree below each terminal's shell and aggregates CPU and
//! memory. Blocking: callers run this on the background executor.

use std::collections::{HashMap, HashSet};

use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

#[derive(Clone, Debug)]
pub struct SessionUsage {
    pub pid: u32,
    pub cpu: f32,
    pub memory: u64,
    pub processes: usize,
}

#[derive(Clone, Debug, Default)]
pub struct Usage {
    pub sessions: Vec<SessionUsage>,
    pub total_cpu: f32,
    pub total_memory: u64,
    pub total_processes: usize,
}

/// Samples CPU and memory for the process trees rooted at `pids`.
pub fn measure(pids: &[u32]) -> Usage {
    if pids.is_empty() {
        return Usage::default();
    }

    let refresh_kind = ProcessRefreshKind::nothing()
        .with_cpu()
        .with_memory()
        .with_cmd(UpdateKind::Never)
        .with_exe(UpdateKind::Never)
        .without_tasks();

    // Resolve the descendant set once, then refresh only those pids. Refreshing
    // every process on the machine (`ProcessesToUpdate::All`) twice per sample
    // opened one `/proc/<pid>/stat` handle per process on every tick, which
    // showed up as a burst of hundreds of fds and dominated idle CPU on large
    // machines.
    let tree = collect_tree_pids(pids);
    let mut system = System::new();
    system.refresh_processes_specifics(ProcessesToUpdate::Some(&tree), false, refresh_kind);
    // CPU usage is measured between two refreshes.
    std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
    system.refresh_processes_specifics(ProcessesToUpdate::Some(&tree), false, refresh_kind);

    let children = child_map(&system);

    let mut usage = Usage::default();
    for root in pids {
        let mut stack = vec![*root];
        let mut seen = HashSet::new();
        let mut session = SessionUsage {
            pid: *root,
            cpu: 0.0,
            memory: 0,
            processes: 0,
        };
        while let Some(pid) = stack.pop() {
            if !seen.insert(pid) {
                continue;
            }
            if let Some(process) = system.process(Pid::from_u32(pid)) {
                session.cpu += process.cpu_usage();
                session.memory += process.memory();
                session.processes += 1;
            }
            if let Some(kids) = children.get(&pid) {
                stack.extend(kids.iter().copied());
            }
        }
        usage.total_cpu += session.cpu;
        usage.total_memory += session.memory;
        usage.total_processes += session.processes;
        usage.sessions.push(session);
    }

    usage
}

/// Builds `parent -> children` for the processes currently held by `system`.
fn child_map(system: &System) -> HashMap<u32, Vec<u32>> {
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for (pid, process) in system.processes() {
        if let Some(parent) = process.parent() {
            children
                .entry(parent.as_u32())
                .or_default()
                .push(pid.as_u32());
        }
    }
    children
}

/// Collects the pids of `roots` and all of their descendants.
///
/// On Linux this walks `/proc/<pid>/stat` directly: `sysinfo` only reports
/// parents for processes it already tracks, so giving it `ProcessesToUpdate::All`
/// was the only way to discover the tree — at the cost of stat-ing every process
/// twice per sample.
#[cfg(target_os = "linux")]
pub(crate) fn collect_tree_pids(roots: &[u32]) -> Vec<Pid> {
    let mut parent_of: HashMap<u32, u32> = HashMap::new();
    if let Ok(entries) = std::fs::read_dir("/proc") {
        for entry in entries.flatten() {
            let Some(pid) = entry
                .file_name()
                .to_str()
                .and_then(|name| name.parse::<u32>().ok())
            else {
                continue;
            };
            if let Some(parent) = read_parent_pid(pid) {
                parent_of.insert(pid, parent);
            }
        }
    }

    let mut wanted: HashSet<u32> = roots.iter().copied().collect();
    let mut changed = true;
    while changed {
        changed = false;
        for (pid, parent) in &parent_of {
            if !wanted.contains(pid) && wanted.contains(parent) {
                wanted.insert(*pid);
                changed = true;
            }
        }
    }
    wanted.into_iter().map(Pid::from_u32).collect()
}

/// Reads the parent pid out of `/proc/<pid>/stat`.
///
/// The format is `pid (comm) state ppid ...`, where `comm` may contain spaces
/// and parentheses, so the fields are split after the *last* `)`.
#[cfg(target_os = "linux")]
fn read_parent_pid(pid: u32) -> Option<u32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let close = stat.rfind(')')?;
    stat[close + 1..]
        .split_whitespace()
        .nth(1)
        .and_then(|field| field.parse().ok())
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn collect_tree_pids(roots: &[u32]) -> Vec<Pid> {
    roots.iter().map(|pid| Pid::from_u32(*pid)).collect()
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    /// Regression test: the sampler used to refresh every process on the
    /// machine twice per tick, which opened one `/proc/<pid>/stat` handle per
    /// process each time. The tree must now be discovered without asking
    /// `sysinfo` for the whole process table.
    #[test]
    fn tree_covers_root_and_descendants() {
        // `sleep` is a direct child, so the tree must contain both pids.
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("failed to spawn child process");
        let root = std::process::id();
        let child_pid = child.id();

        let tree: HashSet<u32> = collect_tree_pids(&[root])
            .into_iter()
            .map(|pid| pid.as_u32())
            .collect();

        child.kill().expect("failed to kill child process");
        child.wait().expect("failed to wait for child process");

        assert!(tree.contains(&root), "tree is missing the root pid");
        assert!(
            tree.contains(&child_pid),
            "tree is missing the spawned child pid"
        );
    }

    #[test]
    fn measure_reports_the_root_process() {
        let root = std::process::id();
        let usage = measure(&[root]);
        assert_eq!(usage.sessions.len(), 1);
        assert_eq!(usage.sessions[0].pid, root);
        assert!(
            usage.sessions[0].processes >= 1,
            "the root process must be counted"
        );
        assert!(usage.total_processes >= 1);
    }
}
