fn main() {
    println!("cargo:rerun-if-env-changed=TERRY_VERSION");
    println!("cargo:rerun-if-env-changed=GITHUB_REF_NAME");

    let version = date_version();
    println!("cargo:rustc-env=TERRY_VERSION={version}");
}

/// Product version as `YYYYMMDD`. A release tag or `TERRY_VERSION` wins so a
/// rebuild later in the day does not change a tagged release.
fn date_version() -> String {
    for raw in [std::env::var("TERRY_VERSION").ok(), std::env::var("GITHUB_REF_NAME").ok()]
        .into_iter()
        .flatten()
    {
        let raw = raw.trim().trim_start_matches('v');
        if is_yyyymmdd(raw) {
            return raw.to_string();
        }
    }
    local_yyyymmdd().unwrap_or_else(utc_yyyymmdd)
}

fn is_yyyymmdd(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 8 || !bytes.iter().all(|byte| byte.is_ascii_digit()) {
        return false;
    }
    let month: u8 = value[4..6].parse().unwrap_or(0);
    let day: u8 = value[6..8].parse().unwrap_or(0);
    (1..=12).contains(&month) && (1..=31).contains(&day)
}

fn local_yyyymmdd() -> Option<String> {
    let output = if cfg!(windows) {
        std::process::Command::new("powershell")
            .args(["-NoProfile", "-Command", "Get-Date -Format yyyyMMdd"])
            .output()
            .ok()?
    } else {
        std::process::Command::new("date")
            .arg("+%Y%m%d")
            .output()
            .ok()?
    };
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let text = text.trim();
    is_yyyymmdd(text).then(|| text.to_string())
}

fn utc_yyyymmdd() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let days = (secs / 86_400) as i64;
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };
    format!("{year:04}{month:02}{day:02}")
}
