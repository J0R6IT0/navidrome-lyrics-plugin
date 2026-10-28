use crate::{
    config::PluginConfig,
    providers::{
        error::ProviderResult,
        services::{
            AppleMusic, BiniLyrics, Genie, KuGou, LrcRed, Lrclib, Lrcmux, LyricsOvh, NetEase,
            QQMusic, Stixoi,
        },
    },
    types::{Lyrics, LyricsKind, LyricsTrack, TrackKind},
};
use nd_pdk::lyrics::TrackInfo;

pub mod error;
mod http;
mod registry;
mod services;

pub use registry::ProviderRegistry;

const USER_AGENT: &str = concat!(
    "navidrome-lyrics-plugin/",
    env!("CARGO_PKG_VERSION"),
    " (https://github.com/J0R6IT0/navidrome-lyrics-plugin)"
);

const BROWSER_USER_AGENT: &str =
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 15.7; rv:150.0) Gecko/20100101 Firefox/150.0";

pub fn register_providers(registry: &mut ProviderRegistry) {
    registry.register("lrclib", Lrclib::create);
    registry.register("lyrics.ovh", LyricsOvh::create);
    registry.register("lrcmux", Lrcmux::create);
    registry.register("kugou", KuGou::create);
    registry.register("netease", NetEase::create);
    registry.register("qqmusic", QQMusic::create);
    registry.register("applemusic", AppleMusic::create);
    registry.register("stixoi", Stixoi::create);
    registry.register("genie", Genie::create);
    registry.register("binilyrics", BiniLyrics::create);
    registry.register("lrc.red", LrcRed::create);
}

/// Lyrics returned by a provider. Can have optional translation and transliteration
/// tracks that will be merged into a single TTML.
pub struct ProviderLyrics {
    pub lyrics: Lyrics,
    pub tracks: Vec<LyricsTrack>,
}

#[allow(dead_code)]
impl ProviderLyrics {
    /// Adds a translation track.
    fn translation(mut self, lrc: String, lang: String) -> Self {
        self.tracks.push(LyricsTrack {
            kind: TrackKind::Translation,
            lang,
            lrc,
        });
        self
    }

    /// Adds a transliteration track.
    fn transliteration(mut self, lrc: String, lang: String) -> Self {
        self.tracks.push(LyricsTrack {
            kind: TrackKind::Transliteration,
            lang,
            lrc,
        });
        self
    }
}

impl From<Lyrics> for ProviderLyrics {
    fn from(lyrics: Lyrics) -> Self {
        Self {
            lyrics,
            tracks: Vec::new(),
        }
    }
}

pub trait LyricsProvider {
    fn supported_kinds(&self) -> &'static [LyricsKind];

    fn fetch_lyrics(
        &self,
        track: &TrackInfo,
        cfg: &PluginConfig,
    ) -> ProviderResult<Option<ProviderLyrics>>;

    /// Configuration parameters worth including in logs, as `(key, value)` pairs.
    /// Never return a token, cookie or any other credential here.
    fn log_params(&self) -> Vec<(&'static str, String)> {
        Vec::new()
    }
}
