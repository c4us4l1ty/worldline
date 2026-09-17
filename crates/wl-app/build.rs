//! Shell build script.
//!
//! Fail fast when the embedded frontend is missing: `tauri.conf.json`
//! points `frontendDist` at the Dioxus release bundle
//! (`ui/target/dx/wl-ui/release/web/public`). A `cargo build` against a
//! missing (never built / `cargo clean`ed) dist otherwise produces a
//! binary that boots a silently BLANK window — the single most confusing
//! failure mode of this repo. Building the UI first is required:
//!   cd crates/wl-app/ui && ../../scripts/dx.sh build --release

fn main() {
    let dist =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("ui/target/dx/wl-ui/release/web/public");
    assert!(
        dist.join("index.html").exists(),
        "frontend bundle missing at {} — build it first: cd crates/wl-app/ui && ../../scripts/dx.sh build --release",
        dist.display()
    );
    println!("cargo:rerun-if-changed=tauri.conf.json");
    tauri_build::build()
}
