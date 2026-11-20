//! Desktop entry point. Android loads the shared library directly.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    wl_app_lib::run();
}
