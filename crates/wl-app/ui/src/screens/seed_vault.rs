//! BIP-39 Seed Vault onboarding (skill §4.D, US-1).
//!
//! Fresh install: generate + display the 12 words with the
//! uncompromising mandate; then a 3-word verification challenge.

use dioxus::prelude::*;

use crate::app::{flash, invoke, AppCtx, Screen};

#[derive(Clone, Default, serde::Deserialize)]
pub struct GeneratedIdentity {
    pub phrase: String,
    pub account_id: String,
    #[serde(default)]
    pub verify_indices: Vec<usize>,
}

#[component]
pub fn SeedVaultScreen(phrase: Vec<String>, verify_indices: Vec<usize>, restore: bool) -> Element {
    let ctx = use_context::<AppCtx>();
    let generated = use_signal::<Option<Vec<String>>>(|| None);
    let gen_indices = use_signal(Vec::<usize>::new);
    let mut mode = use_signal(|| {
        if restore {
            "restore"
        } else if phrase.is_empty() {
            "intro"
        } else {
            "display"
        }
    });
    let mut restore_input = use_signal(String::new);
    let mut words_input = use_signal::<Vec<String>>(|| vec![String::new(); 3]);
    let mut error = use_signal(String::new);
    let mut busy = use_signal(|| false);

    let seeded_phrase: Vec<String> = if phrase.is_empty() {
        generated.read().clone().unwrap_or_default()
    } else {
        phrase.clone()
    };

    // Kick off generation on first render (intro → display).
    use_effect(move || {
        let cur = mode.read().clone();
        let phrase_empty = phrase.is_empty();
        let mut g = generated;
        let mut m = mode;
        spawn(async move {
            if cur == "intro" && phrase_empty && g.read().is_none() {
                match invoke::<GeneratedIdentity>("identity_generate", ()).await {
                    Ok(id) => {
                        let words: Vec<String> =
                            id.phrase.split_whitespace().map(String::from).collect();
                        *g.write() = Some(words);
                        let mut gi = gen_indices;
                        *gi.write() = id.verify_indices;
                        m.set("display");
                    }
                    Err(e) => flash(&ctx, &format!("ERR {e}")),
                }
            }
        });
    });

    // Challenge positions: server-issued random indices first, then
    // props, then the legacy deterministic fallback (Item 6).
    let gi = gen_indices.read().clone();
    let challenge: Vec<usize> = if !gi.is_empty() {
        gi
    } else if verify_indices.is_empty() {
        vec![2, 6, 10]
    } else {
        verify_indices.clone()
    };

    let verify_phrase = seeded_phrase.clone();
    let verify_challenge = challenge.clone();
    let verify = move || {
        let ctx = ctx;
        let words: Vec<String> = words_input.read().clone();
        let mut words_sig = words_input;
        let ch = verify_challenge.clone();
        let mut g = generated;
        let seeded = verify_phrase.clone();
        let mut error = error;
        spawn(async move {
            let full: Vec<String> = if seeded.is_empty() {
                g.read().clone().unwrap_or_default()
            } else {
                seeded
            };
            // Local pre-check (shell re-verifies authoritatively).
            let ok = ch
                .iter()
                .zip(words.iter())
                .all(|(&idx, w)| match full.get(idx) {
                    Some(word) => w.trim().eq_ignore_ascii_case(word),
                    None => false,
                });
            if !ok {
                error.set(
                    "Those words don't match the sequence. Check your backup again.".to_string(),
                );
                return;
            }
            #[derive(serde::Serialize)]
            struct V {
                indices: Vec<usize>,
                words: Vec<String>,
            }
            let payload = V {
                indices: ch.clone(),
                words: words.clone(),
            };
            let verified: bool = invoke::<bool>("identity_verify_backup", payload)
                .await
                .unwrap_or(false);
            if verified {
                // Secret hygiene: the 12 words lived in signals/DOM for
                // the one mandated display — drop them the moment the
                // shell confirms the backup (AGENTS.md: secrets never
                // linger in the DOM).
                g.set(None);
                words_sig.set(vec![String::new(); 3]);
                let mut s = ctx.screen;
                *s.write() = Screen::ByokSetup;
            } else {
                error.set(
                    "Those words don't match the sequence. Check your backup again.".to_string(),
                );
            }
        });
    };

    let seeded_phrase = seeded_phrase.clone();
    let challenge = challenge.clone();

    rsx! {
        div { class: "wl-scroll-region",
            section { class: "wl-seed-vault",
                div { class: "wl-seed-header",
                    h2 { class: "wl-serif-title",
                        "Secret " span { class: "wl-italic-accent", "recovery" } " phrase"
                    }
                    p { class: "wl-seed-sub",
                        "Write these down in exact sequence. Worldline has no email servers to recover them. If you lose these 12 words, your data cannot be recovered."
                    }
                }

                if *mode.read() == "restore" {
                    p { class: "wl-seed-sub",
                        "Enter your existing 12-word phrase to unlock this device. Words are checked locally; only the derived public key ever leaves."
                    }
                    textarea {
                        class: "wl-textarea",
                        rows: "3",
                        placeholder: "twelve words separated by spaces",
                        autocomplete: "off",
                        autocapitalize: "off",
                        spellcheck: "false",
                        value: "{restore_input.read().clone()}",
                        oninput: move |e| restore_input.set(e.value()),
                    }
                    if !error.read().is_empty() {
                        p { class: "wl-seed-sub", style: "color: var(--wl-accent-coral);", "{error.read().clone()}" }
                    }
                    button {
                        class: "wl-btn-primary",
                        style: "margin-top: 8px;",
                        onclick: move |_| {
                            let ctx = ctx;
                            let phrase = restore_input.read().clone();
                            spawn(async move {
                                match invoke::<String>(
                                    "identity_restore",
                                    serde_json::json!({ "phrase": phrase }),
                                )
                                .await
                                {
                                    Ok(_) => {
                                        // Secret hygiene: drop the typed
                                        // phrase from the signal/DOM now
                                        // that the shell holds it.
                                        restore_input.set(String::new());
                                        let mut s = ctx.screen;
                                        *s.write() = Screen::ByokSetup;
                                    }
                                    Err(_) => {
                                        let mut e = error;
                                        e.set("That phrase was not accepted. Check each word and try again.".to_string());
                                    }
                                }
                            });
                        },
                        "Restore identity"
                    }
                    button {
                        class: "wl-btn-escape",
                        style: "margin-top: 6px;",
                        onclick: move |_| { mode.set("intro"); },
                        "Create a new identity instead"
                    }
                } else if *mode.read() == "intro" {
                    div { class: "wl-directive-card",
                        p { class: "wl-body-muted", "Generating cryptographic identity from OS entropy…" }
                    }
                    button {
                        class: "wl-btn-escape",
                        style: "margin-top: 6px;",
                        onclick: move |_| { mode.set("restore"); },
                        "Already have 12 words? Restore"
                    }
                } else if *mode.read() == "display" {
                    div { class: "wl-seed-grid",
                        for (i, word) in seeded_phrase.iter().enumerate() {
                            div { class: "wl-seed-token", key: "{i}",
                                span { class: "wl-seed-num", "{i + 1:02}" }
                                "{word}"
                            }
                        }
                    }
                    button {
                        class: "wl-btn-primary",
                        style: "margin-top: 8px;",
                        onclick: move |_| { mode.set("verify"); },
                        "I saved the words"
                    }
                } else {
                    p { class: "wl-seed-sub",
                        "Verification challenge — type the highlighted words exactly as written."
                    }
                    for (row, &idx) in challenge.iter().enumerate() {
                        input {
                            class: "wl-verify-slot",
                            key: "verify-{row}",
                            r#type: "text",
                            placeholder: "Word {idx + 1:02}",
                            autocomplete: "off",
                            value: "{words_input.read()[row].clone()}",
                            oninput: move |e| {
                                let mut v = words_input.read().clone();
                                v[row] = e.value();
                                words_input.set(v);
                            },
                        }
                    }
                    if !error.read().is_empty() {
                        p { class: "wl-seed-sub", style: "color: var(--wl-accent-coral);", "{error.read().clone()}" }
                    }
                    button { class: "wl-btn-primary", onclick: move |_| verify(), "Verify backup" }
                }
            }
        }
    }
}
