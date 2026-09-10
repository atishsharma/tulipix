//! The chat assistant overlay (Ctrl+J).
//!
//! The overlay is wired; the engine behind it is not, in this build or in the
//! Slint one. `tulipix-ai` carries the llm bindings but nothing links them to
//! a model, so the honest answer is the one the Slint build already gives --
//! and giving a different one here would make the port look like it grew a
//! feature it does not have.
//!
//! State lives in Dart: a turn list is a list, and round-tripping it through
//! the bridge on every message would buy nothing.

use crate::api::shell::load;

/// One line of the conversation. `role` is "user" or "assistant".
pub struct ChatTurn {
    pub role: String,
    pub content: String,
}

/// The greeting the overlay opens on. `sync` because it is a constant: making
/// the widget's `initState` await a Future to learn a fixed string would put a
/// frame of empty transcript in front of every open.
#[flutter_rust_bridge::frb(sync)]
pub fn chat_greeting() -> ChatTurn {
    ChatTurn {
        role: "assistant".into(),
        content: "Hi! Once a model is configured in Settings → AI Models I can search \
                  and act on your library."
            .into(),
    }
}

/// What the assistant answers. Blank input is not a turn -- an accidental
/// Enter in an empty box should not put an error in the transcript.
pub fn chat_reply(question: String) -> Option<ChatTurn> {
    if question.trim().is_empty() {
        return None;
    }
    let on = load().flag("ai.chat", false);
    tracing::info!(q = %question, enabled = on, "chat submitted");
    Some(ChatTurn {
        role: "assistant".into(),
        content: if on {
            "The chat engine isn't bundled in this build yet — your message was logged.".into()
        } else {
            "AI chat is disabled. Enable it in Settings → AI Models.".into()
        },
    })
}
