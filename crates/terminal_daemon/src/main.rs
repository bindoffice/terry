//! `terry-daemon`: the optional terminal daemon.
//!
//! The GUI starts this automatically when `terminal.persistent_sessions` is
//! enabled. It can also be driven by hand for debugging:
//!
//! ```sh
//! terry-daemon serve --socket /tmp/terry.sock
//! terry-daemon list  --socket /tmp/terry.sock
//! terry-daemon kill my-session --socket /tmp/terry.sock
//! ```

use std::path::PathBuf;

use terminal_daemon::Control;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    let result = match args.first().map(String::as_str) {
        Some("serve") => serve(&args[1..]),
        Some("list") => list(&args[1..]),
        Some("kill") => kill(&args[1..]),
        Some("help" | "--help" | "-h") | None => {
            usage();
            Ok(())
        }
        Some(other) => {
            eprintln!("terry-daemon: unknown command {other}");
            usage();
            std::process::exit(2);
        }
    };

    if let Err(error) = result {
        eprintln!("terry-daemon: {error}");
        std::process::exit(1);
    }
}

fn usage() {
    println!("用法: terry-daemon <serve|list|kill> [--socket PATH]");
    println!();
    println!("  serve              启动守护进程（GUI 在开启持久会话时会自动启动）");
    println!("  list               列出会话（JSON）");
    println!("  kill SESSION       结束会话及其子进程");
    println!();
    println!("默认 socket: {}", terminal_daemon::socket_path().display());
}

/// Parses `--socket PATH` and returns it with the remaining positional args.
fn parse_args(args: &[String]) -> (PathBuf, Vec<String>) {
    let mut socket = terminal_daemon::socket_path();
    let mut positional = Vec::new();
    let mut index = 0;
    while index < args.len() {
        if args[index] == "--socket" {
            if let Some(value) = args.get(index + 1) {
                socket = PathBuf::from(value);
                index += 2;
                continue;
            }
            index += 1;
            continue;
        }
        if let Some(value) = args[index].strip_prefix("--socket=") {
            socket = PathBuf::from(value);
            index += 1;
            continue;
        }
        positional.push(args[index].clone());
        index += 1;
    }
    (socket, positional)
}

fn serve(args: &[String]) -> anyhow::Result<()> {
    let (socket, _) = parse_args(args);
    terminal_daemon::serve(&socket)
}

fn list(args: &[String]) -> anyhow::Result<()> {
    let (socket, _) = parse_args(args);
    let mut control = Control::connect(&socket)?;
    let sessions = control.list()?;
    println!("{}", serde_json::to_string_pretty(&sessions)?);
    Ok(())
}

fn kill(args: &[String]) -> anyhow::Result<()> {
    let (socket, positional) = parse_args(args);
    let Some(session) = positional.first() else {
        anyhow::bail!("kill 需要一个 session id");
    };
    let mut control = Control::connect(&socket)?;
    control.kill(session)?;
    println!("killed {session}");
    Ok(())
}
