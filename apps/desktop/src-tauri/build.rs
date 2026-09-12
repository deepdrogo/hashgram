//! Tauri build script: embeds the config, icons and capabilities, and the
//! git commit this binary was built from (shown in About and Help).

fn main() {
    let commit = std::env::var("HASHGRAM_COMMIT")
        .ok()
        .filter(|c| !c.trim().is_empty())
        .or_else(|| {
            std::process::Command::new("git")
                .args(["rev-parse", "--short=12", "HEAD"])
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        })
        .unwrap_or_else(|| "unknown".to_owned());
    println!("cargo:rustc-env=HASHGRAM_COMMIT={commit}");
    println!("cargo:rerun-if-env-changed=HASHGRAM_COMMIT");
    println!("cargo:rerun-if-changed=../../../.git/HEAD");

    // Sidecars (hashgram-node, its service wrapper) are copied into
    // `binaries/` by release.ps1. Development builds may not have them yet;
    // an empty placeholder keeps `cargo build`/`cargo test` working (the
    // node manager treats an empty file as "not bundled"). Release builds
    // refuse placeholders so an installer never ships without the node.
    let profile = std::env::var("PROFILE").unwrap_or_default();
    let dir = std::path::Path::new("binaries");
    let _ = std::fs::create_dir_all(dir);
    for name in [
        "hashgram-node-x86_64-pc-windows-msvc.exe",
        "hashgram-node-service-x86_64-pc-windows-msvc.exe",
    ] {
        let p = dir.join(name);
        let empty = std::fs::metadata(&p).map(|m| m.len() == 0).unwrap_or(true);
        if empty {
            if profile == "release" && std::env::var("HASHGRAM_ALLOW_EMPTY_SIDECARS").is_err() {
                eprintln!(
                    "sidecar {} is missing or empty; run apps/desktop/release.ps1 (which builds it) or set HASHGRAM_ALLOW_EMPTY_SIDECARS=1 for a build without the node",
                    p.display()
                );
                std::process::exit(1);
            }
            let _ = std::fs::write(&p, b"");
        }
    }
    tauri_build::build();
}
