// Deezer lyrics provider based on the work in lrcmux
// (https://github.com/f1nniboy/lrcmux/tree/main/internal/providers/deezer)

use crate::config::{PluginConfig, ProviderParams};
use crate::ext::TrackInfoExt;
use crate::format::elrc;
use crate::providers::error::{ProviderError, ProviderResult};
use crate::providers::http::Http;
use crate::providers::{LyricsProvider, ProviderLyrics};
use crate::types::{Lyrics, LyricsKind, Word};
use extism_pdk::info;
use nd_pdk::{host::cache, lyrics::TrackInfo};
use serde::{Deserialize, Serialize};
use std::time::Duration;

const AUTH_URL: &str = "https://auth.deezer.com/login/anonymous?jo=p&rto=c&i=c";
const API_URL: &str = "https://api.deezer.com";
const PIPE_URL: &str = "https://pipe.deezer.com/api";

const TOKEN_CACHE_KEY: &str = "deezer:token";
const TOKEN_TTL: i64 = 5 * 60;

const SEARCH_LIMIT: i32 = 50;

const FALLBACK_LANGUAGE: &str = "en-US";

const LYRICS_QUERY: &str = "query GetLyrics($trackId: String!) {
  track(trackId: $trackId) {
    lyrics {
      text
      synchronizedWordByWordLines { words { start end word } }
      synchronizedLines { line lineTranslated lrcTimestamp milliseconds duration }
    }
  }
}";

const ERROR_QUOTA_EXCEEDED: i64 = 4;
const ERROR_NO_DATA: i64 = 800;

#[derive(Deserialize, Debug)]
struct DeezerTrack {
    album: DeezerAlbum,
    id: i64,
    duration: u64,
}

#[derive(Deserialize, Debug)]
struct DeezerAlbum {
    title: String,
}

#[derive(Deserialize, Debug)]
struct DeezerError {
    #[serde(rename = "type")]
    error_type: String,
    message: String,
    code: i64,
}

impl From<DeezerError> for ProviderError {
    fn from(error: DeezerError) -> Self {
        match error.code {
            ERROR_QUOTA_EXCEEDED => ProviderError::rate_limited(None),
            _ => ProviderError::other(format!(
                "deezer error {}: {}",
                error.error_type, error.message
            )),
        }
    }
}

impl DeezerError {
    pub fn is_no_data(&self) -> bool {
        self.code == ERROR_NO_DATA
    }
}

#[derive(Deserialize, Debug)]
struct DeezerSearchResponse {
    error: Option<DeezerError>,
    data: Vec<DeezerTrack>,
}

#[derive(Deserialize)]
struct DeezerAuthResponse {
    jwt: String,
}

#[derive(Serialize)]
struct DeezerGraphQlRequestVariables {
    #[serde(rename = "trackId")]
    track_id: String,
}

#[derive(Serialize)]
struct DeezerGraphQlRequest {
    query: String,
    variables: DeezerGraphQlRequestVariables,
}

#[derive(Deserialize)]
struct DeezerWord {
    word: String,
    start: i64,
    end: i64,
}

#[derive(Deserialize)]
struct WordLine {
    words: Vec<DeezerWord>,
}

#[derive(Deserialize)]
struct SyncedLine {
    line: String,
    #[serde(rename = "lineTranslated")]
    line_translated: String,
    #[serde(rename = "lrcTimestamp")]
    lrc_timestamp: String,
}

#[derive(Deserialize)]
struct LyricsData {
    #[serde(default)]
    text: String,
    #[serde(rename = "synchronizedWordByWordLines", default)]
    word_lines: Option<Vec<WordLine>>,
    #[serde(rename = "synchronizedLines", default)]
    lines: Option<Vec<SyncedLine>>,
}

#[derive(Deserialize)]
struct Track {
    lyrics: Option<LyricsData>,
}

#[derive(Deserialize)]
struct Data {
    track: Track,
}

#[derive(Deserialize)]
struct DeezerGraphQlErrorExtensions {
    #[serde(rename = "type")]
    error_type: String,
}

#[derive(Deserialize)]
struct DeezerGraphQlError {
    message: String,
    extensions: DeezerGraphQlErrorExtensions,
}

#[derive(Deserialize)]
struct DeezerLyricsResponse {
    data: Option<Data>,
    #[serde(default)]
    errors: Vec<DeezerGraphQlError>,
}

enum LookupError {
    Unauthorized,
    Other(ProviderError),
}

pub struct Deezer {
    translation_language: Option<String>,
}

impl Deezer {
    pub fn create(params: &ProviderParams) -> Box<dyn LyricsProvider> {
        Box::new(Deezer {
            translation_language: params
                .get("translationLanguage")
                .filter(|s| *s != "none")
                .map(str::to_string),
        })
    }

    fn search(&self, query: String) -> ProviderResult<Vec<DeezerTrack>> {
        let response = Http::get(format!("{API_URL}/search"))
            .param("q", query)
            .param("limit", SEARCH_LIMIT.to_string())
            .browser()
            .send()?;

        let parsed: DeezerSearchResponse = match response.status {
            200 => response.json("the search")?,
            403 | 429 => return Err(response.rate_limited()),
            _ => return Err(response.unexpected_status("the search")),
        };

        if let Some(error) = parsed.error
            && !error.is_no_data()
        {
            return Err(ProviderError::from(error));
        }

        Ok(parsed.data)
    }

    fn get_lyrics(
        &self,
        song_id: i64,
        token: String,
        translation_language: Option<&str>,
    ) -> Result<Option<ProviderLyrics>, LookupError> {
        let body = DeezerGraphQlRequest {
            query: LYRICS_QUERY.to_string(),
            variables: DeezerGraphQlRequestVariables {
                track_id: song_id.to_string(),
            },
        };

        let response = match Http::post(PIPE_URL)
            .body(serde_json::to_vec(&body).unwrap_or_default())
            .header("Content-Type", "application/json")
            .header("Authorization", format!("Bearer {}", token))
            .header(
                "Accept-Language",
                translation_language.unwrap_or(FALLBACK_LANGUAGE),
            )
            .browser()
            .send()
        {
            Ok(response) => response,
            Err(err) => return Err(LookupError::Other(err)),
        };

        if response.status == 429 || response.status == 403 {
            return Err(LookupError::Other(response.rate_limited()));
        }

        if response.status != 200 {
            return Err(LookupError::Other(
                response.unexpected_status("the lyrics endpoint"),
            ));
        }

        let parsed: DeezerLyricsResponse = match response.json("the lyrics") {
            Ok(parsed) => parsed,
            Err(err) => return Err(LookupError::Other(err)),
        };

        let mut not_found = false;
        for error in parsed.errors {
            match error.extensions.error_type.as_str() {
                "LyricsNotFoundError" | "TrackNotFoundError" => not_found = true,
                "JwtTokenExpiredError" | "JwtTokenInvalidError" => {
                    return Err(LookupError::Unauthorized);
                }
                _ => {
                    return Err(LookupError::Other(ProviderError::other(format!(
                        "deezer graphql error {}: {}",
                        error.extensions.error_type, error.message
                    ))));
                }
            }
        }

        if not_found || parsed.data.is_none() {
            return Ok(None);
        }

        Ok(if let Some(lyrics) = parsed.data.unwrap().track.lyrics {
            parse_lyrics(lyrics, translation_language)
        } else {
            None
        })
    }
}

impl LyricsProvider for Deezer {
    fn supported_kinds(&self) -> &'static [LyricsKind] {
        &[LyricsKind::Elrc, LyricsKind::Lrc, LyricsKind::Plain]
    }

    fn fetch_lyrics(
        &self,
        track: &TrackInfo,
        cfg: &PluginConfig,
    ) -> ProviderResult<Option<ProviderLyrics>> {
        if !track.has_artist() {
            return Err(ProviderError::other("track has no artist"));
        }

        let query = format!(
            "{} {}",
            track.first_artist().unwrap_or_default(),
            track.title
        );

        let search_results = self.search(query)?;
        let result = search_results
            .iter()
            .filter(|t| {
                track.matches_duration(Duration::from_secs(t.duration), cfg.duration_tolerance)
            })
            .filter(|t| !cfg.require_album_match || track.matches_album(&t.album.title))
            .min_by_key(|t| Duration::from_secs(t.duration).abs_diff(track.duration()));
        let song_id = match result {
            Some(result) => result.id,
            None => return Ok(None),
        };

        let attempt = |force_refresh: bool| -> Result<Option<ProviderLyrics>, LookupError> {
            let token = match get_token(force_refresh) {
                Ok(token) => token,
                Err(err) => return Err(LookupError::Other(err)),
            };

            self.get_lyrics(song_id, token, self.translation_language.as_deref())
        };

        let result = match attempt(false) {
            Err(LookupError::Unauthorized) => {
                info!("deezer: cached token rejected, refreshing");
                attempt(true)
            }
            result => result,
        };

        result.map_err(|e| match e {
            LookupError::Unauthorized => {
                ProviderError::other("deezer: authorization failed even after refreshing the token")
            }
            LookupError::Other(err) => err,
        })
    }

    fn log_params(&self) -> Vec<(&'static str, String)> {
        let mut params = Vec::new();

        if let Some(lang) = &self.translation_language {
            params.push(("translationLanguage", lang.clone()));
        }

        params
    }
}
fn get_token(force_refresh: bool) -> ProviderResult<String> {
    if !force_refresh && let Ok(Some(token)) = cache::get_string(TOKEN_CACHE_KEY) {
        info!("deezer: reusing cached token");
        return Ok(token);
    }

    info!("deezer: scraping fresh token");
    let token = fetch_token()?;
    let _ = cache::set_string(TOKEN_CACHE_KEY, &token, TOKEN_TTL);
    Ok(token)
}

fn fetch_token() -> ProviderResult<String> {
    let response = Http::get(AUTH_URL).browser().send()?;
    let parsed: DeezerAuthResponse = match response.status {
        200 => response.json("deezer auth")?,
        403 | 429 => return Err(response.rate_limited()),
        _ => return Err(response.unexpected_status("the auth endpoint")),
    };
    Ok(parsed.jwt)
}

fn parse_lyrics(lyrics: LyricsData, translation_language: Option<&str>) -> Option<ProviderLyrics> {
    let translate = translation_language.is_some();
    let translated: Option<Vec<String>> = if translate
        && let Some(ref lines) = lyrics.lines
        && !lines.is_empty()
        && !lines[0].line_translated.is_empty()
    {
        Some(
            lines
                .iter()
                .map(|line| format!("[{}]{}", line.lrc_timestamp, line.line_translated))
                .collect(),
        )
    } else {
        None
    };

    let out = if let Some(word_lines) = lyrics.word_lines
        && !word_lines.is_empty()
    {
        let out: Vec<String> = word_lines
            .iter()
            .filter_map(|line| {
                (!line.words.is_empty())
                    .then(|| {
                        let line_start = line.words[0].start;
                        let words: Vec<Word> = line
                            .words
                            .iter()
                            .map(|word| Word {
                                text: format!("{} ", word.word),
                                start_ms: word.start,
                                end_ms: word.end,
                            })
                            .collect();
                        elrc::render_line(line_start, &words)
                    })
                    .flatten()
            })
            .collect();

        ProviderLyrics::from(Lyrics::Elrc(out.join("\n")))
    } else if let Some(lines) = lyrics.lines
        && !lines.is_empty()
    {
        let out: Vec<String> = lines
            .iter()
            .map(|line| format!("{}{}", line.lrc_timestamp, line.line))
            .collect();

        ProviderLyrics::from(Lyrics::Lrc(out.join("\n")))
    } else {
        ProviderLyrics::from(Lyrics::Plain(lyrics.text))
    };

    if out.lyrics.kind() != LyricsKind::Plain
        && let Some(translated) = translated
    {
        Some(out.translation(
            translated.join("\n"),
            translation_language.unwrap_or_default().to_string(),
        ))
    } else {
        Some(out)
    }
}
