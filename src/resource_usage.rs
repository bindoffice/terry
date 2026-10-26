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
        .without_tasks();
    let mut system = System::new();
    system.refresh_processes_specifics(ProcessesToUpdate::All, true, refresh_kind);
    // CPU usage is measured between two refreshes.
    std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
    system.refresh_processes_specifics(ProcessesToUpdate::All, true, refresh_kind);

    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for (pid, process) in system.processes() {
        if let Some(parent) = process.parent() {
            children
                .entry(parent.as_u32())
                .or_default()
                .push(pid.as_u32());
        }
    }

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
