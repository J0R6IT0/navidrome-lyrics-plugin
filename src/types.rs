use crate::{
    config::PluginConfig,
    format::{self, censor, lrc, lyricsfile, ttml},
};
use nd_pdk::lyrics::{GetLyricsResponse, LyricsText};
use std::borrow::Cow;

#[derive(PartialEq, Debug)]
pub enum Lyrics {
    Plain(String),
    Lrc(String),
    Elrc(String),
    Ttml(String),
    Srt(String),
    Lyricsfile(String),
    Instrumental,
}

impl Lyrics {
    pub fn kind(&self) -> LyricsKind {
        match self {
            Lyrics::Plain(_) => LyricsKind::Plain,
            Lyrics::Lrc(_) => LyricsKind::Lrc,
            Lyrics::Elrc(_) => LyricsKind::Elrc,
            Lyrics::Ttml(_) => LyricsKind::Ttml,
            Lyrics::Srt(_) => LyricsKind::Srt,
            Lyrics::Lyricsfile(_) => LyricsKind::Lyricsfile,
            Lyrics::Instrumental => LyricsKind::Instrumental,
        }
    }

    pub fn sync_level(&self) -> SyncLevel {
        match self {
            Lyrics::Plain(_) | Lyrics::Instrumental => SyncLevel::Plain,
            Lyrics::Lrc(_) | Lyrics::Srt(_) => SyncLevel::Line,
            Lyrics::Elrc(_) => SyncLevel::Word,
            Lyrics::Ttml(s) => ttml::sync_level(s),
            Lyrics::Lyricsfile(s) => lyricsfile::sync_level(s),
        }
    }

    pub fn is_censored(&self) -> bool {
        match self {
            Lyrics::Plain(s)
            | Lyrics::Lrc(s)
            | Lyrics::Elrc(s)
            | Lyrics::Ttml(s)
            | Lyrics::Srt(s)
            | Lyrics::Lyricsfile(s) => censor::is_censored(s),
            Lyrics::Instrumental => false,
        }
    }

    pub fn text<'a>(&'a self, cfg: &'a PluginConfig) -> Cow<'a, str> {
        match self {
            Lyrics::Plain(s)
            | Lyrics::Lrc(s)
            | Lyrics::Elrc(s)
            | Lyrics::Ttml(s)
            | Lyrics::Srt(s)
            | Lyrics::Lyricsfile(s) => Cow::Borrowed(s),
            Lyrics::Instrumental => {
                Cow::Borrowed(cfg.instrumental_text.as_deref().unwrap_or_default())
            }
        }
    }

    pub fn sanitize(&mut self, cfg: &PluginConfig) {
        match self {
            Lyrics::Lrc(s) | Lyrics::Elrc(s) => {
                *s = lrc::sanitize(s);
                if cfg.strip_section_labels {
                    *s = format::strip_section_labels(s);
                }
            }
            Lyrics::Plain(s) => {
                if cfg.strip_section_labels {
                    *s = format::strip_section_labels(s);
                }
            }
            Lyrics::Ttml(_) | Lyrics::Srt(_) | Lyrics::Lyricsfile(_) | Lyrics::Instrumental => {}
        }
    }

    pub fn is_empty(&self) -> bool {
        match self {
            Lyrics::Plain(s)
            | Lyrics::Lrc(s)
            | Lyrics::Elrc(s)
            | Lyrics::Ttml(s)
            | Lyrics::Srt(s)
            | Lyrics::Lyricsfile(s) => s.trim().is_empty(),
            Lyrics::Instrumental => false,
        }
    }

    pub fn to_response(&self, cfg: &PluginConfig) -> GetLyricsResponse {
        GetLyricsResponse {
            lyrics: vec![LyricsText {
                lang: "xxx".into(),
                text: self.text(cfg).into_owned(),
            }],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum SyncLevel {
    Plain,
    Line,
    Word,
}

impl SyncLevel {
    pub fn rank(self) -> usize {
        match self {
            SyncLevel::Word => 0,
            SyncLevel::Line => 1,
            SyncLevel::Plain => 2,
        }
    }

    pub fn from_rank(rank: usize) -> Option<SyncLevel> {
        match rank {
            0 => Some(SyncLevel::Word),
            1 => Some(SyncLevel::Line),
            2 => Some(SyncLevel::Plain),
            _ => None,
        }
    }

    pub fn slug(&self) -> &'static str {
        match self {
            SyncLevel::Word => "word-by-word",
            SyncLevel::Line => "line-by-line",
            SyncLevel::Plain => "plain",
        }
    }
}

pub struct Word {
    pub text: String,
    pub start_ms: i64,
    pub end_ms: i64,
}

impl Word {
    pub fn new(text: impl Into<String>, start_ms: i64, end_ms: i64) -> Self {
        Self {
            text: text.into(),
            start_ms,
            end_ms,
        }
    }

    pub fn duration(&self) -> i64 {
        self.end_ms - self.start_ms
    }

    pub fn is_separator(&self) -> bool {
        self.text.trim().is_empty()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrackKind {
    Translation,
    Transliteration,
}

pub struct LyricsTrack {
    pub kind: TrackKind,
    pub lang: String,
    pub lrc: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LyricsKind {
    Plain,
    Lrc,
    Elrc,
    Ttml,
    Srt,
    Lyricsfile,
    Instrumental,
}

impl LyricsKind {
    pub fn slug(&self) -> &'static str {
        match self {
            LyricsKind::Plain => "plain",
            LyricsKind::Lrc => "lrc",
            LyricsKind::Elrc => "elrc",
            LyricsKind::Ttml => "ttml",
            LyricsKind::Srt => "srt",
            LyricsKind::Lyricsfile => "lyricsfile",
            LyricsKind::Instrumental => "instrumental",
        }
    }

    pub fn max_sync_level(&self) -> SyncLevel {
        match self {
            LyricsKind::Plain | LyricsKind::Instrumental => SyncLevel::Plain,
            LyricsKind::Lrc | LyricsKind::Srt => SyncLevel::Line,
            LyricsKind::Elrc | LyricsKind::Ttml | LyricsKind::Lyricsfile => SyncLevel::Word,
        }
    }

    pub fn from_slug(slug: &str) -> Option<LyricsKind> {
        match slug.trim().to_ascii_lowercase().as_str() {
            "plain" => Some(LyricsKind::Plain),
            "lrc" => Some(LyricsKind::Lrc),
            "elrc" => Some(LyricsKind::Elrc),
            "ttml" => Some(LyricsKind::Ttml),
            "srt" => Some(LyricsKind::Srt),
            "lyricsfile" => Some(LyricsKind::Lyricsfile),
            "instrumental" => Some(LyricsKind::Instrumental),
            _ => None,
        }
    }
}
