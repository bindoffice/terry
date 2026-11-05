//! Stamps the crate with a fingerprint of the code it was built from.
//!
//! The GUI reuses whatever daemon is already listening on the socket, so after
//! an upgrade it has to be able to tell that the daemon came from an older
//! build. A content hash is stable when the sources did not change (unlike a
//! timestamp) but changes whenever the daemon's code does.
//!
//! `TERRY_DAEMON_BUILD_ID` overrides the hash, for packagers that want their
//! own release identifier.

use std::path::{Path, PathBuf};

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// Local crates whose sources end up inside the daemon's behaviour.
const LOCAL_SOURCES: &[&str] = &[
    "src",
    "../../crates/agent_catalog/src",
    "../../crates/agent_detect/src",
    "../../crates/vendored/alacritty_terminal/src",
    "../../crates/paths/src",
    "../../crates/terminal_core/src",
];

fn main() {
    for path in LOCAL_SOURCES {
        println!("cargo:rerun-if-changed={path}");
    }
    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rerun-if-changed=../../Cargo.lock");
    println!("cargo:rerun-if-env-changed=TERRY_DAEMON_BUILD_ID");

    let build_id = std::env::var("TERRY_DAEMON_BUILD_ID")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(fingerprint);

    println!("cargo:rustc-env=TERRY_DAEMON_BUILD_ID={build_id}");
}

fn fingerprint() -> String {
    let mut files = Vec::new();
    for path in LOCAL_SOURCES {
        collect_files(Path::new(path), &mut files);
    }
    files.push(PathBuf::from("Cargo.toml"));
    files.push(PathBuf::from("../../Cargo.lock"));
    files.sort();

    let mut digest = FNV_OFFSET;
    for file in files {
        hash_bytes(file.to_string_lossy().as_bytes(), &mut digest);
        match std::fs::read(&file) {
            Ok(contents) => hash_bytes(&contents, &mut digest),
            // A missing lockfile (a packaged crate) should still produce a
            // stable id instead of failing the build.
            Err(_) => hash_bytes(b"missing", &mut digest),
        }
    }
    format!("{digest:016x}")
}

fn collect_files(dir: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_files(&path, files);
        } else {
            files.push(path);
        }
    }
}

fn hash_bytes(bytes: &[u8], digest: &mut u64) {
    for byte in bytes {
        *digest ^= u64::from(*byte);
        *digest = digest.wrapping_mul(FNV_PRIME);
    }
}
