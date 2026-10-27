//! Terry kittens: local executable plugins discovered from user directories.
//!
//! These are intentionally a Terry-specific extension point, not a drop-in
//! implementation of kitty's Python kitten API. A kitten receives the
//! `TERRY_*` environment below and can drive the running app through the
//! automation API or the `terry` CLI.

use std::{
    collections::HashSet,
    env, fs,
    path::{Path, PathBuf},
    process::{Command, id},
};

use serde::Deserialize;
use serde_json::json;

#[derive(Clone, Debug)]
struct Kitten {
    name: String,
    path: PathBuf,
}

#[derive(Deserialize)]
struct IpcInfo {
    port: u16,
    token: String,
    #[serde(default)]
    pid: Option<u32>,
}

pub fn run_cli_subcommand(args: &[String]) -> i32 {
    let Some(command) = args.first().map(String::as_str) else {
        print_usage();
        return 2;
    };

    match command {
        "list" => {
            list_kittens();
            0
        }
        "path" => {
            for dir in kitten_dirs() {
                println!("{}", dir.display());
            }
            0
        }
        "run" => {
            let Some(name) = args.get(1) else {
                eprintln!("用法: terry kitten run NAME [ARGS...]");
                return 2;
            };
            run_kitten(name, &args[2..])
        }
        "help" | "--help" | "-h" => {
            print_usage();
            0
        }
        // Shorthand: `terry kitten my-kitten args...`
        name if !name.starts_with('-') => run_kitten(name, &args[1..]),
        other => {
            eprintln!("未知 kitten 子命令: {other}");
            print_usage();
            2
        }
    }
}

fn print_usage() {
    println!("用法: terry kitten <子命令>");
    println!();
    println!("子命令:");
    println!("  list                  列出可用的 kitten 插件");
    println!("  run NAME [ARGS...]    运行一个 kitten");
    println!("  path                  显示 kitten 搜索目录");
    println!("  help                  显示本帮助");
    println!();
    println!("`terry kitten NAME [ARGS...]` 等价于 `terry kitten run NAME [ARGS...]`。");
    println!("`terry plugin` 是 `terry kitten` 的别名。");
}

fn list_kittens() {
    let directories: Vec<_> = kitten_dirs()
        .into_iter()
        .map(|dir| {
            json!({
                "path": dir.display().to_string(),
                "exists": dir.is_dir(),
            })
        })
        .collect();
    let kittens: Vec<_> = discover_kittens()
        .into_iter()
        .map(|kitten| {
            json!({
                "name": kitten.name,
                "path": kitten.path.display().to_string(),
            })
        })
        .collect();

    match serde_json::to_string_pretty(&json!({
        "directories": directories,
        "kittens": kittens,
    })) {
        Ok(output) => println!("{output}"),
        Err(error) => eprintln!("无法序列化 kitten 列表: {error}"),
    }
}

fn run_kitten(name: &str, args: &[String]) -> i32 {
    let Some(kitten) = discover_kittens()
        .into_iter()
        .find(|kitten| kitten.name == name)
    else {
        eprintln!("未找到 kitten {name:?}");
        eprintln!("搜索目录:");
        for dir in kitten_dirs() {
            eprintln!("  {}", dir.display());
        }
        return 1;
    };

    let mut command = match command_for(&kitten.path) {
        Ok(command) => command,
        Err(error) => {
            eprintln!("{error}");
            return 1;
        }
    };
    command.args(args);
    apply_kitten_env(&mut command, &kitten);

    match command.status() {
        Ok(status) => status.code().unwrap_or(1),
        Err(error) => {
            eprintln!("无法运行 {}: {error}", kitten.path.display());
            1
        }
    }
}

fn command_for(path: &Path) -> Result<Command, String> {
    #[cfg(windows)]
    {
        let extension = path
            .extension()
            .and_then(|extension| extension.to_str())
            .map(|extension| extension.to_ascii_lowercase());
        match extension.as_deref() {
            Some("cmd" | "bat") => {
                let mut command = Command::new("cmd");
                command.arg("/C").arg(path);
                Ok(command)
            }
            Some("ps1") => {
                let mut command = Command::new("powershell");
                command
                    .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
                    .arg(path);
                Ok(command)
            }
            _ => Ok(Command::new(path)),
        }
    }

    #[cfg(not(windows))]
    {
        Ok(Command::new(path))
    }
}

fn apply_kitten_env(command: &mut Command, kitten: &Kitten) {
    command
        .env("TERRY_KITTEN", "1")
        .env("TERRY_KITTEN_NAME", &kitten.name)
        .env(
            "TERRY_KITTEN_DIR",
            kitten.path.parent().unwrap_or(Path::new(".")),
        )
        .env("TERRY_VERSION", env!("TERRY_VERSION"))
        .env("TERRY_CONFIG_DIR", paths::config_dir())
        .env("TERRY_DATA_DIR", paths::data_dir())
        .env("TERRY_CLI_PID", id().to_string());

    if let Ok(joined) = env::join_paths(kitten_dirs()) {
        command.env("TERRY_KITTEN_PATH", joined);
    }

    if let Some(info) = read_ipc_info() {
        command
            .env("TERRY_PID", info.pid.unwrap_or(id()).to_string())
            .env("TERRY_IPC_PORT", info.port.to_string())
            .env("TERRY_IPC_TOKEN", info.token)
            .env("TERRY_IPC_URL", format!("http://127.0.0.1:{}", info.port));
    } else {
        command.env("TERRY_PID", id().to_string());
    }
}

fn read_ipc_info() -> Option<IpcInfo> {
    let text = fs::read_to_string(paths::data_dir().join("ipc.json")).ok()?;
    serde_json::from_str(&text).ok()
}

fn kitten_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(value) = env::var_os("TERRY_KITTEN_PATH") {
        dirs.extend(env::split_paths(&value));
    }
    dirs.push(paths::config_dir().join("kittens"));
    dirs.push(paths::data_dir().join("kittens"));
    dirs
}

fn discover_kittens() -> Vec<Kitten> {
    discover_in_dirs(&kitten_dirs())
}

fn discover_in_dirs(dirs: &[PathBuf]) -> Vec<Kitten> {
    let mut seen = HashSet::new();
    let mut kittens = Vec::new();

    for dir in dirs {
        let Ok(entries) = fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(name) = kitten_name(&path) else {
                continue;
            };
            if !is_executable(&path) || !seen.insert(name.clone()) {
                continue;
            }
            kittens.push(Kitten { name, path });
        }
    }

    kittens.sort_by(|a, b| a.name.cmp(&b.name));
    kittens
}

fn kitten_name(path: &Path) -> Option<String> {
    if !path.is_file() {
        return None;
    }
    kitten_name_from_file_name(path.file_name()?.to_str()?)
}

fn kitten_name_from_file_name(file_name: &str) -> Option<String> {
    if file_name.is_empty() || file_name.starts_with('.') {
        return None;
    }

    let (stem, extension) = match file_name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => (stem, Some(extension.to_ascii_lowercase())),
        _ => (file_name, None),
    };

    // Ignore documentation/metadata that often sits next to plugins.
    if matches!(
        extension.as_deref(),
        Some("json" | "toml" | "md" | "txt" | "png" | "jpg" | "jpeg" | "gif" | "svg" | "lock")
    ) {
        return None;
    }

    if matches!(
        extension.as_deref(),
        Some(
            "sh" | "bash"
                | "zsh"
                | "fish"
                | "py"
                | "rb"
                | "js"
                | "lua"
                | "pl"
                | "kitten"
                | "exe"
                | "cmd"
                | "bat"
                | "ps1"
                | "com"
        )
    ) {
        Some(stem.to_string())
    } else {
        Some(file_name.to_string())
    }
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    path.is_file()
        && path
            .metadata()
            .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
}

#[cfg(windows)]
fn is_executable(path: &Path) -> bool {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.to_ascii_lowercase());
    path.is_file()
        && matches!(
            extension.as_deref(),
            Some("exe" | "cmd" | "bat" | "ps1" | "com")
        )
}

#[cfg(not(any(unix, windows)))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_names_strip_known_script_extensions() {
        assert_eq!(kitten_name_from_file_name("hello"), Some("hello".into()));
        assert_eq!(kitten_name_from_file_name("hello.sh"), Some("hello".into()));
        assert_eq!(kitten_name_from_file_name("hello.py"), Some("hello".into()));
        assert_eq!(
            kitten_name_from_file_name("hello.kitten"),
            Some("hello".into())
        );
        assert_eq!(kitten_name_from_file_name("notes.md"), None);
        assert_eq!(kitten_name_from_file_name("README"), Some("README".into()));
        assert_eq!(kitten_name_from_file_name(".hidden"), None);
    }

    #[cfg(unix)]
    #[test]
    fn discovers_executable_kittens_in_order() {
        use std::os::unix::fs::PermissionsExt;
        use std::time::{SystemTime, UNIX_EPOCH};

        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = env::temp_dir().join(format!("terry-kitten-test-{unique}"));
        fs::create_dir_all(&dir).unwrap();
        let script = dir.join("hello.sh");
        fs::write(&script, "#!/bin/sh\n").unwrap();
        let mut permissions = fs::metadata(&script).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&script, permissions).unwrap();
        fs::write(dir.join("notes.md"), "not a plugin").unwrap();

        let kittens = discover_in_dirs(&[dir.clone()]);
        assert_eq!(kittens.len(), 1);
        assert_eq!(kittens[0].name, "hello");
        assert_eq!(kittens[0].path, script);

        fs::remove_dir_all(dir).ok();
    }
}
