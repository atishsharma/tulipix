//! The Servers modal: host list and request-signing key.

use super::*;

/// Fill the editor with the configured hosts, one per line — and the current
/// signing key, since both live in the same modal and open together.
pub fn stream_hosts_load(weak: slint::Weak<MainWindow>) {
    let text = stream::hosts::load().join("\n");
    let key = stream::sign_key::load();
    let _ = weak.upgrade_in_event_loop(move |w| {
        w.set_video_stream_hosts_text(text.into());
        w.set_video_stream_hosts_error("".into());
        w.set_video_stream_key_text(key.into());
        w.set_video_stream_key_error("".into());
    });
}

/// Validate and persist the signing key. On success the running signer switches
/// to it and the client is rebuilt so `init()` re-runs under the new key.
pub fn stream_key_save(weak: slint::Weak<MainWindow>, key: String) {
    match stream::sign_key::save(&key) {
        Ok(saved) => {
            stream_invalidate_client();
            let _ = weak.upgrade_in_event_loop(move |w| {
                w.set_video_stream_key_text(saved.into());
                w.set_video_stream_key_error("".into());
                w.set_video_stream_key_saved(true);
                w.set_video_stream_status("Signing key updated.".into());
            });
        }
        Err(msg) => {
            let _ = weak.upgrade_in_event_loop(move |w| w.set_video_stream_key_error(msg.into()));
        }
    }
}

/// Drop the built-in default key into the editor. Not persisted until Save.
pub fn stream_key_reset(weak: slint::Weak<MainWindow>) {
    let text = stream::sign_key::default();
    let _ = weak.upgrade_in_event_loop(move |w| {
        w.set_video_stream_key_text(text.into());
        w.set_video_stream_key_error("Press Save to apply the default key.".into());
    });
}

/// Validate and persist the edited host list. A rejected entry leaves the
/// stored list untouched and reports why.
pub fn stream_hosts_save(weak: slint::Weak<MainWindow>, text: String) {
    let entries: Vec<String> = text.lines().map(|l| l.to_string()).collect();
    match stream::hosts::save(&entries) {
        Ok(saved) => {
            stream_invalidate_client();
            let joined = saved.join("\n");
            let n = saved.len();
            let _ = weak.upgrade_in_event_loop(move |w| {
                w.set_video_stream_hosts_text(joined.into());
                w.set_video_stream_hosts_error("".into());
                // Keep the popup open and flash the "Applied" pill on Save so the
                // user sees the change landed; the UI auto-clears the flag.
                w.set_video_stream_hosts_saved(true);
                w.set_video_stream_status(
                    format!("Saved {n} server{}.", if n == 1 { "" } else { "s" }).into(),
                );
            });
        }
        Err(msg) => {
            let _ = weak.upgrade_in_event_loop(move |w| w.set_video_stream_hosts_error(msg.into()));
        }
    }
}

/// Time every host in the editor and report which ones answer.
///
/// Reads the text box, not the saved list, so an edit can be tested before it is
/// committed. Nothing is written — the user decides whether to reorder.
pub fn stream_hosts_check(weak: slint::Weak<MainWindow>, text: String) {
    let hosts: Vec<String> =
        text.lines().filter_map(|l| stream::hosts::validate(l).ok()).collect();
    if hosts.is_empty() {
        let _ = weak.upgrade_in_event_loop(|w| {
            w.set_video_stream_hosts_error("Nothing to test — add a server first.".into())
        });
        return;
    }
    let _ = weak.upgrade_in_event_loop(|w| {
        w.set_video_stream_hosts_checking(true);
        w.set_video_stream_hosts_error("".into());
    });

    tokio::runtime::Handle::current().spawn(async move {
        // A dead host is only known to be dead once it has timed out, so this is
        // as slow as the slowest entry — hence its own busy flag.
        let results = stream::probe::probe_all(&hosts).await;
        let summary = stream::probe::summary(&results);
        let ranked = stream::probe::rank(&results).join("\n");
        let rows: Vec<(String, String, bool)> = results
            .iter()
            .map(|r| (short_host(&r.host), r.label(), r.ok))
            .collect();

        let _ = weak.upgrade_in_event_loop(move |w| {
            let model: Vec<StreamHostHealth> = rows
                .into_iter()
                .map(|(host, note, ok)| StreamHostHealth {
                    host: host.into(),
                    note: note.into(),
                    ok,
                })
                .collect();
            w.set_video_stream_host_health(slint::ModelRc::new(slint::VecModel::from(model)));
            w.set_video_stream_hosts_summary(summary.into());
            // Offered, not applied: Save is still the only thing that writes.
            w.set_video_stream_hosts_ranked(ranked.into());
            w.set_video_stream_hosts_checking(false);
        });
    });
}

/// "https://api5.aoneroom.com" → "api5.aoneroom.com", for a narrow row.
fn short_host(host: &str) -> String {
    host.split_once("://").map(|(_, rest)| rest).unwrap_or(host).to_string()
}

/// Put the built-in list back into the editor. Not persisted until Save, so a
/// misclick is one Escape away from being harmless.
pub fn stream_hosts_reset(weak: slint::Weak<MainWindow>) {
    let text = stream::hosts::defaults().join("\n");
    let _ = weak.upgrade_in_event_loop(move |w| {
        w.set_video_stream_hosts_text(text.into());
        w.set_video_stream_hosts_error("Press Save to apply the default servers.".into());
    });
}
