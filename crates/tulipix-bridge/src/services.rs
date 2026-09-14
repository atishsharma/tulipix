//! The seven service integrations, in one place: whether each is switched on,
//! and the key it needs.
//!
//! Every one of them was a switch in Settings that nothing read. The rule
//! here is the same for all seven — a client only exists when its switch is
//! on *and* its key is saved, so a caller that gets `Some(client)` never has
//! to check either again, and a caller that gets `None` carries on with
//! whatever it was already doing. None of them is ever the first source: they
//! run when MusicBrainz, TMDB, OpenSubtitles or yt-dlp came back with nothing.
//!
//! Keys live in the system keychain (`api_keys`), never settings.json. The
//! switches live in settings.json, because a switch is not a secret.

use std::sync::{Arc, Mutex, OnceLock};

use tulipix_music::discogs::DiscogsClient;
use tulipix_music::spotify_api::SpotifyClient;
use tulipix_music::youtube_data::YoutubeDataClient;
use tulipix_videos::anime::{AniDbProvider, AniListProvider};
use tulipix_videos::sub_addic7ed::Addic7edClient;
use tulipix_videos::trakt::TraktClient;

/// Whether a Settings switch is on. Every integration is off until asked for.
pub(crate) fn on(flag: &str) -> bool {
    crate::api::shell::load().flag(flag, false)
}

/// A saved key, or `None` when there is none worth sending. Blank counts as
/// none: a key box someone opened and closed is not a key.
pub(crate) fn key(service: &str) -> Option<String> {
    tulipix_core::api_keys::fetch(service)
        .ok()
        .flatten()
        .map(|k| k.trim().to_string())
        .filter(|k| !k.is_empty())
}

/// Discogs: music metadata where MusicBrainz has no match.
pub(crate) fn discogs() -> Option<DiscogsClient> {
    if !on("api.discogs") {
        return None;
    }
    Some(DiscogsClient::new(key("discogs")?))
}

/// Spotify: the artist page, for genres and a picture.
///
/// One client for the process, because it caches an hour-long token and a
/// fresh client per lookup would ask for a new one every time. It is rebuilt
/// when the saved id changes, which is the only thing that invalidates it.
pub(crate) fn spotify() -> Option<Arc<SpotifyClient>> {
    if !on("api.spotify") {
        return None;
    }
    let (id, secret) = (key("spotify_id")?, key("spotify_secret")?);
    static CACHE: OnceLock<Mutex<Option<(String, Arc<SpotifyClient>)>>> = OnceLock::new();
    let cell = CACHE.get_or_init(|| Mutex::new(None));
    let mut g = cell.lock().unwrap_or_else(|p| p.into_inner());
    if let Some((cached_id, client)) = g.as_ref() {
        if *cached_id == id {
            return Some(client.clone());
        }
    }
    let client = Arc::new(SpotifyClient::new(&id, &secret));
    *g = Some((id, client.clone()));
    Some(client)
}

/// YouTube Data: view counts and real thumbnails on the YouTube tab's cards,
/// where yt-dlp gives a title and a duration.
pub(crate) fn youtube_data() -> Option<YoutubeDataClient> {
    if !on("api.youtube-data") {
        return None;
    }
    Some(YoutubeDataClient::new(key("youtube_data")?))
}

/// AniList: anime titles and overviews. The one integration with no key —
/// their API is open, so the switch is the whole of it.
pub(crate) fn anilist() -> Option<AniListProvider> {
    on("api.anilist").then(AniListProvider::new)
}

/// AniDB: per-episode data, which AniList does not have. Their terms require
/// a client registered on their site; `anidb` holds that name.
pub(crate) fn anidb() -> Option<AniDbProvider> {
    if !on("api.anidb") {
        return None;
    }
    let name = key("anidb")?;
    let dir = tulipix_core::paths::cache_dir()?.join("anidb");
    Some(AniDbProvider::new(name, dir))
}

/// Addic7ed: the subtitle source for what OpenSubtitles has not got yet.
/// No key — the switch is the whole of it.
pub(crate) fn addic7ed() -> Option<Addic7edClient> {
    on("api.addic7ed").then(Addic7edClient::new)
}

/// Trakt: a finished film or episode, sent to the account. Needs the app's id
/// and secret *and* the token the device flow handed back, so this is `None`
/// until the account has been linked.
pub(crate) fn trakt() -> Option<(TraktClient, String)> {
    if !on("api.trakt") {
        return None;
    }
    let token = key("trakt_token")?;
    Some((TraktClient::new(key("trakt")?, key("trakt_secret").unwrap_or_default()), token))
}

/// The client for linking, which is the one call that runs before there is a
/// token. Only the id and secret are needed.
pub(crate) fn trakt_app() -> Option<TraktClient> {
    Some(TraktClient::new(key("trakt")?, key("trakt_secret")?))
}
