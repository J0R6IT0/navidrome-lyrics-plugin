use crate::{
    config::{PluginConfig, ProviderParams},
    ext::TrackInfoExt,
    providers::{
        LyricsProvider, ProviderLyrics,
        error::{ProviderError, ProviderResult},
        http::Http,
    },
    types::{Lyrics, LyricsKind},
};
use nd_pdk::lyrics::TrackInfo;
use serde::Deserialize;

const BASE_URL: &str = "https://unison.boidu.dev";

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum Record {
    Success {
        data: LyricsData,
    },

    Failure {
        error: String,
        code: String,
        hint: String,
    },
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LyricsData {
    pub format: LyricsFormat,
    pub sync_type: SyncType,
    pub album: Option<String>,
    pub lyrics: String,
}

#[derive(Debug, Deserialize)]
pub enum LyricsFormat {
    #[serde(rename = "ttml")]
    Ttml,
    #[serde(rename = "lrc")]
    Lrc,
    #[serde(rename = "plain")]
    Plain,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
pub enum SyncType {
    #[serde(rename = "plain")]
    Plain,
    #[serde(rename = "linesync")]
    Line,
    #[serde(rename = "richsync")]
    Rich,
}

pub struct Unison;

impl Unison {
    pub fn create(_params: &ProviderParams) -> Box<dyn LyricsProvider> {
        Box::new(Self)
    }
}

impl LyricsProvider for Unison {
    fn supported_kinds(&self) -> &'static [LyricsKind] {
        &[
            LyricsKind::Ttml,
            LyricsKind::Lrc,
            LyricsKind::Elrc,
            LyricsKind::Plain,
        ]
    }

    fn fetch_lyrics(
        &self,
        track: &TrackInfo,
        cfg: &PluginConfig,
    ) -> ProviderResult<Option<ProviderLyrics>> {
        let mut request = Http::get(format!("{BASE_URL}/lyrics"))
            .param("song", &track.title)
            .param("artist", track.all_artists(", "))
            .param("duration", track.duration.to_string());

        if cfg.require_album_match {
            request = request.param("album", &track.album);
        }

        let response = request.send()?;

        let record = match response.status {
            200 => response.json::<Record>("Unison Lyrics API response")?,
            404 => return Ok(None),
            429 => return Err(response.rate_limited()),
            _ => return Err(response.unexpected_status("Unison Lyrics API")),
        };

        let data = match record {
            Record::Success { data } => data,
            Record::Failure { error, code, hint } => {
                return Err(ProviderError::Other(format!(
                    "Unison Lyrics API error: {} (code: {}, hint: {})",
                    error, code, hint
                )));
            }
        };

        if cfg.require_album_match && !track.matches_album(data.album.as_deref().unwrap_or("")) {
            return Ok(None);
        }

        let lyrics = match data.format {
            LyricsFormat::Ttml => Lyrics::Ttml(data.lyrics),
            LyricsFormat::Lrc if data.sync_type == SyncType::Rich => Lyrics::Elrc(data.lyrics),
            LyricsFormat::Lrc => Lyrics::Lrc(data.lyrics),
            LyricsFormat::Plain => Lyrics::Plain(data.lyrics),
        };

        Ok(Some(lyrics.into()))
    }
}
