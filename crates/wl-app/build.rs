//! Shell build script.
//!
//! Two failure modes of this repo are confusing enough to be worth a
//! build-time guard, and both are caught here rather than at runtime.
//!
//! **1. Missing frontend bundle.** `tauri.conf.json` points
//! `frontendDist` at the Dioxus release bundle
//! (`ui/target/dx/wl-ui/release/web/public`). Building against a missing
//! (never built / `cargo clean`ed) dist produces a binary that boots a
//! silently BLANK window. Building the UI first is required:
//!   cd crates/wl-app/ui && ../../scripts/dx.sh build --release
//!
//! **2. Debug binary without the dev server — "Could not connect to
//! localhost: Connection refused".** Tauri picks its frontend source at
//! COMPILE time from the `custom-protocol` Cargo feature:
//!
//! * feature ON  → `frontendDist` (the embedded bundle; self-contained)
//! * feature OFF → `devUrl`, i.e. `http://localhost:1420`
//!
//! `cargo tauri build` and `cargo tauri dev` pass the feature the right
//! way, but a plain `cargo build` / `cargo run` does NOT — so
//! `target/debug/wl-app` always loads `http://localhost:1420`. Run it
//! without `dx serve` on that port and the webview shows the *browser's*
//! error page inside the Tauri window, which reads like the app is
//! broken when nothing is wrong with the app at all.
//!
//! This cannot be fixed by asserting: the bundle really is present, and
//! the binary really is valid. So it is surfaced as a loud build warning
//! naming both correct commands, rather than left to be rediscovered.
//
// **3. Two different CSPs, and they must not be confused.**
// `tauri.conf.json` carries both `csp` (shipped builds) and `devCsp`
// (applied when `custom-protocol` is OFF):
//
// * `csp` — `connect-src 'self'`. The webview only ever talks to its
//   own embedded bundle; every relay and provider request is made by
//   the Rust side with reqwest, which the CSP does not govern. The
//   previous policy allowed ANY localhost port in shipped builds, a
//   standing SSRF surface for any script executing in the webview.
// * `devCsp` — additionally `ws://localhost:1420` and its http
//   equivalent, for `dx serve`'s hot-reload socket, scoped to that one
//   port and to loopback. Tauri only applies this when
//   `custom-protocol` is off, so it never reaches a shipped build.
//
// Neither object may carry an extra key: `tauri-build` validates the
// security schema and fails the build on an unknown field, so a comment
// key (the obvious way to annotate this in JSON) is not available.

fn main() {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let dist = manifest.join("ui/target/dx/wl-ui/release/web/public");
    assert!(
        dist.join("index.html").exists(),
        "frontend bundle missing at {} — build it first: cd crates/wl-app/ui && ../../scripts/dx.sh build --release",
        dist.display()
    );

    // `custom-protocol` off means this binary will load devUrl instead of
    // the embedded bundle. Warn at build time, where the message can
    // actually be read, instead of leaving a browser error page in the
    // window.
    if std::env::var_os("CARGO_FEATURE_CUSTOM_PROTOCOL").is_none() {
        println!(
            "cargo:warning=BUILT WITHOUT `custom-protocol`: this binary loads \
             http://localhost:1420 (tauri.conf.json devUrl), NOT the embedded \
             frontend. Running it now shows \"Could not connect to localhost\" in \
             the window. Use one of:\n  \
             * daily use:  cargo tauri build --no-bundle, then run \
             crates/wl-app/target/release/wl-app (embeds the bundle, no server needed)\n  \
             * dev:        cargo tauri dev  (starts `dx serve --port 1420` for you)\n  \
             * plain cargo build/run is only valid while that dev server is already up."
        );
    }

    println!("cargo:rerun-if-changed=tauri.conf.json");
    tauri_build::build()
}
