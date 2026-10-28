use crate::{
    config::{PluginConfig, ProviderParams},
    ext::TrackInfoExt,
    providers::{LyricsProvider, ProviderLyrics, ProviderResult, http::Http},
    types::{Lyrics, LyricsKind},
};
use nd_pdk::lyrics::TrackInfo;
use serde::Deserialize;
use std::time::Duration;

const BASE_URL: &str = "https://lrc.red";

#[derive(Debug, Deserialize)]
struct SearchResponse {
    #[serde(default)]
    hits: Vec<Hit>,
}

#[derive(Debug, Deserialize)]
struct Hit {
    isrc: String,
    #[serde(default)]
    artist: String,
    #[serde(default)]
    album: Option<String>,
    duration: Option<f32>,
}

pub struct LrcRed;

impl LrcRed {
    pub fn create(_: &ProviderParams) -> Box<dyn LyricsProvider> {
        Box::new(Self)
    }

    fn search(&self, track: &TrackInfo) -> ProviderResult<Vec<Hit>> {
        let query = match track.first_artist() {
            Some(artist) => format!("{} {}", artist, track.clean_title()),
            None => track.clean_title(),
        };

        let response = Http::get(format!("{BASE_URL}/search.json"))
            .param("q", query)
            .send()?;

        match response.status {
            200 => response.json::<SearchResponse>("search").map(|r| r.hits),
            404 => Ok(Vec::new()),
            429 => Err(response.rate_limited()),
            _ => Err(response.unexpected_status("the search endpoint")),
        }
    }

    fn fetch_file(&self, isrc: &str, cfg: &PluginConfig) -> ProviderResult<Option<Lyrics>> {
        for kind in &cfg.lyrics_type_priority {
            let (extension, wrap): (&str, fn(String) -> Lyrics) = match kind {
                LyricsKind::Ttml => ("ttml", Lyrics::Ttml),
                LyricsKind::Lyricsfile => ("lyricsfile.yaml", Lyrics::Lyricsfile),
                LyricsKind::Lrc => ("lrc", Lyrics::Lrc),
                _ => continue,
            };

            let response = Http::get(format!("{BASE_URL}/s/{isrc}.{extension}")).send()?;

            match response.status {
                200 => {
                    let text = response.text();
                    if !text.trim().is_empty() {
                        return Ok(Some(wrap(text.into_owned())));
                    }
                }
                404 => {}
                429 => return Err(response.rate_limited()),
                _ => return Err(response.unexpected_status("the lyrics file")),
            }
        }

        Ok(None)
    }
}

impl LyricsProvider for LrcRed {
    fn supported_kinds(&self) -> &'static [LyricsKind] {
        &[LyricsKind::Ttml, LyricsKind::Lyricsfile, LyricsKind::Lrc]
    }

    fn fetch_lyrics(
        &self,
        track: &TrackInfo,
        cfg: &PluginConfig,
    ) -> ProviderResult<Option<ProviderLyrics>> {
        match self
            .search(track)?
            .into_iter()
            .find(|hit| matches(track, cfg, hit))
        {
            Some(hit) => Ok(self.fetch_file(&hit.isrc, cfg)?.map(Into::into)),
            None => Ok(None),
        }
    }
}

fn matches(track: &TrackInfo, cfg: &PluginConfig, hit: &Hit) -> bool {
    hit.duration.is_some_and(|d| {
        Duration::try_from_secs_f32(d)
            .is_ok_and(|d| track.matches_duration(d, cfg.duration_tolerance))
    }) && artist_matches(track, &hit.artist)
        && (!cfg.require_album_match || track.matches_album(hit.album.as_deref().unwrap_or("")))
}

fn artist_matches(track: &TrackInfo, artist: &str) -> bool {
    if !track.has_artist() {
        return true;
    }

    let artist = artist.to_lowercase();
    track.artists.iter().any(|a| {
        let name = a.name.trim().to_lowercase();
        !name.is_empty() && (artist.contains(&name) || name.contains(&artist))
    })
}
