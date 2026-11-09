use crate::{
    config::{PluginConfig, ProviderParams},
    ext::TrackInfoExt,
    providers::{LyricsProvider, ProviderLyrics, ProviderResult, error::ProviderError, http::Http},
    types::{Lyrics, LyricsKind},
};
use nd_pdk::lyrics::TrackInfo;
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use serde::Deserialize;

const DEFAULT_BASE_URL: &str = "https://api.lyrics.ovh";

#[derive(Debug, Deserialize)]
struct ApiResponse {
    lyrics: String,
}

pub struct LyricsOvh {
    base_url: String,
}

impl LyricsOvh {
    pub fn create(params: &ProviderParams) -> Box<dyn LyricsProvider> {
        Box::new(Self {
            base_url: params
                .get("baseUrl")
                .unwrap_or(DEFAULT_BASE_URL)
                .to_string(),
        })
    }

    fn get(&self, track: &TrackInfo) -> ProviderResult<Option<ApiResponse>> {
        let encode = |s: &str| utf8_percent_encode(s, NON_ALPHANUMERIC).to_string();
        let url = format!(
            "{}/v1/{}/{}",
            self.base_url,
            encode(track.first_artist().unwrap_or_default()),
            encode(&track.title)
        );

        let response = Http::get(url).send()?;

        match response.status {
            200 => response.json("get").map(Some),
            429 => Err(response.rate_limited()),
            404 => Ok(None),
            _ => Err(response.unexpected_status("lyrics.ovh")),
        }
    }
}

impl LyricsProvider for LyricsOvh {
    fn supported_kinds(&self) -> &'static [LyricsKind] {
        &[LyricsKind::Plain]
    }

    fn log_params(&self) -> Vec<(&'static str, String)> {
        vec![("baseUrl", self.base_url.clone())]
    }

    fn fetch_lyrics(
        &self,
        track: &TrackInfo,
        _cfg: &PluginConfig,
    ) -> ProviderResult<Option<ProviderLyrics>> {
        if !track.has_artist() {
            return Err(ProviderError::other("track has no artist"));
        }

        Ok(self
            .get(track)?
            .map(|response| Lyrics::Plain(response.lyrics).into()))
    }
}
