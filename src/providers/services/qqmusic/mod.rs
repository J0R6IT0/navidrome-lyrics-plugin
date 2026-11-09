//! QQ Music provider, using the endpoints of the QQ Music Lite Android client
//! as found by https://github.com/chenmozhijin/LDDC (LDDC/core/api/lyrics/qm.py).
//!
//! As of October 22 2026, the device identity block, client headers and many other
//! parameters seem to not be required at all. If something break they should be
//! added back to see if that's the reason.

use super::ChineseTranslation;
use crate::{
    config::{PluginConfig, ProviderParams},
    ext::TrackInfoExt,
    format::lrc,
    providers::{LyricsProvider, ProviderLyrics, ProviderResult, error::ProviderError, http::Http},
    types::{Lyrics, LyricsKind},
};
use extism_pdk::{info, warn};
use nd_pdk::lyrics::TrackInfo;
use regex::Regex;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::time::Duration;

mod qrc;

const LITE_URL: &str = "https://shu6.y.qq.com/cgi-bin/musicu.fcg";
const DESKTOP_URL: &str = "https://u.y.qq.com/cgi-bin/musicu.fcg";
const SEARCH_MODULE: &str = "music.search.SearchCgiService";
const LITE_SEARCH_METHOD: &str = "DoSearchForQQMusicLite";
const DESKTOP_SEARCH_METHOD: &str = "DoSearchForQQMusicDesktop";
const LYRIC_MODULE: &str = "music.musichallSong.PlayLyricInfo";
const LYRIC_METHOD: &str = "GetPlayLyricInfo";

#[derive(Debug, Serialize)]
struct SearchParam<'a> {
    query: &'a str,
    /// 0 searches for songs.
    search_type: u32,
    num_per_page: u32,
    page_num: u32,
}

impl<'a> SearchParam<'a> {
    fn new(query: &'a str) -> Self {
        Self {
            query,
            search_type: 0,
            num_per_page: 5,
            page_num: 1,
        }
    }
}

/// Without `qrc` and `crypt` the endpoint returns line synced lyrics.
#[derive(Debug, Serialize)]
struct LyricParam {
    #[serde(rename = "songID")]
    song_id: u64,
    qrc: u32,
    crypt: u32,
    /// With `1` the response also carries the translation, if there is one.
    trans: u32,
}

impl LyricParam {
    fn new(song: &Song, translation: bool) -> Self {
        Self {
            song_id: song.id,
            qrc: 1,
            crypt: 1,
            trans: translation.into(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct ApiResponse<T> {
    code: Option<i64>,
    request: Option<ModuleResponse<T>>,
}

#[derive(Debug, Deserialize)]
struct ModuleResponse<T> {
    code: Option<i64>,
    data: Option<T>,
}

#[derive(Debug, Deserialize)]
struct LiteSearchData {
    #[serde(default)]
    body: LiteSearchBody,
}

#[derive(Debug, Default, Deserialize)]
struct LiteSearchBody {
    #[serde(default)]
    item_song: Vec<Song>,
}

#[derive(Debug, Deserialize)]
struct DesktopSearchData {
    #[serde(default)]
    body: DesktopSearchBody,
}

#[derive(Debug, Default, Deserialize)]
struct DesktopSearchBody {
    #[serde(default)]
    song: SongList,
}

#[derive(Debug, Default, Deserialize)]
struct SongList {
    #[serde(default)]
    list: Vec<Song>,
}

#[derive(Debug, Deserialize)]
struct Song {
    id: u64,
    #[serde(default)]
    album: Album,
    /// Track length in seconds.
    #[serde(default)]
    interval: u64,
}

#[derive(Debug, Default, Deserialize)]
struct Album {
    #[serde(default)]
    name: String,
}

#[derive(Debug, Deserialize)]
struct LyricData {
    #[serde(default)]
    lyric: String,
    #[serde(default)]
    trans: String,
}

struct SongLyrics {
    content: String,
    translation: Option<String>,
}

pub struct QQMusic {
    translation: ChineseTranslation,
}

impl QQMusic {
    pub fn create(params: &ProviderParams) -> Box<dyn LyricsProvider> {
        Box::new(Self {
            translation: ChineseTranslation::from_params(params),
        })
    }

    fn search(&self, track: &TrackInfo) -> ProviderResult<Vec<Song>> {
        let query = format!(
            "{} {}",
            track.title,
            track.first_artist().unwrap_or_default()
        );
        let param = SearchParam::new(&query);

        with_fallback(
            "search",
            || {
                self.request(LITE_URL, SEARCH_MODULE, LITE_SEARCH_METHOD, &param)
                    .map(|data: LiteSearchData| data.body.item_song)
            },
            || {
                self.request(DESKTOP_URL, SEARCH_MODULE, DESKTOP_SEARCH_METHOD, &param)
                    .map(|data: DesktopSearchData| data.body.song.list)
            },
        )
    }

    fn get(&self, song: &Song, translation: bool) -> ProviderResult<Option<SongLyrics>> {
        let param = LyricParam::new(song, translation);
        let data: LyricData = with_fallback(
            "lyric",
            || self.request(LITE_URL, LYRIC_MODULE, LYRIC_METHOD, &param),
            || self.request(DESKTOP_URL, LYRIC_MODULE, LYRIC_METHOD, &param),
        )?;

        if data.lyric.is_empty() {
            info!("qqmusic: song {} has no lyrics attached", song.id);
            return Ok(None);
        }

        let xml = match qrc::decrypt(&data.lyric) {
            Ok(xml) => xml,
            Err(e) => {
                warn!(
                    "qqmusic: failed to decrypt the lyrics for song {} ({} hex chars): {e}",
                    song.id,
                    data.lyric.len()
                );
                return Ok(None);
            }
        };

        let Some(content) = qrc::extract_lyric_content(&xml) else {
            warn!(
                "qqmusic: the decrypted payload for song {} carries no LyricContent",
                song.id
            );
            return Ok(None);
        };

        Ok(Some(SongLyrics {
            content,
            translation: decrypt_translation(song, &data.trans),
        }))
    }

    fn request<P: Serialize, T: DeserializeOwned>(
        &self,
        endpoint: &str,
        module: &str,
        method: &str,
        param: &P,
    ) -> ProviderResult<T> {
        let body = serde_json::to_vec(&serde_json::json!({
            "request": {
                "module": module,
                "method": method,
                "param": param,
            },
        }))
        .map_err(|e| ProviderError::other(format!("failed to encode the request: {e}")))?;

        let response = Http::post(endpoint)
            .browser()
            .header("Content-Type", "application/json")
            .body(body)
            .send()?;

        if response.status != 200 {
            return Err(response.unexpected_status("the API"));
        }

        let parsed: ApiResponse<T> = response.json("API")?;
        check_code(parsed.code, module, method, "gateway")?;

        let result = parsed
            .request
            .ok_or_else(|| missing(module, method, "'request' object"))?;
        check_code(result.code, module, method, "module")?;

        result
            .data
            .ok_or_else(|| missing(module, method, "'data' object"))
    }
}

impl LyricsProvider for QQMusic {
    fn supported_kinds(&self) -> &'static [LyricsKind] {
        &[LyricsKind::Lrc, LyricsKind::Elrc]
    }

    fn log_params(&self) -> Vec<(&'static str, String)> {
        self.translation.log_params()
    }

    fn fetch_lyrics(
        &self,
        track: &TrackInfo,
        cfg: &PluginConfig,
    ) -> ProviderResult<Option<ProviderLyrics>> {
        if !track.has_artist() {
            return Err(ProviderError::other("track has no artist"));
        }

        let song = match self
            .search(track)?
            .into_iter()
            .filter(|song| {
                track.matches_duration(Duration::from_secs(song.interval), cfg.duration_tolerance)
            })
            .find(|song| !cfg.require_album_match || track.matches_album(&song.album.name))
        {
            Some(song) => song,
            None => return Ok(None),
        };

        let SongLyrics {
            content,
            translation,
        } = match self.get(&song, self.translation.wanted(cfg))? {
            Some(lyrics) => lyrics,
            None => return Ok(None),
        };

        let is_qrc = qrc::is_qrc(&content);

        if !is_qrc && lrc::is_instrumental(&content) {
            info!("qqmusic: track is instrumental");
            return Ok(Some(Lyrics::Instrumental.into()));
        }

        for &kind in &cfg.lyrics_type_priority {
            match kind {
                LyricsKind::Elrc if is_qrc => {
                    let elrc = qrc::to_enhanced_lrc(&content);

                    if !elrc.trim().is_empty() {
                        return Ok(Some(
                            self.translation
                                .add(cfg, Lyrics::Elrc(elrc), || translation),
                        ));
                    }
                }

                LyricsKind::Lrc => {
                    let lrc = if is_qrc {
                        qrc::to_lrc(&content)
                    } else {
                        content.clone()
                    };

                    if !lrc.trim().is_empty() {
                        return Ok(Some(
                            self.translation.add(cfg, Lyrics::Lrc(lrc), || translation),
                        ));
                    }
                }

                _ => {}
            }
        }

        Ok(None)
    }
}

fn decrypt_translation(song: &Song, hex: &str) -> Option<String> {
    if hex.is_empty() {
        return None;
    }

    match qrc::decrypt(hex) {
        Ok(lrc) => Some(strip_untranslated(&lrc)),
        Err(e) => {
            warn!(
                "qqmusic: failed to decrypt the translation for song {}: {e}",
                song.id
            );
            None
        }
    }
}

fn strip_untranslated(lrc: &str) -> String {
    let placeholder = Regex::new(r"^(?:\[[^\]]*\])+\s*//\s*$").unwrap();

    lrc.lines()
        .filter(|line| !placeholder.is_match(line.trim()))
        .collect::<Vec<_>>()
        .join("\n")
}

fn with_fallback<T>(
    what: &str,
    primary: impl FnOnce() -> ProviderResult<T>,
    fallback: impl FnOnce() -> ProviderResult<T>,
) -> ProviderResult<T> {
    primary().or_else(|e| {
        info!("qqmusic: {what} failed on the primary endpoint ({e}), trying the fallback");
        fallback()
    })
}

fn check_code(code: Option<i64>, module: &str, method: &str, level: &str) -> ProviderResult<()> {
    match code {
        Some(0) => Ok(()),
        Some(2001) => Err(ProviderError::rate_limited(None)),
        Some(code) => Err(ProviderError::other(format!(
            "{module}.{method} returned {level} error code {code}"
        ))),
        None => Err(missing(module, method, &format!("{level} 'code' field"))),
    }
}

fn missing(module: &str, method: &str, what: &str) -> ProviderError {
    ProviderError::other(format!("{module}.{method} response is missing the {what}"))
}
