use crate::{
    config::ProviderParams,
    ext::TrackInfoExt,
    providers::{LyricsProvider, error::ProviderError, http::Http},
    types::LyricsKind,
};
use serde::Deserialize;

const BASE_URL: &str = "https://unison.boidu.dev";

// todo(@Suya1671): investigate actually parsing using the `success` field,
// Currently not doing as serde can't tag using a boolean for some reason.
//
// Closest option is https://docs.rs/serde-bool/latest/serde_bool,
// but that's another dependency.
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
// abridged, see https://unison.betterlyrics.org/docs for full API.
pub struct LyricsData {
    pub format: LyricsFormat,
    pub sync_type: SyncType,
    pub album: String,
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
        // under https://unison.betterlyrics.org/docs Formats
        // note(@Suya1671, @J0R6IT0): lrc + richsync => Elrc, https://github.com/J0R6IT0/navidrome-lyrics-plugin/pull/48#discussion_r4388801391
        &[
            LyricsKind::Ttml,
            LyricsKind::Lrc,
            LyricsKind::Elrc,
            LyricsKind::Plain,
        ]
    }

    fn fetch_lyrics(
        &self,
        track: &nd_pdk::lyrics::TrackInfo,
        cfg: &crate::config::PluginConfig,
    ) -> crate::providers::error::ProviderResult<Option<crate::types::Lyrics>> {
        let mut request = Http::get(format!("{BASE_URL}/lyrics"))
            .param("song", &track.title)
            .param("artist", track.all_artists())
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

        if cfg.require_album_match && data.album != track.album {
            return Ok(None);
        }

        let lyrics = match data.format {
            LyricsFormat::Ttml => crate::types::Lyrics::Ttml(data.lyrics),
            // ref: https://github.com/J0R6IT0/navidrome-lyrics-plugin/pull/48#discussion_r4388801391
            LyricsFormat::Lrc if data.sync_type == SyncType::Rich => {
                crate::types::Lyrics::Elrc(data.lyrics)
            }
            LyricsFormat::Lrc => crate::types::Lyrics::Lrc(data.lyrics),
            LyricsFormat::Plain => crate::types::Lyrics::Plain(data.lyrics),
        };

        Ok(Some(lyrics))
    }
}
