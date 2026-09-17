use std::time::Duration;

use crate::{
    config::{PluginConfig, ProviderParams},
    ext::TrackInfoExt,
    format::lrc,
    providers::{LyricsProvider, ProviderResult, error::ProviderError, http::Http},
    types::{Lyrics, LyricsKind},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use extism_pdk::{info, warn};
use nd_pdk::lyrics::TrackInfo;
use serde::Deserialize;

mod krc;

const SONG_SEARCH_URL: &str = "http://mobilecdn.kugou.com/api/v3/search/song";
const LYRICS_SEARCH_URL: &str = "https://lyrics.kugou.com/search";
const DOWNLOAD_URL: &str = "https://lyrics.kugou.com/download";

#[derive(Debug, Deserialize)]
struct SongSearchResponse {
    data: SongSearchData,
}

#[derive(Debug, Deserialize)]
struct SongSearchData {
    #[serde(default)]
    info: Vec<SongInfo>,
}

#[derive(Debug, Deserialize)]
struct SongInfo {
    hash: String,
    duration: Option<u64>,
    trans_param: Option<TransParam>,
    #[serde(default)]
    album_name: String,
}

#[derive(Debug, Deserialize)]
struct TransParam {
    #[serde(default)]
    language: String,
}

#[derive(Debug, Deserialize)]
struct LyricsSearchResponse {
    // TODO: investigate further if this is needed
    // status: u32,
    #[serde(default)]
    candidates: Vec<Candidate>,
}

#[derive(Debug, Deserialize)]
struct Candidate {
    id: String,
    accesskey: String,
    #[serde(default)]
    krctype: i64,
}

#[derive(Debug, Deserialize)]
struct DownloadResponse {
    content: String,
}

pub struct KuGou;

impl KuGou {
    pub fn create(_params: &ProviderParams) -> Box<dyn LyricsProvider> {
        Box::new(Self)
    }

    fn search_song(&self, track: &TrackInfo) -> ProviderResult<SongSearchResponse> {
        let keyword = format!(
            "{} {}",
            track.title,
            track.first_artist().unwrap_or_default()
        );

        let response = Http::get(SONG_SEARCH_URL)
            .browser()
            .param("format", "json")
            .param("keyword", keyword)
            .param("page", "1")
            .param("pagesize", "10")
            .param("showtype", "1")
            .send()?;

        match response.status {
            200 => response.json("the song search"),
            429 => Err(response.rate_limited()),
            _ => Err(response.unexpected_status("song search endpoint")),
        }
    }

    fn search_lyrics(&self, song: &SongInfo) -> ProviderResult<LyricsSearchResponse> {
        let response = Http::get(LYRICS_SEARCH_URL)
            .browser()
            .param("ver", "1")
            .param("man", "yes")
            .param("client", "mobi")
            .param("keyword", "")
            .param("duration", song.duration.unwrap_or(0).to_string())
            .param("hash", &song.hash)
            .param("album_audio_id", "")
            .send()?;

        match response.status {
            200 => response.json("the lyrics search"),
            429 => Err(response.rate_limited()),
            _ => Err(response.unexpected_status("lyrics search endpoint")),
        }
    }

    fn download(&self, candidate: &Candidate, fmt: &str) -> ProviderResult<Vec<u8>> {
        let response = Http::get(DOWNLOAD_URL)
            .browser()
            .param("ver", "1")
            .param("client", "pc")
            .param("id", &candidate.id)
            .param("accesskey", &candidate.accesskey)
            .param("fmt", fmt)
            .param("charset", "utf8")
            .send()?;

        match response.status {
            200 => {}
            429 => return Err(response.rate_limited()),
            _ => return Err(response.unexpected_status("download endpoint")),
        }

        let encoded: DownloadResponse = response.json("the download")?;

        STANDARD
            .decode(&encoded.content)
            .map_err(|e| ProviderError::other(format!("failed to decode lyrics content: {e}")))
    }
}

impl LyricsProvider for KuGou {
    fn supported_kinds(&self) -> &'static [LyricsKind] {
        &[LyricsKind::Lrc, LyricsKind::Elrc]
    }

    fn fetch_lyrics(
        &self,
        track: &TrackInfo,
        cfg: &PluginConfig,
    ) -> ProviderResult<Option<Lyrics>> {
        if !track.has_artist() {
            return Err(ProviderError::other("track has no artist"));
        }

        let song = match self
            .search_song(track)?
            .data
            .info
            .into_iter()
            .filter(|record| {
                record.duration.is_some_and(|d| {
                    track.matches_duration(Duration::from_secs(d), cfg.duration_tolerance)
                })
            })
            .find(|record| !cfg.require_album_match || track.matches_album(&record.album_name))
        {
            Some(song) => song,
            None => return Ok(None),
        };

        if song
            .trans_param
            .as_ref()
            .is_some_and(|p| p.language == "纯音乐")
        {
            info!("kugou: track is instrumental");
            return Ok(Some(Lyrics::Instrumental));
        }

        let candidate = match self.search_lyrics(&song)?.candidates.into_iter().next() {
            Some(lyrics) => lyrics,
            None => return Ok(None),
        };

        for &kind in &cfg.lyrics_type_priority {
            match kind {
                LyricsKind::Elrc if candidate.krctype != 0 => {
                    let bytes = self.download(&candidate, "krc")?;
                    match krc::to_enhanced_lrc(&bytes) {
                        Ok(elrc) if !elrc.trim().is_empty() => {
                            return Ok(Some(Lyrics::Elrc(elrc)));
                        }
                        Ok(_) => {}
                        Err(e) => warn!("kugou: failed to decode krc lyrics: {e}"),
                    }
                }

                LyricsKind::Lrc => {
                    let bytes = self.download(&candidate, "lrc")?;
                    let lrc = String::from_utf8(bytes).map_err(|e| {
                        ProviderError::other(format!("lyrics content is not valid UTF-8: {e}"))
                    })?;

                    if lrc::is_instrumental(&lrc) {
                        warn!("kugou: track was not marked as instrumental and is instrumental");
                        return Ok(Some(Lyrics::Instrumental));
                    }
                    if !lrc.trim().is_empty() {
                        return Ok(Some(Lyrics::Lrc(lrc)));
                    }
                }

                _ => {}
            }
        }

        Ok(None)
    }
}
