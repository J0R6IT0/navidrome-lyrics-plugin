use crate::{
    config::{PluginConfig, ProviderParams},
    ext::TrackInfoExt,
    providers::{LyricsProvider, ProviderResult, http::Http},
    types::{Lyrics, LyricsKind},
};
use nd_pdk::lyrics::TrackInfo;
use serde::Deserialize;
use std::time::Duration;

const BASE_URL: &str = "https://lyrics-api.binimum.org";

#[derive(Debug, Deserialize)]
struct SearchResponse {
    #[serde(default)]
    results: Vec<Record>,
}

#[derive(Debug, Deserialize)]
struct Record {
    #[serde(default)]
    album_name: String,
    duration: Option<u32>,
    #[serde(rename = "lyricsUrl")]
    lyrics_url: String,
}

pub struct BiniLyrics;

impl BiniLyrics {
    pub fn create(_: &ProviderParams) -> Box<dyn LyricsProvider> {
        Box::new(Self)
    }

    fn track_lookup(&self, track: &TrackInfo) -> ProviderResult<Option<Record>> {
        let response = Http::get(BASE_URL)
            .param("track", track.title.as_str())
            .param("artist", track.all_artists())
            // Album is not enforced server-side.
            .param("album", track.album.as_str())
            .param("duration", track.duration().as_secs().to_string())
            .send()?;

        match response.status {
            200 => response
                .json::<SearchResponse>("track lookup")
                .map(|r| r.results.into_iter().next()),
            404 => Ok(None),
            429 => Err(response.rate_limited()),
            _ => Err(response.unexpected_status("the track lookup endpoint")),
        }
    }

    fn search(&self, track: &TrackInfo) -> ProviderResult<Vec<Record>> {
        let query = match track.first_artist() {
            Some(artist) => format!("{} {}", artist, track.clean_title()),
            None => track.clean_title(),
        };

        let response = Http::get(BASE_URL).param("q", query).send()?;

        match response.status {
            200 => response.json::<SearchResponse>("search").map(|r| r.results),
            404 => Ok(Vec::new()),
            429 => Err(response.rate_limited()),
            _ => Err(response.unexpected_status("the search endpoint")),
        }
    }

    fn fetch_ttml(&self, url: &str) -> ProviderResult<Option<Lyrics>> {
        let response = Http::get(url).send()?;

        match response.status {
            200 => {
                let text = response.text();
                Ok((!text.trim().is_empty()).then(|| Lyrics::Ttml(text.into_owned())))
            }
            404 => Ok(None),
            429 => Err(response.rate_limited()),
            _ => Err(response.unexpected_status("the lyrics file")),
        }
    }
}

impl LyricsProvider for BiniLyrics {
    fn supported_kinds(&self) -> &'static [LyricsKind] {
        &[LyricsKind::Ttml]
    }

    fn fetch_lyrics(
        &self,
        track: &TrackInfo,
        cfg: &PluginConfig,
    ) -> ProviderResult<Option<Lyrics>> {
        if track.has_artist()
            && let Some(record) = self.track_lookup(track)?.filter(|r| matches(track, cfg, r))
        {
            return self.fetch_ttml(&record.lyrics_url);
        }

        match self
            .search(track)?
            .into_iter()
            .find(|record| matches(track, cfg, record))
        {
            Some(record) => self.fetch_ttml(&record.lyrics_url),
            None => Ok(None),
        }
    }
}

fn matches(track: &TrackInfo, cfg: &PluginConfig, record: &Record) -> bool {
    record.duration.is_some_and(|d| {
        track.matches_duration(Duration::from_secs(d.into()), cfg.duration_tolerance)
    }) && (!cfg.require_album_match || track.matches_album(&record.album_name))
}
