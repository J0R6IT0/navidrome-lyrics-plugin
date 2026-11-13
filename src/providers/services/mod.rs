mod applemusic;
mod genie;
mod kugou;
mod lrclib;
mod lrcmux;
mod lrcred;
mod lyricsovh;
mod netease;
mod qqmusic;
mod stixoi;
mod unison;

pub use applemusic::*;
pub use genie::*;
pub use kugou::*;
pub use lrclib::*;
pub use lrcmux::*;
pub use lrcred::*;
pub use lyricsovh::*;
pub use netease::*;
pub use qqmusic::*;
pub use stixoi::*;
pub use unison::*;

use crate::{
    config::{PluginConfig, ProviderParams},
    providers::ProviderLyrics,
    types::{Lyrics, LyricsKind},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ChineseTranslation(bool);

impl ChineseTranslation {
    const PARAM: &str = "includeChineseTranslation";
    const LANG: &str = "zh-Hans";

    fn from_params(params: &ProviderParams) -> Self {
        Self(params.flag(Self::PARAM))
    }

    fn wanted(self, cfg: &PluginConfig) -> bool {
        self.0 && cfg.wants(LyricsKind::Ttml)
    }

    fn add(
        self,
        cfg: &PluginConfig,
        lyrics: Lyrics,
        translation: impl FnOnce() -> Option<String>,
    ) -> ProviderLyrics {
        let lyrics = ProviderLyrics::from(lyrics);
        if !self.wanted(cfg) {
            return lyrics;
        }

        match translation() {
            Some(lrc) => lyrics.translation(lrc, Self::LANG.to_string()),
            None => lyrics,
        }
    }

    fn log_params(self) -> Vec<(&'static str, String)> {
        if self.0 {
            vec![(Self::PARAM, "true".to_string())]
        } else {
            Vec::new()
        }
    }
}
