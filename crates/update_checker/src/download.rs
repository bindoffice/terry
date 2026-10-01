//! Download a GitHub release asset and install it over the running app when
//! that app is a packaged build.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context as _, Result, anyhow};
use futures::AsyncReadExt;
use http_client::{AsyncBody, HttpClient};

use crate::ReleaseAsset;

/// Where a finished download ended up.
#[derive(Debug)]
pub enum InstallOutcome {
    /// The packaged app was replaced. Restart to run the new build.
    RestartRequired,
    /// The archive is on disk. The running build was left in place.
    PackageReady(PathBuf),
}

/// Picks the archive published for this operating system and CPU.
pub fn select_release_asset<'a>(
    assets: &'a [ReleaseAsset],
    os: &str,
    arch: &str,
) -> Option<&'a ReleaseAsset> {
    let os = match os {
        "macos" | "darwin" => "macos",
        "windows" => "windows",
        "linux" => "linux",
        other => other,
    };
    let marker = format!("-{os}-{arch}.");
    assets.iter().find(|asset| {
        let name = asset.name.to_ascii_lowercase();
        name.contains(&marker) && (name.ends_with(".zip") || name.ends_with(".tar.gz"))
    })
}

/// True when this process is a packaged install, not a `cargo` build under `target/`.
pub fn looks_like_installed_app() -> bool {
    #[cfg(target_os = "macos")]
    {
        running_app_bundle().is_some()
    }
    #[cfg(not(target_os = "macos"))]
    {
        std::env::current_exe().ok().is_some_and(|path| {
            !path
                .components()
                .any(|component| component.as_os_str() == "target")
        })
    }
}

/// `…/Terry.app` when the current executable lives inside a macOS app bundle.
pub fn running_app_bundle() -> Option<PathBuf> {
    running_app_bundle_from(&std::env::current_exe().ok()?)
}

pub fn running_app_bundle_from(exe: &Path) -> Option<PathBuf> {
    let macos_dir = exe.parent()?;
    if macos_dir.file_name()? != "MacOS" {
        return None;
    }
    let contents = macos_dir.parent()?;
    if contents.file_name()? != "Contents" {
        return None;
    }
    let app = contents.parent()?;
    if app.extension()? != "app" {
        return None;
    }
    Some(app.to_path_buf())
}

pub async fn fetch_and_install(
    asset: &ReleaseAsset,
    http_client: &dyn HttpClient,
    mut on_progress: impl FnMut(Option<f32>),
) -> Result<InstallOutcome> {
    let dir = updates_dir()?;
    std::fs::create_dir_all(&dir).with_context(|| format!("failed to create {}", dir.display()))?;
    let archive = dir.join(&asset.name);
    download_to_file(&asset.download_url, &archive, http_client, &mut on_progress)
        .await
        .with_context(|| format!("failed to download {}", asset.name))?;

    if !looks_like_installed_app() {
        return Ok(InstallOutcome::PackageReady(archive));
    }

    match install_downloaded(&archive) {
        Ok(()) => Ok(InstallOutcome::RestartRequired),
        Err(error) => {
            log::warn!("downloaded update but could not install it: {error:#}");
            Ok(InstallOutcome::PackageReady(archive))
        }
    }
}

/// Starts a helper that relaunches Terry after this process exits.
pub fn schedule_relaunch() -> Result<()> {
    let pid = std::process::id();
    let target = running_app_bundle()
        .or_else(|| std::env::current_exe().ok())
        .context("could not locate the app to relaunch")?;
    let dir = updates_dir()?;
    std::fs::create_dir_all(&dir)?;

    #[cfg(target_os = "windows")]
    {
        let script = dir.join("relaunch.bat");
        let body = format!(
            "@echo off\r\n:loop\r\ntasklist /FI \"PID eq {pid}\" | find \"{pid}\" >nul\r\nif not errorlevel 1 (\r\n  timeout /t 1 /nobreak >nul\r\n  goto loop\r\n)\r\nstart \"\" \"{}\"\r\n",
            target.display()
        );
        std::fs::write(&script, body)?;
        Command::new("cmd")
            .args(["/C", "start", "", "/MIN"])
            .arg(&script)
            .spawn()
            .context("failed to start the relaunch helper")?;
    }
    #[cfg(not(target_os = "windows"))]
    {
        let script = dir.join("relaunch.sh");
        let quoted = shell_single_quote(&target.display().to_string());
        let body = format!(
            "#!/bin/sh\nwhile kill -0 {pid} 2>/dev/null; do sleep 0.25; done\nif [ -d {quoted} ]; then\n  open -n {quoted}\nelse\n  {quoted} &\nfi\n"
        );
        std::fs::write(&script, body)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = std::fs::metadata(&script)?.permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&script, permissions)?;
        }
        Command::new(&script)
            .spawn()
            .context("failed to start the relaunch helper")?;
    }
    Ok(())
}

fn updates_dir() -> Result<PathBuf> {
    let base = dirs::data_local_dir()
        .or_else(dirs::data_dir)
        .unwrap_or_else(std::env::temp_dir);
    Ok(base.join("terry").join("updates"))
}

async fn download_to_file(
    url: &str,
    dest: &Path,
    http_client: &dyn HttpClient,
    on_progress: &mut impl FnMut(Option<f32>),
) -> Result<()> {
    let partial = dest.with_extension("partial");
    let mut file = std::fs::File::create(&partial)
        .with_context(|| format!("failed to create {}", partial.display()))?;

    let mut response = http_client
        .get(url, AsyncBody::default(), true)
        .await
        .context("failed to start the download")?;
    let status = response.status();
    if !status.is_success() {
        return Err(anyhow!("download returned {status}"));
    }

    let total_bytes = response
        .headers()
        .get(http_client::http::header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|total| *total > 0);

    let mut downloaded: u64 = 0;
    let mut last_percent: Option<u8> = None;
    let mut buffer = [0u8; 64 * 1024];
    let body = response.body_mut();
    loop {
        let read = body.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        std::io::Write::write_all(&mut file, &buffer[..read])?;
        downloaded += read as u64;
        if let Some(total) = total_bytes {
            let fraction = (downloaded as f32 / total as f32).clamp(0.0, 1.0);
            let percent = (fraction * 100.0) as u8;
            if last_percent != Some(percent) {
                last_percent = Some(percent);
                on_progress(Some(fraction));
            }
        } else if last_percent.is_none() {
            last_percent = Some(0);
            on_progress(None);
        }
    }
    std::io::Write::flush(&mut file)?;
    drop(file);
    if dest.exists() {
        std::fs::remove_file(dest).ok();
    }
    std::fs::rename(&partial, dest)?;
    on_progress(Some(1.0));
    Ok(())
}

fn install_downloaded(archive: &Path) -> Result<()> {
    let extract_dir = archive.with_extension("extracted");
    if extract_dir.exists() {
        std::fs::remove_dir_all(&extract_dir).ok();
    }
    std::fs::create_dir_all(&extract_dir)?;
    extract_archive(archive, &extract_dir)?;

    #[cfg(target_os = "macos")]
    {
        let running = running_app_bundle().context("not running from an app bundle")?;
        let new_app = find_app_bundle(&extract_dir, 3).context("archive has no .app bundle")?;
        let status = Command::new("ditto")
            .arg(&new_app)
            .arg(&running)
            .status()
            .context("failed to run ditto")?;
        anyhow::ensure!(
            status.success(),
            "ditto failed to replace {}",
            running.display()
        );
        return Ok(());
    }

    #[cfg(not(target_os = "macos"))]
    {
        let file_name = if cfg!(target_os = "windows") {
            "terry.exe"
        } else {
            "terry"
        };
        let new_binary =
            find_named_file(&extract_dir, file_name, 4).context("archive has no terry binary")?;
        replace_running_binary(&new_binary)?;
        Ok(())
    }
}

fn extract_archive(archive: &Path, dest: &Path) -> Result<()> {
    let name = archive
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let status = if name.ends_with(".tar.gz") {
        Command::new("tar")
            .arg("-xzf")
            .arg(archive)
            .arg("-C")
            .arg(dest)
            .status()
            .context("failed to run tar")?
    } else if cfg!(target_os = "macos") {
        Command::new("ditto")
            .args(["-x", "-k"])
            .arg(archive)
            .arg(dest)
            .status()
            .context("failed to run ditto")?
    } else {
        Command::new("tar")
            .arg("-xf")
            .arg(archive)
            .arg("-C")
            .arg(dest)
            .status()
            .context("failed to extract zip")?
    };
    anyhow::ensure!(
        status.success(),
        "failed to extract {}",
        archive.display()
    );
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn replace_running_binary(new_binary: &Path) -> Result<()> {
    let dest = std::env::current_exe().context("current executable is unavailable")?;
    let backup = dest.with_extension("old");
    let _ = std::fs::remove_file(&backup);
    std::fs::rename(&dest, &backup)
        .with_context(|| format!("failed to move {} aside", dest.display()))?;
    if let Err(error) = std::fs::copy(new_binary, &dest) {
        let _ = std::fs::rename(&backup, &dest);
        return Err(error).context("failed to copy the new binary into place");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o755));
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn find_app_bundle(dir: &Path, depth: u8) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "app") && path.is_dir() {
            return Some(path);
        }
        if depth > 0
            && path.is_dir()
            && let Some(found) = find_app_bundle(&path, depth - 1)
        {
            return Some(found);
        }
    }
    None
}

#[cfg(not(target_os = "macos"))]
fn find_named_file(dir: &Path, name: &str, depth: u8) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.file_name().is_some_and(|file_name| file_name == name) && path.is_file() {
            return Some(path);
        }
        if depth > 0
            && path.is_dir()
            && let Some(found) = find_named_file(&path, name, depth - 1)
        {
            return Some(found);
        }
    }
    None
}

#[cfg(not(target_os = "windows"))]
fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asset(name: &str) -> ReleaseAsset {
        ReleaseAsset {
            name: name.into(),
            download_url: format!("https://example.com/{name}"),
            size: 10,
        }
    }

    #[test]
    fn selects_the_archive_for_this_platform() {
        let assets = vec![
            asset("Terry-0.2.0-macos-aarch64.zip"),
            asset("Terry-0.2.0-macos-x86_64.zip"),
            asset("Terry-0.2.0-linux-x86_64.tar.gz"),
            asset("Terry-0.2.0-windows-x86_64.zip"),
        ];
        let selected = select_release_asset(&assets, "macos", "aarch64").unwrap();
        assert_eq!(selected.name, "Terry-0.2.0-macos-aarch64.zip");
        let selected = select_release_asset(&assets, "linux", "x86_64").unwrap();
        assert_eq!(selected.name, "Terry-0.2.0-linux-x86_64.tar.gz");
        let selected = select_release_asset(&assets, "windows", "x86_64").unwrap();
        assert_eq!(selected.name, "Terry-0.2.0-windows-x86_64.zip");
        assert!(select_release_asset(&assets, "linux", "aarch64").is_none());
    }

    #[test]
    fn detects_a_macos_app_bundle_path() {
        let exe = Path::new("/Applications/Terry.app/Contents/MacOS/terry");
        assert_eq!(
            running_app_bundle_from(exe).unwrap(),
            PathBuf::from("/Applications/Terry.app")
        );
        assert!(running_app_bundle_from(Path::new("/tmp/target/release/terry")).is_none());
    }
}
