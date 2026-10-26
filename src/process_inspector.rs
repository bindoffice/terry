//! Per-terminal process and port inspector.
//!
//! Lists the process tree rooted at a terminal's shell together with the TCP
//! ports those processes are listening on. Ports can be opened in the browser
//! and processes can be terminated from the same window.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use gpui::{
    App, AppContext as _, Context, FocusHandle, Focusable, IntoElement, ParentElement, Render,
    SharedString, Task, TitlebarOptions, WeakEntity, Window, WindowBounds, WindowOptions, actions,
    div, px,
};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
use terminal_view::TerminalView;
use theme::ActiveTheme;
use ui::{
    Button, ButtonSize, ButtonStyle, Color, Icon, IconButton, IconName, IconSize, Label, LabelSize,
    Tooltip, prelude::*,
};
use util::ResultExt;

actions!(process_inspector, [OpenProcessInspector]);

/// How often the window refreshes while it is open.
const REFRESH_INTERVAL: Duration = Duration::from_millis(2000);

#[derive(Clone, Debug)]
struct InspectorProcess {
    pid: u32,
    depth: usize,
    name: String,
    command: String,
}

#[derive(Clone, Debug)]
struct InspectorPort {
    port: u16,
    pid: u32,
    command: String,
}

#[derive(Clone, Debug, Default)]
struct Snapshot {
    processes: Vec<InspectorProcess>,
    ports: Vec<InspectorPort>,
    error: Option<String>,
}

/// Collects the process tree under `root_pid` and the ports it listens on.
/// Blocking; run it on the background executor.
fn collect(root_pid: u32) -> Snapshot {
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing()
            .with_cmd(UpdateKind::Always)
            .without_tasks(),
    );

    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for (pid, process) in system.processes() {
        if let Some(parent) = process.parent() {
            children
                .entry(parent.as_u32())
                .or_default()
                .push(pid.as_u32());
        }
    }

    let mut processes = Vec::new();
    let mut seen = HashSet::new();
    walk(&system, &children, root_pid, 0, &mut seen, &mut processes);

    // A shell can exit while the window is open; keep whatever is left.
    if processes.is_empty() {
        return Snapshot {
            processes,
            ports: Vec::new(),
            error: None,
        };
    }

    let pids: HashSet<u32> = processes.iter().map(|process| process.pid).collect();
    let (ports, error) = match collect_ports(&pids) {
        Ok(ports) => (ports, None),
        Err(error) => (Vec::new(), Some(error)),
    };

    Snapshot {
        processes,
        ports,
        error,
    }
}

fn walk(
    system: &System,
    children: &HashMap<u32, Vec<u32>>,
    pid: u32,
    depth: usize,
    seen: &mut HashSet<u32>,
    out: &mut Vec<InspectorProcess>,
) {
    if !seen.insert(pid) {
        return;
    }
    let Some(process) = system.process(Pid::from_u32(pid)) else {
        return;
    };
    let name = process.name().to_string_lossy().to_string();
    let command = process
        .cmd()
        .iter()
        .map(|arg| arg.to_string_lossy())
        .collect::<Vec<_>>()
        .join(" ");
    let command = if command.is_empty() {
        name.clone()
    } else {
        command
    };
    out.push(InspectorProcess {
        pid,
        depth,
        name,
        command,
    });

    if let Some(kids) = children.get(&pid) {
        let mut kids = kids.clone();
        kids.sort_unstable();
        for kid in kids {
            walk(system, children, kid, depth + 1, seen, out);
        }
    }
}

/// Parses `lsof -nP -iTCP -sTCP:LISTEN -F pcn` output, filtered to `pids`.
fn collect_ports(pids: &HashSet<u32>) -> Result<Vec<InspectorPort>, String> {
    let output = std::process::Command::new("lsof")
        .args(["-nP", "-iTCP", "-sTCP:LISTEN", "-F", "pcn"])
        .output()
        .map_err(|error| format!("lsof failed: {error}"))?;

    let mut current_pid = 0;
    let mut current_command = String::new();
    let mut ports = Vec::new();
    let mut seen = HashSet::new();

    for line in String::from_utf8_lossy(&output.stdout).lines() {
        if let Some(rest) = line.strip_prefix('p') {
            current_pid = rest.trim().parse().unwrap_or(0);
            current_command.clear();
        } else if let Some(rest) = line.strip_prefix('c') {
            current_command = rest.trim().to_string();
        } else if let Some(rest) = line.strip_prefix('n') {
            if !pids.contains(&current_pid) {
                continue;
            }
            let Some(port) = rest.rsplit(':').next().and_then(|port| port.parse().ok()) else {
                continue;
            };
            if seen.insert(port) {
                ports.push(InspectorPort {
                    port,
                    pid: current_pid,
                    command: current_command.clone(),
                });
            }
        }
    }

    ports.sort_by_key(|port| port.port);
    Ok(ports)
}

fn kill_process(pid: u32) {
    let _ = std::process::Command::new("kill")
        .arg("-TERM")
        .arg(pid.to_string())
        .status();
}

/// Opens (or focuses) the inspector for `view`.
pub fn open(view: WeakEntity<TerminalView>, cx: &mut App) {
    let Some(view_entity) = view.upgrade() else {
        return;
    };
    let (shell_pid, title) = {
        let view = view_entity.read(cx);
        let terminal = view.terminal().read(cx);
        (
            terminal
                .pid_getter()
                .map(|getter| getter.fallback_pid().as_u32()),
            terminal.title(true),
        )
    };

    if let Some(existing) = cx
        .windows()
        .into_iter()
        .find_map(|window| window.downcast::<ProcessInspectorWindow>())
    {
        existing
            .update(cx, |this, window, cx| {
                if this.shell_pid != shell_pid {
                    this.shell_pid = shell_pid;
                    this.title = title.clone().into();
                    this.snapshot = Snapshot::default();
                    this.schedule_refresh(cx);
                }
                window.activate_window();
                cx.notify();
            })
            .log_err();
        return;
    }

    cx.defer(move |cx| {
        cx.open_window(
            WindowOptions {
                titlebar: Some(TitlebarOptions {
                    title: Some(format!("Terry — {}", i18n::t("process_inspector")).into()),
                    appears_transparent: false,
                    traffic_light_position: None,
                }),
                focus: true,
                show: true,
                is_movable: true,
                kind: gpui::WindowKind::Normal,
                window_background: cx.theme().window_background_appearance(),
                window_bounds: Some(WindowBounds::centered(gpui::size(px(640.), px(560.)), cx)),
                window_min_size: Some(gpui::Size {
                    width: px(420.),
                    height: px(320.),
                }),
                ..Default::default()
            },
            move |window, cx| {
                cx.new(|cx| ProcessInspectorWindow::new(shell_pid, title, window, cx))
            },
        )
        .log_err();
    });
}

pub struct ProcessInspectorWindow {
    focus_handle: FocusHandle,
    shell_pid: Option<u32>,
    title: SharedString,
    snapshot: Snapshot,
    refresh_task: Option<Task<()>>,
}

impl ProcessInspectorWindow {
    fn new(
        shell_pid: Option<u32>,
        title: String,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self {
            focus_handle: cx.focus_handle(),
            shell_pid,
            title: title.into(),
            snapshot: Snapshot::default(),
            refresh_task: None,
        };
        this.schedule_refresh(cx);
        this
    }

    fn schedule_refresh(&mut self, cx: &mut Context<Self>) {
        let shell_pid = self.shell_pid;
        self.refresh_task = Some(cx.spawn(async move |this, cx| {
            loop {
                let snapshot = match shell_pid {
                    Some(pid) => {
                        cx.background_executor()
                            .spawn(async move { collect(pid) })
                            .await
                    }
                    None => Snapshot::default(),
                };
                let alive = this
                    .update(cx, |this, cx| {
                        this.snapshot = snapshot;
                        cx.notify();
                    })
                    .is_ok();
                if !alive {
                    break;
                }
                cx.background_executor().timer(REFRESH_INTERVAL).await;
            }
        }));
    }

    fn refresh_now(&mut self, cx: &mut Context<Self>) {
        self.schedule_refresh(cx);
        cx.notify();
    }

    fn kill(&mut self, pid: u32, cx: &mut Context<Self>) {
        cx.background_executor()
            .spawn(async move { kill_process(pid) })
            .detach();
        self.refresh_now(cx);
    }

    fn render_process(&mut self, process: &InspectorProcess, cx: &mut Context<Self>) -> AnyElement {
        let this = cx.entity().downgrade();
        let pid = process.pid;
        let is_root = process.depth == 0;
        let command = process.command.clone();
        let detail = format!("#{} · {}", process.pid, command);

        h_flex()
            .w_full()
            .px_2()
            .py_1()
            .gap_2()
            .items_center()
            .rounded_md()
            .hover(|style| style.bg(cx.theme().colors().element_hover))
            .child(div().w_4())
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .ml(px(12.) * process.depth as f32)
                    .child(Label::new(process.name.clone()).size(LabelSize::Small))
                    .child(
                        Label::new(detail)
                            .size(LabelSize::XSmall)
                            .color(Color::Muted),
                    ),
            )
            .when(!is_root, move |el| {
                el.child(
                    IconButton::new(
                        SharedString::from(format!("kill-process-{pid}")),
                        IconName::Close,
                    )
                    .icon_size(IconSize::Small)
                    .tooltip(Tooltip::text(i18n::t("kill_process")))
                    .on_click(move |_, _, cx| {
                        this.update(cx, |this, cx| this.kill(pid, cx)).ok();
                    }),
                )
            })
            .into_any_element()
    }

    fn render_port(&self, port: &InspectorPort, cx: &App) -> AnyElement {
        let url = format!("http://localhost:{}", port.port);
        let label = format!("localhost:{}", port.port);
        let detail = format!("#{} · {}", port.pid, port.command);

        h_flex()
            .w_full()
            .px_2()
            .py_1()
            .gap_2()
            .items_center()
            .rounded_md()
            .hover(|style| style.bg(cx.theme().colors().element_hover))
            .child(
                Icon::new(IconName::ArrowUpRight)
                    .size(IconSize::Small)
                    .color(Color::Accent),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(
                        Label::new(label)
                            .size(LabelSize::Small)
                            .color(Color::Accent),
                    )
                    .child(
                        Label::new(detail)
                            .size(LabelSize::XSmall)
                            .color(Color::Muted),
                    ),
            )
            .child(
                Button::new(
                    SharedString::from(format!("open-port-{}", port.port)),
                    i18n::t("open_in_browser"),
                )
                .style(ButtonStyle::Outlined)
                .size(ButtonSize::Compact)
                .on_click(move |_, _, cx| {
                    cx.open_url(&url);
                }),
            )
            .into_any_element()
    }

    fn render_header(&self, cx: &mut Context<Self>) -> AnyElement {
        let this = cx.entity().downgrade();
        h_flex()
            .px_4()
            .pt_3()
            .pb_2()
            .gap_2()
            .items_center()
            .border_b_1()
            .border_color(cx.theme().colors().border)
            .child(
                Icon::new(IconName::Terminal)
                    .size(IconSize::Small)
                    .color(Color::Muted),
            )
            .child(Label::new(i18n::t("process_inspector")))
            .child(
                Label::new(self.title.clone())
                    .size(LabelSize::XSmall)
                    .color(Color::Muted),
            )
            .child(div().flex_1())
            .child(
                Button::new("refresh-process-inspector", i18n::t("refresh"))
                    .style(ButtonStyle::Outlined)
                    .size(ButtonSize::Compact)
                    .on_click(move |_, _, cx| {
                        this.update(cx, |this, cx| this.refresh_now(cx)).ok();
                    }),
            )
            .into_any_element()
    }
}

impl Focusable for ProcessInspectorWindow {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ProcessInspectorWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let process_rows: Vec<AnyElement> = self
            .snapshot
            .processes
            .clone()
            .iter()
            .map(|process| self.render_process(process, cx))
            .collect();
        let port_rows: Vec<AnyElement> = self
            .snapshot
            .ports
            .iter()
            .map(|port| self.render_port(port, cx))
            .collect();
        let error = self.snapshot.error.clone();

        div()
            .id("terry-process-inspector")
            .key_context("ProcessInspector")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|_, _: &menu::Cancel, window, _cx| {
                window.remove_window();
            }))
            .size_full()
            .flex()
            .flex_col()
            .bg(cx.theme().colors().background)
            .text_color(cx.theme().colors().text)
            .child(self.render_header(cx))
            .child(
                div()
                    .id("terry-process-inspector-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_2()
                    .gap_3()
                    .flex()
                    .flex_col()
                    .child(
                        v_flex()
                            .gap_1()
                            .child(
                                Label::new(i18n::t("processes"))
                                    .size(LabelSize::XSmall)
                                    .color(Color::Muted),
                            )
                            .children(process_rows),
                    )
                    .child(
                        v_flex()
                            .gap_1()
                            .child(
                                Label::new(i18n::t("listening_ports"))
                                    .size(LabelSize::XSmall)
                                    .color(Color::Muted),
                            )
                            .when(port_rows.is_empty(), |this| {
                                this.child(
                                    Label::new(i18n::t("no_listening_ports"))
                                        .size(LabelSize::XSmall)
                                        .color(Color::Muted),
                                )
                            })
                            .children(port_rows),
                    )
                    .when_some(error, |this, error| {
                        this.child(
                            Label::new(error)
                                .size(LabelSize::XSmall)
                                .color(Color::Error),
                        )
                    }),
            )
    }
}
