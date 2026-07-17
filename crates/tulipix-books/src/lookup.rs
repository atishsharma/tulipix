//! Word lookup for the reader's "Define" popup (R2): dictionary definition,
//! translation, and a Wikipedia link. Best-effort over free, key-less public
//! APIs — any part that fails just comes back empty, never an error the reader
//! has to handle.
//!
//! - Definition : dictionaryapi.dev (Free Dictionary API)
//! - Translation: MyMemory (api.mymemory.translated.net)
//! - Wikipedia  : REST summary endpoint (en.wikipedia.org/api/rest_v1)

use serde_json::Value;

/// One word lookup result. Any field may be empty when that source had nothing.
#[derive(Debug, Clone, Default)]
pub struct Lookup {
    pub word: String,
    pub definition: String,
    pub part_of_speech: String,
    pub translation: String,
    pub wiki_extract: String,
    pub wiki_url: String,
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent("Tulipix/1.0 (books reader)")
        .timeout(std::time::Duration::from_secs(8))
        .build()
        .unwrap_or_default()
}

/// Look a word up. `target_lang` is a 2-letter code (e.g. "es", "fr", "hi");
/// empty skips translation. Runs the three fetches concurrently.
pub async fn lookup(word: &str, target_lang: &str) -> Lookup {
    let word = word.trim();
    let mut out = Lookup { word: word.to_string(), ..Default::default() };
    if word.is_empty() {
        return out;
    }
    let c = client();
    // Sequential (best-effort; keeps the crate off tokio's "macros" feature).
    let def = definition(&c, word).await;
    let tr = translation(&c, word, target_lang).await;
    let wiki = wikipedia(&c, word).await;
    if let Some((d, pos)) = def {
        out.definition = d;
        out.part_of_speech = pos;
    }
    out.translation = tr.unwrap_or_default();
    if let Some((extract, url)) = wiki {
        out.wiki_extract = extract;
        out.wiki_url = url;
    }
    out
}

/// First definition + part of speech from the Free Dictionary API.
async fn definition(c: &reqwest::Client, word: &str) -> Option<(String, String)> {
    let url = format!(
        "https://api.dictionaryapi.dev/api/v2/entries/en/{}",
        urlencoding::encode(word)
    );
    let v: Value = c.get(url).send().await.ok()?.json().await.ok()?;
    let meaning = v.get(0)?.get("meanings")?.get(0)?;
    let pos = meaning.get("partOfSpeech").and_then(|p| p.as_str()).unwrap_or("").to_string();
    let def = meaning
        .get("definitions")?
        .get(0)?
        .get("definition")?
        .as_str()?
        .to_string();
    Some((def, pos))
}

/// Translation via MyMemory (no key). `en` → `target_lang`.
async fn translation(c: &reqwest::Client, word: &str, target_lang: &str) -> Option<String> {
    if target_lang.is_empty() {
        return None;
    }
    let url = format!(
        "https://api.mymemory.translated.net/get?q={}&langpair=en|{}",
        urlencoding::encode(word),
        urlencoding::encode(target_lang)
    );
    let v: Value = c.get(url).send().await.ok()?.json().await.ok()?;
    let t = v.get("responseData")?.get("translatedText")?.as_str()?.trim().to_string();
    (!t.is_empty()).then_some(t)
}

/// Wikipedia summary (extract + canonical article URL).
async fn wikipedia(c: &reqwest::Client, word: &str) -> Option<(String, String)> {
    let url = format!(
        "https://en.wikipedia.org/api/rest_v1/page/summary/{}",
        urlencoding::encode(word)
    );
    let v: Value = c.get(url).send().await.ok()?.json().await.ok()?;
    // Disambiguation / missing pages have no useful extract.
    let extract = v.get("extract").and_then(|e| e.as_str()).unwrap_or("").to_string();
    let page = v
        .get("content_urls")
        .and_then(|u| u.get("desktop"))
        .and_then(|d| d.get("page"))
        .and_then(|p| p.as_str())
        .unwrap_or("")
        .to_string();
    if extract.is_empty() && page.is_empty() {
        return None;
    }
    Some((extract, page))
}
