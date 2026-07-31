//! The rclone backend catalogue — what the Connect dialog builds its form from.
//!
//! `rclone config providers` prints its own schema: every backend it can talk
//! to, and for each one every option, its type, default, help text, whether it
//! is required, secret, or advanced, and the values it accepts. Parsing that is
//! what lets the dialog ask for "Region" with a list of regions instead of a
//! `key=value` textarea — and it stays right across rclone upgrades, because
//! the schema comes from the binary the app is actually going to run.
//!
//! Nothing here spawns anything. argv construction and output parsing only;
//! the subprocess lives in the app layer, same as the rest of this crate.

use serde::Deserialize;

/// `rclone config providers` — the whole schema, as JSON.
pub fn providers_args() -> Vec<String> {
    vec!["config".into(), "providers".into()]
}

/// `rclone config update <name> <backend> key=val …` — same shape as create,
/// but keeps options the form did not send. Used by the Edit path.
pub fn update_args(name: &str, backend: &str, opts: &[(&str, &str)]) -> Vec<String> {
    let mut a = vec!["config".into(), "update".into(), name.into(), backend.into()];
    for (k, v) in opts {
        a.push(format!("{k}={v}"));
    }
    a.push("--non-interactive".into());
    a
}

/// `rclone authorize <backend>` — the headless OAuth dance. Opens a browser,
/// waits for the redirect, prints the token between two markers.
pub fn authorize_args(backend: &str) -> Vec<String> {
    vec!["authorize".into(), backend.into()]
}

/// Pull the token blob out of `rclone authorize` output.
///
/// It is wrapped in prose ("Paste the following into your remote machine --->"
/// … "<---End paste"), so the marker lines are stripped rather than the whole
/// stdout being handed over as a value.
pub fn parse_authorize(out: &str) -> Option<String> {
    let start = out.find('{')?;
    let end = out.rfind('}')?;
    if end <= start {
        return None;
    }
    let token = out[start..=end].trim();
    // Cheap sanity check — the shape every backend's token has.
    if token.contains("access_token") || token.contains("token_type") || token.contains("expiry") {
        Some(token.to_string())
    } else {
        None
    }
}

// ---- schema ----

/// One backend rclone can talk to — "drive", "s3", "sftp"…
#[derive(Debug, Clone)]
pub struct Backend {
    pub name: String,
    pub description: String,
    pub options: Vec<Opt>,
}

impl Backend {
    /// Does this backend authenticate over OAuth? Decided by the presence of a
    /// `token` option, which is how rclone itself marks one.
    pub fn is_oauth(&self) -> bool {
        self.options.iter().any(|o| o.name == "token")
    }

    /// The option that gates the others, if there is one. On `s3` and a few
    /// siblings, picking a `provider` changes which of the remaining options
    /// even apply.
    pub fn gate(&self) -> Option<&Opt> {
        self.options.iter().find(|o| o.name == "provider")
    }
}

/// One configurable option on a backend.
#[derive(Debug, Clone)]
pub struct Opt {
    pub name: String,
    /// First paragraph of rclone's help. The rest is usually a wall of prose
    /// about a specific provider's quirks, which does not fit on a form row.
    pub help: String,
    /// rclone's own type name: `string`, `bool`, `int`, `SizeSuffix`, …
    pub kind: String,
    pub default: String,
    pub required: bool,
    /// Masked in the form: a password, a key, a token.
    pub secret: bool,
    pub advanced: bool,
    /// Only the listed values are legal — render a closed dropdown.
    pub exclusive: bool,
    /// Which `provider` values this option applies to. Empty = all. A leading
    /// `!` inverts it into an exclusion list.
    pub provider: String,
    /// `(value, help)` pairs rclone suggests.
    pub examples: Vec<(String, String)>,
}

impl Opt {
    /// Which widget the form should draw. Keeps the .slint free of rclone's
    /// thirteen type names.
    pub fn widget(&self) -> &'static str {
        if self.kind == "bool" {
            "bool"
        } else if !self.examples.is_empty() {
            "select"
        } else if self.secret {
            "secret"
        } else {
            "text"
        }
    }

    /// `access_key_id` → `Access key id`. rclone ships no display names.
    pub fn label(&self) -> String {
        let spaced = self.name.replace('_', " ");
        let mut c = spaced.chars();
        match c.next() {
            Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
            None => String::new(),
        }
    }
}

// ---- parsing ----

#[derive(Deserialize)]
struct RawBackend {
    #[serde(rename = "Name")]
    name: String,
    #[serde(rename = "Description")]
    description: String,
    #[serde(rename = "Options")]
    options: Option<Vec<RawOpt>>,
    #[serde(rename = "Hide")]
    hide: Option<bool>,
}

#[derive(Deserialize)]
struct RawOpt {
    #[serde(rename = "Name")]
    name: String,
    #[serde(rename = "Help")]
    help: Option<String>,
    #[serde(rename = "Type")]
    kind: Option<String>,
    #[serde(rename = "DefaultStr")]
    default_str: Option<String>,
    #[serde(rename = "Required")]
    required: Option<bool>,
    #[serde(rename = "IsPassword")]
    is_password: Option<bool>,
    #[serde(rename = "Sensitive")]
    sensitive: Option<bool>,
    #[serde(rename = "Advanced")]
    advanced: Option<bool>,
    #[serde(rename = "Exclusive")]
    exclusive: Option<bool>,
    #[serde(rename = "Provider")]
    provider: Option<String>,
    #[serde(rename = "Hide")]
    hide: Option<i64>,
    #[serde(rename = "Examples")]
    examples: Option<Vec<RawExample>>,
}

#[derive(Deserialize)]
struct RawExample {
    #[serde(rename = "Value")]
    value: String,
    #[serde(rename = "Help")]
    help: Option<String>,
}

/// rclone's `Hide` bitmask. Bit 1 means "not offered by the configurator" —
/// those options exist for the command line only and have no business on a form.
const HIDE_CONFIGURATOR: i64 = 2;

/// Parse `rclone config providers`. Unknown JSON yields an empty list rather
/// than an error: the dialog falls back to a free-text backend name, which is
/// what it did before this module existed.
pub fn parse_providers(json: &str) -> Vec<Backend> {
    let raw: Vec<RawBackend> = serde_json::from_str(json).unwrap_or_default();
    raw.into_iter()
        .filter(|b| !b.hide.unwrap_or(false))
        .map(|b| Backend {
            name: b.name,
            description: b.description,
            options: b
                .options
                .unwrap_or_default()
                .into_iter()
                .filter(|o| o.hide.unwrap_or(0) & HIDE_CONFIGURATOR == 0)
                .map(|o| Opt {
                    // A password is secret by definition; `Sensitive` covers
                    // the rest (tokens, account ids, endpoints with creds in).
                    secret: o.is_password.unwrap_or(false) || o.sensitive.unwrap_or(false),
                    help: first_paragraph(o.help.as_deref().unwrap_or_default()),
                    kind: o.kind.unwrap_or_else(|| "string".into()),
                    default: o.default_str.unwrap_or_default(),
                    required: o.required.unwrap_or(false),
                    advanced: o.advanced.unwrap_or(false),
                    exclusive: o.exclusive.unwrap_or(false),
                    provider: o.provider.unwrap_or_default(),
                    examples: o
                        .examples
                        .unwrap_or_default()
                        .into_iter()
                        .map(|e| (e.value, first_paragraph(e.help.as_deref().unwrap_or_default())))
                        .collect(),
                    name: o.name,
                })
                .collect(),
        })
        .collect()
}

/// rclone help is one summary line, then blank line, then detail. Only the
/// summary fits a form row.
fn first_paragraph(help: &str) -> String {
    help.split("\n\n").next().unwrap_or(help).replace('\n', " ").trim().to_string()
}

/// Does an option whose `Provider` gate is `gate` apply when `provider` is
/// selected?
///
/// rclone's own rule: empty gate means always; a leading `!` makes the list an
/// exclusion; otherwise it is an inclusion. An unset provider matches
/// everything, so a half-filled form still shows the fields that will matter.
pub fn provider_matches(gate: &str, provider: &str) -> bool {
    if gate.is_empty() || provider.is_empty() {
        return true;
    }
    if let Some(excluded) = gate.strip_prefix('!') {
        !excluded.split(',').any(|p| p.trim() == provider)
    } else {
        gate.split(',').any(|p| p.trim() == provider)
    }
}

/// The options a form should show: gated by the chosen `provider`, and by
/// whether the Advanced switch is on.
///
/// `provider` itself is always shown when the backend has one — it is the
/// control that decides what the rest of the form looks like.
pub fn visible<'a>(backend: &'a Backend, provider: &str, advanced: bool) -> Vec<&'a Opt> {
    backend
        .options
        .iter()
        .filter(|o| o.name == "provider" || advanced || !o.advanced)
        .filter(|o| provider_matches(&o.provider, provider))
        .collect()
}

/// Find a backend by rclone's own name.
pub fn find<'a>(backends: &'a [Backend], name: &str) -> Option<&'a Backend> {
    backends.iter().find(|b| b.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"[
      {"Name":"s3","Description":"Amazon S3 Compliant","Hide":false,"Options":[
        {"Name":"provider","Help":"Choose your S3 provider.\n\nMore prose.","Type":"string",
         "Required":true,"Exclusive":true,
         "Examples":[{"Value":"AWS","Help":"Amazon Web Services"},{"Value":"Ceph","Help":"Ceph"}]},
        {"Name":"region","Help":"Region to connect to.","Type":"string","Provider":"AWS,Ceph"},
        {"Name":"secret_access_key","Help":"Secret key.","Type":"string","Sensitive":true},
        {"Name":"chunk_size","Help":"Chunk size.","Type":"SizeSuffix","Advanced":true,
         "DefaultStr":"5Mi"},
        {"Name":"no_check_bucket","Help":"Skip the check.","Type":"bool","Provider":"!AWS"},
        {"Name":"memory_pool","Help":"internal","Type":"int","Hide":2}
      ]},
      {"Name":"drive","Description":"Google Drive","Hide":false,"Options":[
        {"Name":"token","Help":"OAuth token.","Type":"string","Sensitive":true}
      ]},
      {"Name":"secret","Description":"Hidden","Hide":true,"Options":[]}
    ]"#;

    fn parsed() -> Vec<Backend> {
        parse_providers(SAMPLE)
    }

    #[test]
    fn hidden_backends_and_non_configurable_options_are_dropped() {
        let b = parsed();
        assert_eq!(b.len(), 2, "the Hide:true backend never reaches the form");
        let s3 = find(&b, "s3").unwrap();
        assert!(
            !s3.options.iter().any(|o| o.name == "memory_pool"),
            "Hide bit 2 means command line only",
        );
    }

    #[test]
    fn help_is_cut_to_its_first_paragraph() {
        let s3 = parsed();
        let provider = find(&s3, "s3").unwrap().options.iter().find(|o| o.name == "provider").unwrap();
        assert_eq!(provider.help, "Choose your S3 provider.");
    }

    #[test]
    fn provider_gate_includes_excludes_and_defaults_open() {
        assert!(provider_matches("", "AWS"), "no gate applies everywhere");
        assert!(provider_matches("AWS,Ceph", "Ceph"));
        assert!(!provider_matches("AWS,Ceph", "Minio"));
        assert!(!provider_matches("!AWS", "AWS"), "a leading ! inverts the list");
        assert!(provider_matches("!AWS", "Minio"));
        // Nothing picked yet: show everything rather than an empty form.
        assert!(provider_matches("AWS", ""));
    }

    #[test]
    fn visible_hides_advanced_but_never_the_gate_itself() {
        let b = parsed();
        let s3 = find(&b, "s3").unwrap();

        let basic: Vec<&str> = visible(s3, "AWS", false).iter().map(|o| o.name.as_str()).collect();
        assert!(basic.contains(&"provider"));
        assert!(basic.contains(&"region"));
        assert!(!basic.contains(&"chunk_size"), "advanced is off");
        assert!(!basic.contains(&"no_check_bucket"), "gated out by !AWS");

        let adv: Vec<&str> = visible(s3, "AWS", true).iter().map(|o| o.name.as_str()).collect();
        assert!(adv.contains(&"chunk_size"));
    }

    #[test]
    fn widgets_follow_type_and_secrecy() {
        let b = parsed();
        let s3 = find(&b, "s3").unwrap();
        let by = |n: &str| s3.options.iter().find(|o| o.name == n).unwrap().widget();
        assert_eq!(by("provider"), "select", "examples mean a dropdown");
        assert_eq!(by("no_check_bucket"), "bool");
        assert_eq!(by("secret_access_key"), "secret");
        assert_eq!(by("chunk_size"), "text");
    }

    #[test]
    fn oauth_backends_are_the_ones_with_a_token() {
        let b = parsed();
        assert!(find(&b, "drive").unwrap().is_oauth());
        assert!(!find(&b, "s3").unwrap().is_oauth());
    }

    #[test]
    fn labels_are_readable() {
        let b = parsed();
        let s3 = find(&b, "s3").unwrap();
        let o = s3.options.iter().find(|o| o.name == "secret_access_key").unwrap();
        assert_eq!(o.label(), "Secret access key");
    }

    #[test]
    fn authorize_token_is_lifted_out_of_the_prose() {
        let out = "Paste the following into your remote machine --->\n\
                   {\"access_token\":\"abc\",\"expiry\":\"2030-01-01\"}\n\
                   <---End paste";
        assert_eq!(
            parse_authorize(out).as_deref(),
            Some(r#"{"access_token":"abc","expiry":"2030-01-01"}"#),
        );
        assert!(parse_authorize("Failed to get token").is_none());
        assert!(parse_authorize("{\"error\":\"nope\"}").is_none(), "not a token shape");
    }

    #[test]
    fn update_argv_is_non_interactive_like_create() {
        let a = update_args("gdrive", "drive", &[("scope", "drive")]);
        assert_eq!(a[..4], ["config", "update", "gdrive", "drive"]);
        assert!(a.contains(&"scope=drive".to_string()));
        assert!(a.contains(&"--non-interactive".to_string()));
    }
}
