//! Google Takeout subscription import. Parses both the `subscriptions.csv` and
//! `subscriptions.json` export shapes into a deduped `(channel_id, title)` list.

use anyhow::{bail, Result};
use serde::Deserialize;

#[derive(Debug, Clone, PartialEq)]
pub struct ImportedSub {
    pub channel_id: String,
    pub title: String,
}

#[derive(Deserialize)]
struct JsonSub {
    snippet: JsonSnippet,
}
#[derive(Deserialize)]
struct JsonSnippet {
    #[serde(default)]
    title: String,
    #[serde(rename = "resourceId")]
    resource_id: JsonRes,
}
#[derive(Deserialize)]
struct JsonRes {
    #[serde(rename = "channelId", default)]
    channel_id: String,
}

/// Split a single CSV line honoring `"..."` quoting (doubled `""` → `"`).
fn split_csv_line(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_q = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if in_q && chars.peek() == Some(&'"') => {
                cur.push('"');
                chars.next();
            }
            '"' => in_q = !in_q,
            ',' if !in_q => {
                out.push(std::mem::take(&mut cur));
            }
            _ => cur.push(c),
        }
    }
    out.push(cur);
    out
}

fn parse_csv(text: &str) -> Result<Vec<ImportedSub>> {
    let mut lines = text.lines();
    let header = lines.next().unwrap_or("");
    if !header.to_lowercase().contains("channel id") {
        bail!("not a Takeout subscriptions CSV (missing 'Channel Id' header)");
    }
    let mut out = Vec::new();
    for line in lines {
        if line.trim().is_empty() {
            continue;
        }
        let cols = split_csv_line(line);
        if cols.len() < 3 {
            continue;
        }
        let id = cols[0].trim().to_string();
        let title = cols[2].trim().to_string();
        if !id.is_empty() {
            out.push(ImportedSub { channel_id: id, title });
        }
    }
    Ok(out)
}

fn parse_json(text: &str) -> Result<Vec<ImportedSub>> {
    let raw: Vec<JsonSub> = serde_json::from_str(text)?;
    Ok(raw
        .into_iter()
        .filter(|s| !s.snippet.resource_id.channel_id.is_empty())
        .map(|s| ImportedSub {
            channel_id: s.snippet.resource_id.channel_id,
            title: s.snippet.title,
        })
        .collect())
}

/// Parse either Takeout shape (auto-detected) and dedup by channel id,
/// preserving first-seen order.
pub fn parse(text: &str) -> Result<Vec<ImportedSub>> {
    let t = text.trim_start();
    let parsed = if t.starts_with('[') {
        parse_json(text)?
    } else {
        parse_csv(text)?
    };
    let mut seen = std::collections::HashSet::new();
    Ok(parsed
        .into_iter()
        .filter(|s| seen.insert(s.channel_id.clone()))
        .collect())
}

/// Parse a Takeout playlist CSV (first column = Video ID) into a video-id list.
/// Header row is skipped; blank lines and obviously-non-id rows are ignored.
pub fn parse_playlist_ids(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for (i, line) in text.lines().enumerate() {
        let first = line.split(',').next().unwrap_or("").trim();
        if i == 0 && first.to_lowercase().contains("video id") { continue; }
        if first.is_empty() { continue; }
        // YouTube ids are 11 url-safe chars.
        if first.len() == 11 && first.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
            if seen.insert(first.to_string()) { out.push(first.to_string()); }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_playlist_ids() {
        let csv = "Video ID,Playlist video creation timestamp\n\
                   dQw4w9WgXcQ,2020-01-01\n\
                   abc123ABC_-,2020-01-02\n\
                   ,bad\n";
        let ids = parse_playlist_ids(csv);
        assert_eq!(ids, vec!["dQw4w9WgXcQ".to_string(), "abc123ABC_-".to_string()]);
    }


    #[test]
    fn parses_csv() {
        let csv = "Channel Id,Channel Url,Channel Title\n\
                   UC123,http://youtube.com/channel/UC123,Tame Impala\n\
                   UC456,http://youtube.com/channel/UC456,NPR Music\n";
        let subs = parse(csv).unwrap();
        assert_eq!(subs.len(), 2);
        assert_eq!(
            subs[0],
            ImportedSub { channel_id: "UC123".into(), title: "Tame Impala".into() }
        );
        assert_eq!(subs[1].title, "NPR Music");
    }

    #[test]
    fn parses_csv_with_quoted_commas() {
        let csv = "Channel Id,Channel Url,Channel Title\n\
                   UC9,http://x,\"Doe, John & Co\"\n";
        let subs = parse(csv).unwrap();
        assert_eq!(subs[0].title, "Doe, John & Co");
    }

    #[test]
    fn parses_json() {
        let json = r#"[
          {"snippet":{"title":"Tame Impala","resourceId":{"channelId":"UC123"}}},
          {"snippet":{"title":"NPR Music","resourceId":{"channelId":"UC456"}}}
        ]"#;
        let subs = parse(json).unwrap();
        assert_eq!(subs.len(), 2);
        assert_eq!(subs[1].channel_id, "UC456");
    }

    #[test]
    fn dedups_by_channel_id() {
        let csv = "Channel Id,Channel Url,Channel Title\n\
                   UC1,u,A\nUC1,u,A dup\nUC2,u,B\n";
        let subs = parse(csv).unwrap();
        assert_eq!(subs.len(), 2);
    }

    #[test]
    fn empty_or_garbage_errors() {
        assert!(parse("not real, no header rows").is_err() || parse("").unwrap().is_empty());
    }
}
