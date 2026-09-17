use std::time::Duration;

use crate::{
    config::{PluginConfig, ProviderParams},
    ext::TrackInfoExt,
    format::lrc,
    providers::{LyricsProvider, ProviderResult, error::ProviderError, http::Http},
    types::{Lyrics, LyricsKind},
};
use extism_pdk::info;
use nd_pdk::lyrics::TrackInfo;
use serde::Deserialize;

mod yrc;

const SEARCH_URL: &str = "https://music.163.com/api/search/get";
const LYRICS_URL: &str = "https://music.163.com/api/song/lyric/v1";

#[derive(Debug, Deserialize)]
struct SearchResponse {
    result: SearchResult,
}

#[derive(Debug, Deserialize)]
struct SearchResult {
    #[serde(default)]
    songs: Vec<Song>,
}

#[derive(Debug, Deserialize)]
struct Song {
    id: u64,
    #[serde(rename = "duration")]
    duration_ms: Option<u64>,
    #[serde(default)]
    album: Album,
}

#[derive(Debug, Default, Deserialize)]
struct Album {
    #[serde(default)]
    name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LyricsResponse {
    lrc: Option<LyricContent>,
    yrc: Option<LyricContent>,
    #[serde(default)]
    pure_music: bool,
}

#[derive(Debug, Deserialize)]
struct LyricContent {
    lyric: Option<String>,
}

impl LyricContent {
    fn text(content: &Option<LyricContent>) -> Option<&str> {
        content
            .as_ref()
            .and_then(|c| c.lyric.as_deref())
            .map(str::trim)
            .filter(|s| !s.is_empty())
    }
}

pub struct NetEase;

impl NetEase {
    pub fn create(_params: &ProviderParams) -> Box<dyn LyricsProvider> {
        Box::new(Self)
    }

    fn search(&self, track: &TrackInfo) -> ProviderResult<SearchResponse> {
        let query = format!(
            "{} {}",
            track.first_artist().unwrap_or_default(),
            track.title
        );

        let response = Http::get(SEARCH_URL)
            .browser()
            .header("Referer", "https://music.163.com")
            .param("s", query)
            .param("type", "1")
            .param("limit", "20")
            .param("offset", "0")
            .send()?;

        match response.status {
            200 => response.json("search"),
            429 => Err(response.rate_limited()),
            _ => Err(response.unexpected_status("the search endpoint")),
        }
    }

    fn get(&self, id: u64) -> ProviderResult<LyricsResponse> {
        let response = Http::get(LYRICS_URL)
            .browser()
            .header("Referer", "https://music.163.com")
            .param("id", id.to_string())
            .param("lv", "-1")
            .param("kv", "-1")
            .param("tv", "-1")
            .param("yv", "1")
            .send()?;

        match response.status {
            200 => response.json("the lyrics"),
            429 => Err(response.rate_limited()),
            _ => Err(response.unexpected_status("the lyrics endpoint")),
        }
    }
}

impl LyricsProvider for NetEase {
    fn supported_kinds(&self) -> &'static [LyricsKind] {
        // Even though NetEase can sometimes return plain lyrics in the "lrc" field,
        // that is considered a problem on their side, so plain lyrics are not officially
        // supported by this provider.
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
            .search(track)?
            .result
            .songs
            .into_iter()
            .filter(|record| {
                record.duration_ms.is_some_and(|d| {
                    track.matches_duration(Duration::from_millis(d), cfg.duration_tolerance)
                })
            })
            .find(|record| !cfg.require_album_match || track.matches_album(&record.album.name))
        {
            Some(song) => song,
            None => return Ok(None),
        };

        let response = self.get(song.id)?;

        if response.pure_music {
            info!("netease: track is instrumental");
            return Ok(Some(Lyrics::Instrumental));
        }

        let mut lrc = LyricContent::text(&response.lrc).map(strip_metadata);

        for &kind in &cfg.lyrics_type_priority {
            match kind {
                LyricsKind::Elrc => {
                    if let Some(raw) = LyricContent::text(&response.yrc) {
                        let elrc = yrc::to_enhanced_lrc(raw);

                        if !elrc.trim().is_empty() {
                            return Ok(Some(Lyrics::Elrc(elrc)));
                        }
                    }
                }

                LyricsKind::Lrc => {
                    if let Some(text) = lrc.take_if(|text| lrc::is_synced(text)) {
                        return Ok(Some(Lyrics::Lrc(text)));
                    }
                }

                LyricsKind::Plain => {
                    if let Some(text) = lrc.take_if(|text| !lrc::is_synced(text)) {
                        return Ok(Some(Lyrics::Plain(text)));
                    }
                }
                _ => {}
            }
        }

        Ok(None)
    }
}

fn strip_metadata(lyric: &str) -> String {
    lyric
        .lines()
        .filter(|line| !line.trim_start().starts_with('{'))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_metadata_removes_json_lines() {
        let input = concat!(
            "{\"t\":0,\"c\":[{\"tx\":\"foo: \"}]}\n",
            "{\"t\":1000,\"c\":[{\"tx\":\"bar: \"}]}\n",
            "[00:28.15]foo\n",
            "[00:32.38]bar"
        );
        assert_eq!(strip_metadata(input), "[00:28.15]foo\n[00:32.38]bar");
    }

    #[test]
    fn strip_metadata_keeps_plain_lyrics() {
        let input = "Hello\nWorld";
        assert_eq!(strip_metadata(input), "Hello\nWorld");
    }

    #[test]
    fn strip_metadata_empty() {
        assert_eq!(strip_metadata(""), "");
    }
}
