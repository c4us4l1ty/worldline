//! Theme controller — drives the `data-theme` attribute on <html>.

use dioxus::prelude::*;

#[derive(Clone, Copy)]
pub struct Theme {
    pub mode: Signal<String>,
}

impl Default for Theme {
    fn default() -> Self {
        Self::new()
    }
}

impl Theme {
    pub fn new() -> Self {
        // Default: dark matte graphite (skill §1).
        Self {
            mode: Signal::new("dark".to_string()),
        }
    }

    /// Applies the theme to the document root element.
    pub fn set(&self, mode: String) {
        apply_data_theme(if mode == "light" { "light" } else { "dark" });
        let mut m = self.mode;
        *m.write() = mode;
    }
}

pub fn apply_data_theme(value: &str) {
    use wasm_bindgen::prelude::*;
    #[wasm_bindgen(inline_js = r#"
    export function set_theme(v) {
      try { document.documentElement.setAttribute("data-theme", v); } catch (e) {}
    }
  "#)]
    extern "C" {
        fn set_theme(v: String);
    }
    set_theme(value.to_string());
}
