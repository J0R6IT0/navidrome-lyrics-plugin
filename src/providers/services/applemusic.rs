use crate::{
    config::{PluginConfig, ProviderParams},
    ext::TrackInfoExt,
    providers::{LyricsProvider, ProviderResult, error::ProviderError, http::Http},
    types::{Lyrics, LyricsKind},
};
use extism_pdk::info;
use nd_pdk::{host::cache, lyrics::TrackInfo};
use regex::Regex;
use serde::Deserialize;
use std::{collections::HashMap, time::Duration};

const MUSIC_APPLE_COM: &str = "https://music.apple.com";
const BASE_URL: &str = "https://amp-api.music.apple.com/v1";
const STOREFRONT_URL: &str = "https://api.music.apple.com/v1/me/storefront";
const DEFAULT_STOREFRONT: &str = "us";

const DEV_TOKEN_CACHE_KEY: &str = "applemusic:dev-token";
const STOREFRONT_CACHE_PREFIX: &str = "applemusic:storefront:";

const DEV_TOKEN_TTL: i64 = 7 * 24 * 3600;
const STOREFRONT_TTL: i64 = 30 * 24 * 3600;

enum LookupError {
    Unauthorized,
    Fatal(ProviderError),
}

impl From<ProviderError> for LookupError {
    fn from(e: ProviderError) -> Self {
        LookupError::Fatal(e)
    }
}

fn is_unauthorized(status: i32) -> bool {
    status == 401 || status == 403
}

struct Session<'a> {
    dev_token: &'a str,
    media_user_token: &'a str,
}

impl Session<'_> {
    fn headers(&self) -> HashMap<String, String> {
        HashMap::from([
            ("Authorization".into(), format!("Bearer {}", self.dev_token)),
            ("Origin".into(), MUSIC_APPLE_COM.into()),
            ("Referer".into(), MUSIC_APPLE_COM.into()),
            ("media-user-token".into(), self.media_user_token.into()),
        ])
    }
}

#[derive(Debug, Deserialize)]
struct SearchResponse {
    results: Option<SearchResults>,
}

#[derive(Debug, Deserialize)]
struct SearchResults {
    songs: Option<SongData>,
}

#[derive(Debug, Deserialize)]
struct SongData {
    #[serde(default)]
    data: Vec<Song>,
}

#[derive(Debug, Deserialize)]
struct Song {
    id: String,
    attributes: SongAttributes,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SongAttributes {
    duration_in_millis: Option<u64>,
    has_lyrics: Option<bool>,
    #[serde(default)]
    album_name: String,
}

#[derive(Debug, Deserialize)]
struct StorefrontResponse {
    #[serde(default)]
    data: Vec<StorefrontEntry>,
}

#[derive(Debug, Deserialize)]
struct StorefrontEntry {
    id: String,
}

#[derive(Debug, Deserialize)]
struct LyricsResponse {
    #[serde(default)]
    data: Vec<LyricsEntry>,
}

#[derive(Debug, Deserialize)]
struct LyricsEntry {
    attributes: LyricsAttributes,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LyricsAttributes {
    ttml: Option<String>,
    ttml_localizations: Option<String>,
}

pub struct AppleMusic {
    media_user_token: Option<String>,
    storefront: Option<String>,
    translation_language: Option<String>,
    romanization_script: Option<String>,
}

impl AppleMusic {
    pub fn create(params: &ProviderParams) -> Box<dyn LyricsProvider> {
        Box::new(Self {
            media_user_token: params.get("mediaUserToken").map(str::to_string),
            storefront: params.get("storefront").map(str::to_string),
            translation_language: params
                .get("translationLanguage")
                .filter(|s| *s != "none")
                .map(str::to_string),
            romanization_script: params
                .get("romanizationScript")
                .filter(|s| *s != "none")
                .map(str::to_string),
        })
    }

    fn resolve_storefront(&self, session: &Session) -> Result<String, LookupError> {
        if let Some(s) = &self.storefront {
            return Ok(s.clone());
        }

        let key = storefront_cache_key(session.media_user_token);
        if let Ok(Some(cached)) = cache::get_string(&key) {
            return Ok(cached);
        }

        match self.fetch_storefront(session)? {
            Some(storefront) => {
                info!("applemusic: resolved and cached storefront '{storefront}'");
                let _ = cache::set_string(&key, &storefront, STOREFRONT_TTL);
                Ok(storefront)
            }
            None => Ok(DEFAULT_STOREFRONT.to_string()),
        }
    }

    fn fetch_storefront(&self, session: &Session) -> Result<Option<String>, LookupError> {
        let response = match Http::get(STOREFRONT_URL)
            .browser()
            .headers(session.headers())
            .send()
        {
            Ok(r) => r,
            Err(_) => return Ok(None),
        };

        match response.status {
            200 => Ok(response
                .json::<StorefrontResponse>("storefront")
                .ok()
                .and_then(|r| r.data.into_iter().next())
                .map(|e| e.id)),
            s if is_unauthorized(s) => Err(LookupError::Unauthorized),
            429 => Err(LookupError::Fatal(response.rate_limited())),
            _ => Ok(None),
        }
    }

    fn search(
        &self,
        session: &Session,
        storefront: &str,
        query: &str,
    ) -> Result<Vec<Song>, LookupError> {
        let response = Http::get(format!("{BASE_URL}/catalog/{storefront}/search"))
            .param("types", "songs")
            .param("term", query)
            .browser()
            .headers(session.headers())
            .send()?;

        let parsed: SearchResponse = match response.status {
            200 => response.json("search")?,
            s if is_unauthorized(s) => return Err(LookupError::Unauthorized),
            429 => return Err(LookupError::Fatal(response.rate_limited())),
            _ => return Err(LookupError::Fatal(response.unexpected_status("the search"))),
        };

        Ok(parsed
            .results
            .and_then(|r| r.songs)
            .map(|s| s.data)
            .unwrap_or_default())
    }

    fn get_lyrics(
        &self,
        session: &Session,
        storefront: &str,
        song_id: &str,
        translation_language: Option<&str>,
        script: Option<&str>,
    ) -> Result<Option<Lyrics>, LookupError> {
        let response = lyrics_request(storefront, song_id, translation_language, script)
            .browser()
            .headers(session.headers())
            .send()?;

        let parsed: LyricsResponse = match response.status {
            200 => response.json("lyrics")?,
            404 => return Ok(None),
            s if is_unauthorized(s) => return Err(LookupError::Unauthorized),
            429 => return Err(LookupError::Fatal(response.rate_limited())),
            _ => {
                return Err(LookupError::Fatal(
                    response.unexpected_status("the lyrics endpoint"),
                ));
            }
        };

        let ttml = parsed
            .data
            .into_iter()
            .next()
            .and_then(|e| e.attributes.ttml_localizations.or(e.attributes.ttml))
            .filter(|t| !t.trim().is_empty());

        Ok(ttml.map(Lyrics::Ttml))
    }
}

impl LyricsProvider for AppleMusic {
    fn supported_kinds(&self) -> &'static [LyricsKind] {
        &[LyricsKind::Ttml]
    }

    fn log_params(&self) -> Vec<(&'static str, String)> {
        let mut params = Vec::new();

        if let Some(storefront) = &self.storefront {
            params.push(("storefront", storefront.clone()));
        }
        if let Some(lang) = &self.translation_language {
            params.push(("translationLanguage", lang.clone()));
        }
        if let Some(script) = &self.romanization_script {
            params.push(("romanizationScript", script.clone()));
        }

        params
    }

    fn fetch_lyrics(
        &self,
        track: &TrackInfo,
        cfg: &PluginConfig,
    ) -> ProviderResult<Option<Lyrics>> {
        if !track.has_artist() {
            return Err(ProviderError::other("track has no artist"));
        }

        let token = self.media_user_token.as_deref().ok_or_else(|| {
            ProviderError::other(
                "a media-user-token is required, configure it in the provider settings",
            )
        })?;

        let query = format!(
            "{} {}",
            track.title,
            track.first_artist().unwrap_or_default()
        );

        let attempt = |force_refresh: bool| -> Result<Option<Lyrics>, LookupError> {
            let dev_token = &get_dev_token(force_refresh)?;
            let session = Session {
                dev_token,
                media_user_token: token,
            };

            let storefront = self.resolve_storefront(&session)?;

            let song = self
                .search(&session, &storefront, &query)?
                .into_iter()
                .filter(|s| s.attributes.has_lyrics != Some(false))
                .filter(|s| {
                    s.attributes.duration_in_millis.is_some_and(|d| {
                        track.matches_duration(Duration::from_millis(d), cfg.duration_tolerance)
                    })
                })
                .filter(|s| {
                    !cfg.require_album_match || track.matches_album(&s.attributes.album_name)
                })
                .min_by_key(|s| duration_diff(s, track.duration()));

            let Some(song) = song else {
                return Ok(None);
            };

            let translation_language = self.translation_language.as_deref();
            let script = self.romanization_script.as_deref();

            self.get_lyrics(
                &session,
                &storefront,
                &song.id,
                translation_language,
                script,
            )
        };

        let result = match attempt(false) {
            Err(LookupError::Unauthorized) => {
                info!("applemusic: cached developer token rejected, refreshing");
                attempt(true)
            }
            result => result,
        };

        result.map_err(|e| match e {
            LookupError::Fatal(e) => e,
            LookupError::Unauthorized => ProviderError::other(
                "authorization failed even after refreshing the developer token",
            ),
        })
    }
}

fn get_dev_token(force_refresh: bool) -> ProviderResult<String> {
    if !force_refresh && let Ok(Some(token)) = cache::get_string(DEV_TOKEN_CACHE_KEY) {
        info!("applemusic: reusing cached developer token");
        return Ok(token);
    }

    info!("applemusic: scraping fresh developer token");
    let token = fetch_dev_token()?;
    let _ = cache::set_string(DEV_TOKEN_CACHE_KEY, &token, DEV_TOKEN_TTL);
    Ok(token)
}

fn capture<'a>(re: &Regex, haystack: &'a str, what: &str) -> ProviderResult<&'a str> {
    re.captures(haystack)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str())
        .ok_or_else(|| ProviderError::other(format!("could not find {what}")))
}

fn fetch_dev_token() -> ProviderResult<String> {
    let home = Http::get(format!("{MUSIC_APPLE_COM}/")).browser().send()?;
    let html = home.text();

    let script_re =
        Regex::new(r#"<script type="module" crossorigin src="(/assets/index[^"]+\.js)""#)
            .map_err(|e| ProviderError::other(format!("invalid script regex: {e}")))?;
    let script_path = capture(&script_re, &html, "the index script tag")?;

    let script = Http::get(format!("{MUSIC_APPLE_COM}{script_path}"))
        .browser()
        .send()?;
    let js = script.text();

    let var_re = Regex::new(r#"\.headers\.Authorization\s*=\s*`Bearer \$\{([A-Za-z0-9_$]+)\}`"#)
        .map_err(|e| ProviderError::other(format!("invalid token var regex: {e}")))?;
    let var_name = capture(&var_re, &js, "the auth token variable")?;

    let value_re = Regex::new(&format!(
        r#"{}\s*=\s*"(eyJ[A-Za-z0-9._-]+)""#,
        regex::escape(var_name)
    ))
    .map_err(|e| ProviderError::other(format!("invalid token value regex: {e}")))?;
    let token = capture(&value_re, &js, "the auth token value")?;

    Ok(token.to_string())
}

fn storefront_cache_key(media_user_token: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in media_user_token.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{STOREFRONT_CACHE_PREFIX}{hash:016x}")
}

fn duration_diff(song: &Song, target: Duration) -> Duration {
    song.attributes
        .duration_in_millis
        .map(|d| Duration::from_millis(d).abs_diff(target))
        .unwrap_or(Duration::MAX)
}

fn lyrics_request(
    storefront: &str,
    song_id: &str,
    translation_language: Option<&str>,
    script: Option<&str>,
) -> Http {
    let mut request = Http::get(format!(
        "{BASE_URL}/catalog/{storefront}/songs/{song_id}/syllable-lyrics"
    ));

    if translation_language.is_none() && script.is_none() {
        return request;
    }

    request = request.param("extend", "ttmlLocalizations");
    if let Some(lang) = translation_language {
        request = request.param("l[lyrics]", lang);
    }
    if let Some(script) = script {
        request = request.param("l[script]", script);
    }

    request
}

#[cfg(test)]
mod tests {
    use super::*;

    #[track_caller]
    fn check_lyrics_url(
        storefront: &str,
        translation_language: Option<&str>,
        script: Option<&str>,
        expected: &str,
    ) {
        let url = lyrics_request(storefront, "123", translation_language, script)
            .url()
            .unwrap();

        assert_eq!(
            url, expected,
            "{storefront} with {translation_language:?} and {script:?}"
        );
    }

    #[test]
    fn a_lyrics_request_without_extras_is_a_bare_url() {
        check_lyrics_url(
            "us",
            None,
            None,
            "https://amp-api.music.apple.com/v1/catalog/us/songs/123/syllable-lyrics",
        );
    }

    #[test]
    fn a_translation_asks_for_the_localizations_extension() {
        check_lyrics_url(
            "us",
            Some("en-US"),
            None,
            "https://amp-api.music.apple.com/v1/catalog/us/songs/123/syllable-lyrics\
             ?extend=ttmlLocalizations&l%5Blyrics%5D=en-US",
        );
    }

    #[test]
    fn a_romanization_asks_for_the_script_instead() {
        check_lyrics_url(
            "us",
            None,
            Some("und-Latn"),
            "https://amp-api.music.apple.com/v1/catalog/us/songs/123/syllable-lyrics\
             ?extend=ttmlLocalizations&l%5Bscript%5D=und-Latn",
        );
    }

    #[test]
    fn a_translation_and_a_romanization_go_together() {
        check_lyrics_url(
            "gb",
            Some("es-ES"),
            Some("und-Latn"),
            "https://amp-api.music.apple.com/v1/catalog/gb/songs/123/syllable-lyrics\
             ?extend=ttmlLocalizations&l%5Blyrics%5D=es-ES&l%5Bscript%5D=und-Latn",
        );
    }

    #[track_caller]
    fn check_storefront_cache_key(a: &str, b: &str, expected_equal: bool) {
        assert_eq!(
            storefront_cache_key(a) == storefront_cache_key(b),
            expected_equal,
            "{a:?} vs {b:?}"
        );
    }

    #[test]
    fn a_storefront_cache_key_is_stable_and_prefixed() {
        let key = storefront_cache_key("token-abc");
        assert!(key.starts_with(STOREFRONT_CACHE_PREFIX));
        check_storefront_cache_key("token-abc", "token-abc", true);
    }

    #[test]
    fn a_storefront_cache_key_differs_per_token() {
        check_storefront_cache_key("token-abc", "token-xyz", false);
    }
}
