//! `terry install-cli` / `terry uninstall-cli`.
//!
//! Terminals spawned by Terry always get `terry` and `terry-daemon` on `PATH`
//! (see `terminal::shell_integration`). This installs the same shims into a
//! user-level bin directory so shells Terry did not spawn can use the CLI too.

use std::path::{Path, PathBuf};

/// Overrides the install directory without passing `--dir`.
const CLI_DIR_ENV: &str = "TERRY_CLI_DIR";

const PATH_SEPARATOR: char = if cfg!(windows) { ';' } else { ':' };

/// Runs `terry install-cli [--dir DIR]`.
pub fn run_install_cli(args: &[String]) -> i32 {
    run(args, true)
}

/// Runs `terry uninstall-cli [--dir DIR]`.
pub fn run_uninstall_cli(args: &[String]) -> i32 {
    run(args, false)
}

fn run(args: &[String], install: bool) -> i32 {
    let (dir_flag, help) = match parse_args(args) {
        Ok(parsed) => parsed,
        Err(error) => {
            eprintln!("{error}");
            return 2;
        }
    };
    if help {
        print_usage();
        return 0;
    }

    let dir = install_dir_from(dir_flag, std::env::var(CLI_DIR_ENV).ok());
    let result = if install {
        terminal::shell_integration::install_cli_shims_to(&dir)
    } else {
        terminal::shell_integration::uninstall_cli_shims_from(&dir)
    };

    match result {
        Ok(paths) if paths.is_empty() => {
            println!("没有找到由 Terry 安装的 shim（{}）", dir.display());
            0
        }
        Ok(paths) => {
            for path in paths {
                println!(
                    "{} {}",
                    if install { "installed" } else { "removed" },
                    path.display()
                );
            }
            if install && !is_on_path(&dir, std::env::var("PATH").ok().as_deref()) {
                print_path_hint(&dir);
            }
            0
        }
        Err(error) => {
            eprintln!("terry: {error:#}");
            if install {
                if format!("{error:#}").contains("not created by Terry") {
                    eprintln!("提示: 请先移走或重命名该文件，或用 --dir 指定其他目录");
                } else {
                    eprintln!(
                        "提示: 如果目录不可写，用 --dir 指定一个用户目录，例如 --dir ~/.local/bin"
                    );
                }
            }
            1
        }
    }
}

fn parse_args(args: &[String]) -> Result<(Option<PathBuf>, bool), String> {
    let mut dir = None;
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        match arg.as_str() {
            "--help" | "-h" => return Ok((dir, true)),
            "--dir" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| "--dir 需要一个目录参数".to_string())?;
                dir = Some(PathBuf::from(value));
            }
            _ => {
                if let Some(value) = arg.strip_prefix("--dir=") {
                    if value.is_empty() {
                        return Err("--dir 需要一个目录参数".to_string());
                    }
                    dir = Some(PathBuf::from(value));
                } else {
                    return Err(format!(
                        "未知参数: {arg}\n用法: terry install-cli [--dir DIR] | terry uninstall-cli [--dir DIR]"
                    ));
                }
            }
        }
        index += 1;
    }
    Ok((dir, false))
}

/// `--dir` wins over `$TERRY_CLI_DIR`, which wins over the platform default.
fn install_dir_from(explicit: Option<PathBuf>, env: Option<String>) -> PathBuf {
    explicit
        .or_else(|| env.filter(|dir| !dir.trim().is_empty()).map(PathBuf::from))
        .unwrap_or_else(default_install_dir)
}

fn default_install_dir() -> PathBuf {
    if cfg!(windows) {
        paths::data_dir().join("bin")
    } else {
        util::paths::home_dir().join(".local").join("bin")
    }
}

fn is_on_path(dir: &Path, path_var: Option<&str>) -> bool {
    let Some(path_var) = path_var else {
        return false;
    };
    path_var
        .split(PATH_SEPARATOR)
        .filter(|entry| !entry.is_empty())
        .any(|entry| same_path(Path::new(entry), dir))
}

fn same_path(left: &Path, right: &Path) -> bool {
    let left = normalize(left);
    let right = normalize(right);
    if cfg!(windows) {
        left.eq_ignore_ascii_case(&right)
    } else {
        left == right
    }
}

fn normalize(path: &Path) -> String {
    path.to_string_lossy()
        .trim_end_matches(['/', '\\'])
        .to_string()
}

fn print_path_hint(dir: &Path) {
    let dir = dir.display();
    eprintln!();
    eprintln!("提示: {dir} 不在 PATH 中，重新打开终端前先把它加入 shell 配置，例如:");
    eprintln!("  zsh:  echo 'export PATH=\"{dir}:$PATH\"' >> ~/.zshrc");
    eprintln!("  fish: fish_add_path '{dir}'");
}

fn print_usage() {
    println!("用法: terry install-cli [--dir DIR]");
    println!("      terry uninstall-cli [--dir DIR]");
    println!();
    println!("把 `terry` 和 `terry-daemon` 安装到一个在 PATH 中的目录，");
    println!("让 Terry 之外的终端也能使用 CLI。`terry-daemon ...` 等价于 `terry daemon ...`。");
    println!();
    println!(
        "默认目录: {}（也可用 TERRY_CLI_DIR 指定）",
        default_install_dir().display()
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_and_uninstall_round_trip() {
        let dir = std::env::temp_dir().join(format!(
            "terry-install-cli-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let dir_arg = dir.display().to_string();

        assert_eq!(run_install_cli(&["--dir".to_string(), dir_arg.clone()]), 0);
        assert!(dir.join(cli_file_name("terry")).is_file());
        assert!(dir.join(cli_file_name("terry-daemon")).is_file());

        assert_eq!(run_uninstall_cli(&[format!("--dir={dir_arg}")]), 0);
        assert!(!dir.join(cli_file_name("terry")).exists());
        assert!(!dir.join(cli_file_name("terry-daemon")).exists());

        std::fs::remove_dir_all(&dir).ok();
    }

    fn cli_file_name(name: &str) -> String {
        if cfg!(windows) {
            format!("{name}.cmd")
        } else {
            name.to_string()
        }
    }

    #[test]
    fn install_dir_precedence() {
        assert_eq!(
            install_dir_from(Some(PathBuf::from("/flag")), Some("/env".to_string())),
            PathBuf::from("/flag")
        );
        assert_eq!(
            install_dir_from(None, Some("/env".to_string())),
            PathBuf::from("/env")
        );
        assert_eq!(
            install_dir_from(None, Some("   ".to_string())),
            default_install_dir()
        );
        assert_eq!(install_dir_from(None, None), default_install_dir());
    }

    #[test]
    fn recognizes_directories_already_on_path() {
        let path =
            format!("/usr/bin{PATH_SEPARATOR}/opt/terry/bin{PATH_SEPARATOR}/home/me/.local/bin/");
        assert!(is_on_path(Path::new("/opt/terry/bin"), Some(&path)));
        // A trailing slash in PATH must not hide the directory.
        assert!(is_on_path(Path::new("/home/me/.local/bin"), Some(&path)));
        // Prefix matches are different directories.
        assert!(!is_on_path(Path::new("/opt/terry"), Some(&path)));
        assert!(!is_on_path(Path::new("/bin"), Some("")));
        assert!(!is_on_path(Path::new("/bin"), None));
    }
}
