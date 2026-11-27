use crate::{
    config::PluginConfig,
    format::{self, censor, lrc, lyricsfile, ttml},
};
use nd_pdk::lyrics::{GetLyricsResponse, LyricsText};

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

    fn content(&self) -> Option<&str> {
        match self {
            Lyrics::Plain(s)
            | Lyrics::Lrc(s)
            | Lyrics::Elrc(s)
            | Lyrics::Ttml(s)
            | Lyrics::Srt(s)
            | Lyrics::Lyricsfile(s) => Some(s),
            Lyrics::Instrumental => None,
        }
    }

    pub fn text<'a>(&'a self, cfg: &'a PluginConfig) -> &'a str {
        self.content()
            .unwrap_or_else(|| cfg.instrumental_text.as_deref().unwrap_or_default())
    }

    pub fn is_empty(&self) -> bool {
        self.content().is_some_and(|s| s.trim().is_empty())
    }

    pub fn is_censored(&self) -> bool {
        self.content().is_some_and(censor::is_censored)
    }

    pub fn sync_level(&self) -> SyncLevel {
        match self {
            Lyrics::Ttml(s) => ttml::sync_level(s),
            Lyrics::Lyricsfile(s) => lyricsfile::sync_level(s),
            _ => self.kind().max_sync_level(),
        }
    }

    pub fn sanitize(&mut self, cfg: &PluginConfig) {
        if let Lyrics::Lrc(s) | Lyrics::Elrc(s) = self {
            *s = lrc::sanitize(s);
        }

        if cfg.strip_section_labels
            && let Lyrics::Plain(s) | Lyrics::Lrc(s) | Lyrics::Elrc(s) = self
        {
            *s = format::strip_section_labels(s);
        }
    }

    pub fn to_response(&self, cfg: &PluginConfig) -> GetLyricsResponse {
        GetLyricsResponse {
            lyrics: vec![LyricsText {
                lang: "xxx".into(),
                text: self.text(cfg).to_owned(),
            }],
        }
    }
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
    const ALL: [LyricsKind; 7] = [
        LyricsKind::Plain,
        LyricsKind::Lrc,
        LyricsKind::Elrc,
        LyricsKind::Ttml,
        LyricsKind::Srt,
        LyricsKind::Lyricsfile,
        LyricsKind::Instrumental,
    ];

    pub fn from_slug(slug: &str) -> Option<LyricsKind> {
        let slug = slug.trim();
        Self::ALL
            .into_iter()
            .find(|kind| kind.slug().eq_ignore_ascii_case(slug))
    }

    pub fn slug(self) -> &'static str {
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

    pub fn max_sync_level(self) -> SyncLevel {
        match self {
            LyricsKind::Plain | LyricsKind::Instrumental => SyncLevel::Plain,
            LyricsKind::Lrc | LyricsKind::Srt => SyncLevel::Line,
            LyricsKind::Elrc | LyricsKind::Ttml | LyricsKind::Lyricsfile => SyncLevel::Word,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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

    pub fn slug(self) -> &'static str {
        match self {
            SyncLevel::Word => "word-by-word",
            SyncLevel::Line => "line-by-line",
            SyncLevel::Plain => "plain",
        }
    }
}

pub struct LyricsTrack {
    pub kind: TrackKind,
    pub lang: String,
    pub lrc: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrackKind {
    Translation,
    Transliteration,
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
