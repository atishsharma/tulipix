//! The Connect / Edit remote dialog — a config form generated from rclone's own
//! schema.
//!
//! `rclone config providers` prints every backend it supports and, for each,
//! every option: type, default, help, whether it is required, secret, advanced,
//! and which values it accepts. This module turns that into the two models the
//! dialog binds to (`cloud-backends`, `cloud-opts`) and turns what comes back
//! into a `rclone config create` / `config update`.
//!
//! Nothing here is a hand-kept list of backend names or option keys. rclone
//! gains a backend, the dialog offers it.

use slint::{ComponentHandle, Model, ModelRc, VecModel, SharedString};
use std::sync::{Mutex, MutexGuard, OnceLock};
use tulipix_cloud::providers::{self, Backend};
use tulipix_ui::*;

use crate::cloud;

/// The parsed schema, read once per process. rclone is not going to grow a
/// backend while the app is running, and the JSON is ~1 MB to re-parse.
fn catalogue() -> MutexGuard<'static, Vec<Backend>> {
    static C: OnceLock<Mutex<Vec<Backend>>> = OnceLock::new();
    C.get_or_init(Mutex::default).lock().unwrap_or_else(|e| e.into_inner())
}

/// The backend the form is currently editing, so option rebuilds do not have to
/// go back through the UI for it.
fn picked() -> MutexGuard<'static, String> {
    static P: OnceLock<Mutex<String>> = OnceLock::new();
    P.get_or_init(Mutex::default).lock().unwrap_or_else(|e| e.into_inner())
}

/// The token `rclone authorize` last handed back, held outside the form model.
///
/// rclone marks `token` as an *advanced* option on every OAuth backend, so with
/// the Advanced switch off — the default — there is no `token` row to write it
/// into and the whole browser dance would end in silence. Kept here and merged
/// back in at submit time.
fn authorized() -> MutexGuard<'static, String> {
    static T: OnceLock<Mutex<String>> = OnceLock::new();
    T.get_or_init(Mutex::default).lock().unwrap_or_else(|e| e.into_inner())
}

pub fn wire(window: &MainWindow) {
    let w = window.as_weak();
    window.on_cloud_connect_open_dialog(move || {
        let Some(w) = w.upgrade() else { return };
        reset(&w, false);
        load_catalogue(w.as_weak());
    });

    let w = window.as_weak();
    window.on_cloud_backend_search(move |q| {
        if let Some(w) = w.upgrade() {
            push_backends(&w, &q);
        }
    });

    let w = window.as_weak();
    window.on_cloud_backend_pick(move |name| {
        let Some(w) = w.upgrade() else { return };
        // A token belongs to the backend it was granted for — going back and
        // picking a different one must not carry it over.
        authorized().clear();
        w.set_cloud_authorized(false);
        *picked() = name.to_string();
        w.set_cloud_form_backend(name.clone());
        w.set_cloud_form_oauth(
            providers::find(&catalogue(), &name).map(Backend::is_oauth).unwrap_or(false),
        );
        w.set_cloud_connect_step(1);
        w.set_cloud_connect_error("".into());
        // A remote's name defaults to its backend — "drive", "s3". Something is
        // better than an empty required field, and it is the name most people
        // would have typed anyway.
        if w.get_cloud_form_name().is_empty() {
            w.set_cloud_form_name(name);
        }
        push_opts(&w);
    });

    // Only the gate option changes what the rest of the form looks like; every
    // other keystroke would rebuild the model out from under the box being typed
    // in.
    let w = window.as_weak();
    window.on_cloud_opt_changed(move |key| {
        if key == "provider"
            && let Some(w) = w.upgrade()
        {
            push_opts(&w);
        }
    });

    let w = window.as_weak();
    window.on_cloud_advanced_toggled(move || {
        if let Some(w) = w.upgrade() {
            push_opts(&w);
        }
    });

    let w = window.as_weak();
    window.on_cloud_authorize(move || {
        let Some(w) = w.upgrade() else { return };
        authorize(w.as_weak(), w.get_cloud_form_backend().to_string());
    });

    let w = window.as_weak();
    window.on_cloud_remote_edit(move |name| {
        let Some(w) = w.upgrade() else { return };
        reset(&w, true);
        w.set_cloud_form_name(name.clone());
        load_for_edit(w.as_weak(), name.to_string());
    });

    let w = window.as_weak();
    window.on_cloud_connect_cancel(move || {
        if let Some(w) = w.upgrade() {
            w.set_cloud_connect_open(false);
        }
    });

    let w = window.as_weak();
    window.on_cloud_connect_submit(move || {
        let Some(w) = w.upgrade() else { return };
        submit(&w);
    });
}

/// Blank the dialog and open it. `edit` skips straight to the form — there is
/// no backend to pick when the remote already has one.
fn reset(w: &MainWindow, edit: bool) {
    w.set_cloud_form_name("".into());
    w.set_cloud_backend_query("".into());
    w.set_cloud_show_advanced(false);
    w.set_cloud_connect_error("".into());
    w.set_cloud_connect_busy(false);
    w.set_cloud_authorizing(false);
    w.set_cloud_authorized(false);
    authorized().clear();
    w.set_cloud_connect_edit(edit);
    w.set_cloud_connect_step(if edit { 1 } else { 0 });
    w.set_cloud_opts(ModelRc::new(VecModel::<CloudOpt>::default()));
    w.set_cloud_connect_open(true);
}

// ── the catalogue ───────────────────────────────────────────────────────────

/// Read `rclone config providers` once and fill the backend list.
fn load_catalogue(weak: slint::Weak<MainWindow>) {
    // Already parsed — nothing to wait for.
    if !catalogue().is_empty() {
        let _ = weak.upgrade_in_event_loop(|w| push_backends(&w, ""));
        return;
    }
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        tracing::warn!("cloud: no tokio runtime; the backend catalogue is unavailable");
        return;
    };
    handle.spawn(async move {
        let json = tokio::task::spawn_blocking(|| cloud::run(&providers::providers_args()))
            .await
            .ok()
            .and_then(|r| r.ok())
            .unwrap_or_default();
        let parsed = providers::parse_providers(&json);
        if parsed.is_empty() {
            tracing::warn!("cloud: rclone reported no backends");
        }
        *catalogue() = parsed;
        let _ = weak.upgrade_in_event_loop(|w| push_backends(&w, ""));
    });
}

/// Backends matching `filter`, name first then description. rclone's own order
/// is neither alphabetical nor meaningful, so it is sorted here.
fn push_backends(w: &MainWindow, filter: &str) {
    let q = filter.trim().to_lowercase();
    let cat = catalogue();
    let mut hits: Vec<&Backend> = cat
        .iter()
        .filter(|b| {
            q.is_empty()
                || b.name.to_lowercase().contains(&q)
                || b.description.to_lowercase().contains(&q)
        })
        .collect();
    // An exact name match belongs at the top: typing "s3" should not bury it
    // under every backend whose description mentions S3 compatibility.
    hits.sort_by(|a, b| {
        let rank = |x: &Backend| if x.name.to_lowercase() == q { 0 } else { 1 };
        rank(a).cmp(&rank(b)).then_with(|| a.name.cmp(&b.name))
    });
    let rows: Vec<CloudBackend> = hits
        .into_iter()
        .map(|b| CloudBackend {
            name: b.name.clone().into(),
            desc: b.description.clone().into(),
            oauth: b.is_oauth(),
        })
        .collect();
    w.set_cloud_backends(ModelRc::new(VecModel::from(rows)));
}

// ── the form ────────────────────────────────────────────────────────────────

/// Rebuild `cloud-opts` for the picked backend, keeping whatever has already
/// been typed.
///
/// Called on every event that changes which options apply: picking a backend,
/// toggling Advanced, and changing the `provider` gate. Values survive because
/// the rebuild reads the live model before replacing it — otherwise switching
/// Advanced on would wipe the form.
fn push_opts(w: &MainWindow) {
    let backend = picked().clone();
    let cat = catalogue();
    let Some(b) = providers::find(&cat, &backend) else {
        w.set_cloud_opts(ModelRc::new(VecModel::<CloudOpt>::default()));
        return;
    };

    let entered = current_values(w);
    let provider = entered
        .iter()
        .find(|(k, _)| k == "provider")
        .map(|(_, v)| v.clone())
        .unwrap_or_default();

    let rows: Vec<CloudOpt> = providers::visible(b, &provider, w.get_cloud_show_advanced())
        .into_iter()
        .map(|o| {
            let value = entered
                .iter()
                .find(|(k, _)| *k == o.name)
                .map(|(_, v)| v.clone())
                .unwrap_or_default();
            // A dropdown whose value is not one of the suggestions must open as
            // a text box, or an edited remote's custom endpoint would silently
            // read as "Choose…".
            let off_list = !value.is_empty() && !o.examples.iter().any(|(v, _)| *v == value);
            CloudOpt {
                key: o.name.clone().into(),
                label: o.label().into(),
                help: o.help.clone().into(),
                kind: o.widget().into(),
                value: value.into(),
                placeholder: o.default.clone().into(),
                required: o.required,
                free: !o.exclusive && !o.examples.is_empty(),
                custom: off_list,
                choices: ModelRc::new(VecModel::from(
                    o.examples
                        .iter()
                        .map(|(v, help)| {
                            SharedString::from(if help.is_empty() {
                                v.clone()
                            } else {
                                format!("{v} — {help}")
                            })
                        })
                        .collect::<Vec<_>>(),
                )),
                choice_values: ModelRc::new(VecModel::from(
                    o.examples.iter().map(|(v, _)| SharedString::from(v.clone())).collect::<Vec<_>>(),
                )),
            }
        })
        .collect();
    w.set_cloud_opts(ModelRc::new(VecModel::from(rows)));
}

/// Everything the form currently holds, as `(key, value)`.
fn current_values(w: &MainWindow) -> Vec<(String, String)> {
    w.get_cloud_opts()
        .iter()
        .map(|o| (o.key.to_string(), o.value.to_string()))
        .collect()
}

/// Seed the form from a remote that already exists.
///
/// Its backend and stored keys come from `rclone config dump`; anything it was
/// configured with that is not in the basic set turns Advanced on, so an
/// existing value is never editable-in-theory-but-invisible.
fn load_for_edit(weak: slint::Weak<MainWindow>, name: String) {
    let Ok(handle) = tokio::runtime::Handle::try_current() else { return };
    handle.spawn(async move {
        // The catalogue has to be in hand before the form can be built from it.
        // The emptiness check is its own statement so no lock is live while the
        // subprocess runs.
        let need_catalogue = catalogue().is_empty();
        if need_catalogue {
            let json = tokio::task::spawn_blocking(|| cloud::run(&providers::providers_args()))
                .await
                .ok()
                .and_then(|r| r.ok())
                .unwrap_or_default();
            *catalogue() = providers::parse_providers(&json);
        }
        let dump = tokio::task::spawn_blocking(|| {
            cloud::run(&tulipix_cloud::remotes::dump_args())
        })
        .await
        .ok()
        .and_then(|r| r.ok())
        .unwrap_or_default();

        let Some((backend, stored)) = tulipix_cloud::remotes::parse_dump_one(&dump, &name) else {
            let _ = weak.upgrade_in_event_loop(move |w| {
                w.set_cloud_connect_error(format!("'{name}' is not in the rclone config").into());
            });
            return;
        };

        let advanced = {
            let cat = catalogue();
            providers::find(&cat, &backend)
                .map(|b| {
                    b.options
                        .iter()
                        .any(|o| o.advanced && stored.contains_key(&o.name))
                })
                .unwrap_or(false)
        };
        let oauth = providers::find(&catalogue(), &backend).map(Backend::is_oauth).unwrap_or(false);

        let _ = weak.upgrade_in_event_loop(move |w| {
            *picked() = backend.clone();
            w.set_cloud_form_backend(backend.into());
            w.set_cloud_form_oauth(oauth);
            w.set_cloud_show_advanced(advanced);
            // Build the empty form first, then write the stored values into it
            // and rebuild — the second pass is what applies the `provider` gate
            // to the values that were just loaded.
            push_opts(&w);
            let opts = w.get_cloud_opts();
            for i in 0..opts.row_count() {
                if let Some(mut row) = opts.row_data(i)
                    && let Some(v) = stored.get(row.key.as_str())
                {
                    row.value = v.clone().into();
                    opts.set_row_data(i, row);
                }
            }
            push_opts(&w);
        });
    });
}

// ── OAuth ───────────────────────────────────────────────────────────────────

/// `rclone authorize <backend>` — opens the browser, waits for the redirect,
/// prints a token. Can sit there for as long as the user takes, which is why it
/// runs on a blocking thread and the button says so.
fn authorize(weak: slint::Weak<MainWindow>, backend: String) {
    let Ok(handle) = tokio::runtime::Handle::try_current() else { return };
    let _ = weak.upgrade_in_event_loop(|w| {
        w.set_cloud_authorizing(true);
        w.set_cloud_connect_error("".into());
    });
    handle.spawn(async move {
        let out = tokio::task::spawn_blocking(move || {
            cloud::run(&providers::authorize_args(&backend))
        })
        .await
        .unwrap_or_else(|e| Err(anyhow::anyhow!(e)));
        // Reduced to `Result<String, String>` before crossing into the event
        // loop: the token has to be lifted out of rclone's prose, and the
        // failure has to survive as a sentence the dialog can show.
        let outcome: Result<String, String> = match out {
            Ok(text) => providers::parse_authorize(&text).ok_or_else(|| {
                "rclone did not return a token — the browser flow was not completed.".to_string()
            }),
            Err(e) => Err(format!("Authorize failed: {e}")),
        };
        let _ = weak.upgrade_in_event_loop(move |w| {
            w.set_cloud_authorizing(false);
            match outcome {
                Ok(token) => {
                    // Written to both: the row only exists with Advanced on, and
                    // the stash is what `submit` falls back to when it does not.
                    set_opt(&w, "token", &token);
                    *authorized() = token;
                    w.set_cloud_authorized(true);
                }
                Err(msg) => w.set_cloud_connect_error(msg.into()),
            }
        });
    });
}

/// Write one option's value into the live form model.
fn set_opt(w: &MainWindow, key: &str, value: &str) {
    let opts = w.get_cloud_opts();
    for i in 0..opts.row_count() {
        if let Some(mut row) = opts.row_data(i)
            && row.key == key
        {
            row.value = value.into();
            opts.set_row_data(i, row);
            return;
        }
    }
}

// ── submit ──────────────────────────────────────────────────────────────────

fn submit(w: &MainWindow) {
    let name = w.get_cloud_form_name().trim().to_string();
    let backend = w.get_cloud_form_backend().to_string();
    if name.is_empty() {
        w.set_cloud_connect_error("Give the remote a name.".into());
        return;
    }
    // rclone remote names go into `remote:path`, so a colon in one makes every
    // path built from it ambiguous.
    if name.contains(':') || name.contains('/') {
        w.set_cloud_connect_error("A remote's name cannot contain ':' or '/'.".into());
        return;
    }
    if backend.is_empty() {
        w.set_cloud_connect_error("Pick a backend.".into());
        return;
    }

    // Blank means "leave it at rclone's default", so blanks are dropped rather
    // than written as empty strings — writing them would pin the option to ""
    // and override the default it was trying to keep.
    let mut pairs: Vec<(String, String)> = w
        .get_cloud_opts()
        .iter()
        .filter(|o| !o.value.trim().is_empty())
        .map(|o| (o.key.to_string(), o.value.trim().to_string()))
        .collect();

    // The token from the browser dance, when the form had no visible row to
    // hold it. A value typed into that row with Advanced on wins.
    let token = authorized().clone();
    if !token.is_empty() && !pairs.iter().any(|(k, _)| k == "token") {
        pairs.push(("token".into(), token));
    }

    if let Some(missing) = w
        .get_cloud_opts()
        .iter()
        .find(|o| o.required && o.value.trim().is_empty())
    {
        w.set_cloud_connect_error(format!("{} is required.", missing.label).into());
        return;
    }

    let edit = w.get_cloud_connect_edit();
    w.set_cloud_connect_busy(true);
    w.set_cloud_connect_error("".into());

    let Ok(handle) = tokio::runtime::Handle::try_current() else { return };
    let weak = w.as_weak();
    handle.spawn(async move {
        let name_c = name.clone();
        let res = tokio::task::spawn_blocking(move || {
            let refs: Vec<(&str, &str)> =
                pairs.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
            let args = if edit {
                providers::update_args(&name_c, &backend, &refs)
            } else {
                tulipix_cloud::remotes::create_args(&name_c, &backend, &refs)
            };
            cloud::run(&args)
        })
        .await
        .unwrap_or_else(|e| Err(anyhow::anyhow!(e)));

        let _ = weak.upgrade_in_event_loop(move |w| {
            w.set_cloud_connect_busy(false);
            match res {
                Ok(_) => {
                    w.set_cloud_connect_open(false);
                    w.set_cloud_status(
                        format!("{} '{name}'", if edit { "Updated" } else { "Connected" }).into(),
                    );
                    crate::cloud_refresh_remotes(w.as_weak());
                }
                Err(e) => w.set_cloud_connect_error(format!("{e}").into()),
            }
        });
    });
}
