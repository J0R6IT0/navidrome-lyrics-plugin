use crate::{
    config::{PluginConfig, ProviderParams},
    ext::TrackInfoExt,
    format::{elrc, lrc},
    providers::{LyricsProvider, ProviderResult, error::ProviderError, http::Http},
    types::{Lyrics, LyricsKind},
};
use nd_pdk::lyrics::TrackInfo;
use serde::Deserialize;
use std::time::Duration;

const DEFAULT_BASE_URL: &str = "https://api.lrcmux.dev";

const KNOWN_SOURCES: &[&str] = &[
    "genius",
    "kugou",
    "lrclib",
    "musixmatch",
    "netease",
    "ytmusic",
];

#[derive(Deserialize)]
struct JsonResponse {
    track: JsonTrack,
    meta: JsonMeta,
    #[serde(default)]
    lines: Vec<Line>,
}

#[derive(Deserialize)]
struct JsonTrack {
    #[serde(default)]
    duration: f32,
    #[serde(default)]
    album: String,
}

#[derive(Deserialize)]
struct JsonMeta {
    level: SyncLevel,
    #[serde(default)]
    instrumental: bool,
}

#[derive(Deserialize, PartialEq, Eq, Clone, Copy)]
#[serde(rename_all = "lowercase")]
enum SyncLevel {
    Word,
    Line,
    None,
}

#[derive(Deserialize)]
struct Line {
    text: String,
    start: Option<i64>,
    end: Option<i64>,
    words: Option<Vec<Word>>,
}

#[derive(Deserialize)]
struct Word {
    text: String,
    start: i64,
    end: Option<i64>,
}

pub struct Lrcmux {
    base_url: String,
    sources: Option<String>,
}

impl Lrcmux {
    pub fn create(params: &ProviderParams) -> Box<dyn LyricsProvider> {
        Box::new(Self {
            base_url: params
                .get("baseUrl")
                .unwrap_or(DEFAULT_BASE_URL)
                .to_string(),
            sources: restrict_sources(params.get("sources")),
        })
    }

    fn get(&self, track: &TrackInfo) -> ProviderResult<Option<JsonResponse>> {
        let mut request = Http::get(format!("{}/get", self.base_url))
            .param("artist", track.first_artist().unwrap_or_default())
            .param("title", &track.title)
            .param("album", &track.album)
            .param("duration", track.duration().as_secs().to_string());

        if let Some(sources) = &self.sources {
            request = request.param("sources", sources);
        }

        let response = request.send()?;

        match response.status {
            200 => response.json("get").map(Some),
            404 => Ok(None),
            429 => Err(response.rate_limited()),
            _ => Err(response.unexpected_status("lrcmux")),
        }
    }
}

impl LyricsProvider for Lrcmux {
    fn supported_kinds(&self) -> &'static [LyricsKind] {
        &[LyricsKind::Elrc, LyricsKind::Lrc, LyricsKind::Plain]
    }

    fn log_params(&self) -> Vec<(&'static str, String)> {
        let mut params = vec![("baseUrl", self.base_url.clone())];
        if let Some(sources) = &self.sources {
            params.push(("sources", sources.clone()));
        }
        params
    }

    fn fetch_lyrics(
        &self,
        track: &TrackInfo,
        cfg: &PluginConfig,
    ) -> ProviderResult<Option<Lyrics>> {
        if !track.has_artist() {
            return Err(ProviderError::other("track has no artist"));
        }

        let Some(response) = self.get(track)? else {
            return Ok(None);
        };

        if !track.matches_duration(
            Duration::from_secs_f32(response.track.duration),
            cfg.duration_tolerance,
        ) {
            return Err(ProviderError::other(format!(
                "duration mismatch: expected {}s ±{}s, got {}s",
                track.duration().as_secs(),
                cfg.duration_tolerance.as_secs(),
                response.track.duration
            )));
        }

        if cfg.require_album_match && !track.matches_album(&response.track.album) {
            return Err(ProviderError::other(format!(
                "album mismatch: expected '{}', got '{}'",
                track.album, response.track.album
            )));
        }

        Ok(pick_lyrics(response, &cfg.lyrics_type_priority))
    }
}

fn pick_lyrics(response: JsonResponse, order: &[LyricsKind]) -> Option<Lyrics> {
    if response.meta.instrumental {
        return Some(Lyrics::Instrumental);
    }

    let level = response.meta.level;
    let lines = response.lines;

    order.iter().find_map(|kind| match kind {
        LyricsKind::Elrc if level == SyncLevel::Word => Some(build_elrc(&lines))
            .filter(|s| !s.is_empty())
            .map(Lyrics::Elrc),
        LyricsKind::Lrc if level != SyncLevel::None => Some(build_lrc(&lines))
            .filter(|s| !s.is_empty())
            .map(Lyrics::Lrc),
        LyricsKind::Plain => Some(Lyrics::Plain(build_plain(&lines))),
        _ => None,
    })
}

fn restrict_sources(configured: Option<&str>) -> Option<String> {
    let picked: Vec<&str> = configured?
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();

    if picked.is_empty() || KNOWN_SOURCES.iter().all(|known| picked.contains(known)) {
        return None;
    }
    Some(picked.join(","))
}

fn timed_words(line: &Line, words: &[Word]) -> Vec<elrc::Word> {
    words
        .iter()
        .enumerate()
        .map(|(i, w)| elrc::Word {
            text: w.text.clone(),
            start_ms: w.start,
            end_ms: w
                .end
                .or_else(|| words.get(i + 1).map(|next| next.start))
                .or(line.end)
                .unwrap_or(w.start),
        })
        .collect()
}

fn build_elrc(lines: &[Line]) -> String {
    lines
        .iter()
        .filter_map(|l| elrc::render_line(l.start?, &timed_words(l, l.words.as_deref()?)))
        .collect::<Vec<_>>()
        .join("\n")
}

fn build_lrc(lines: &[Line]) -> String {
    lines
        .iter()
        .filter_map(|l| Some(format!("[{}] {}", lrc::format_timestamp(l.start?), l.text)))
        .collect::<Vec<_>>()
        .join("\n")
}

fn build_plain(lines: &[Line]) -> String {
    lines
        .iter()
        .map(|l| l.text.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(text: &str, start: i64, end: i64) -> Word {
        Word {
            text: text.into(),
            start,
            end: Some(end),
        }
    }

    fn synced_line() -> Line {
        Line {
            text: "hello world".into(),
            start: Some(1000),
            end: Some(3000),
            words: Some(vec![word("hello", 1000, 2000), word("world", 2000, 3000)]),
        }
    }

    fn response(level: SyncLevel, instrumental: bool, lines: Vec<Line>) -> JsonResponse {
        JsonResponse {
            track: JsonTrack {
                duration: 180.0,
                album: String::new(),
            },
            meta: JsonMeta {
                level,
                instrumental,
            },
            lines,
        }
    }

    #[track_caller]
    fn check_pick(response: JsonResponse, order: &[LyricsKind], expected: Option<Lyrics>) {
        assert_eq!(pick_lyrics(response, order), expected);
    }

    #[test]
    fn the_highest_priority_format_the_sync_level_allows_wins() {
        check_pick(
            response(SyncLevel::Word, false, vec![synced_line()]),
            &[LyricsKind::Elrc, LyricsKind::Lrc, LyricsKind::Plain],
            Some(Lyrics::Elrc(build_elrc(&[synced_line()]))),
        );
        check_pick(
            response(SyncLevel::Line, false, vec![synced_line()]),
            &[LyricsKind::Elrc, LyricsKind::Lrc, LyricsKind::Plain],
            Some(Lyrics::Lrc(build_lrc(&[synced_line()]))),
        );
        check_pick(
            response(SyncLevel::None, false, vec![synced_line()]),
            &[LyricsKind::Elrc, LyricsKind::Lrc, LyricsKind::Plain],
            Some(Lyrics::Plain("hello world".into())),
        );
    }

    #[test]
    fn a_format_above_the_sync_level_is_skipped() {
        check_pick(
            response(SyncLevel::Line, false, vec![synced_line()]),
            &[LyricsKind::Elrc, LyricsKind::Plain],
            Some(Lyrics::Plain("hello world".into())),
        );
        check_pick(
            response(SyncLevel::None, false, vec![synced_line()]),
            &[LyricsKind::Elrc, LyricsKind::Lrc],
            None,
        );
    }

    #[test]
    fn a_synced_format_with_no_timed_lines_is_skipped() {
        let untimed = Line {
            text: "plain".into(),
            start: None,
            end: None,
            words: None,
        };
        check_pick(
            response(SyncLevel::Word, false, vec![untimed]),
            &[LyricsKind::Elrc, LyricsKind::Plain],
            Some(Lyrics::Plain("plain".into())),
        );
    }

    #[test]
    fn an_instrumental_track_needs_no_lines() {
        check_pick(
            response(SyncLevel::None, true, vec![]),
            &[LyricsKind::Elrc, LyricsKind::Lrc, LyricsKind::Plain],
            Some(Lyrics::Instrumental),
        );
    }

    #[test]
    fn formats_this_provider_cannot_serve_are_never_picked() {
        check_pick(
            response(SyncLevel::Word, false, vec![synced_line()]),
            &[LyricsKind::Ttml, LyricsKind::Srt],
            None,
        );
    }

    #[test]
    fn sources_are_unrestricted_when_unset_or_complete() {
        assert_eq!(restrict_sources(None), None);
        assert_eq!(restrict_sources(Some("")), None);
        assert_eq!(restrict_sources(Some(" , ")), None);
        assert_eq!(
            restrict_sources(Some("genius,kugou,lrclib,musixmatch,netease,ytmusic")),
            None
        );
        assert_eq!(
            restrict_sources(Some(
                "ytmusic,lrclib,musixmatch,kugou,genius,netease,future"
            )),
            None
        );
    }

    #[test]
    fn sources_restrict_when_a_source_is_deselected() {
        assert_eq!(
            restrict_sources(Some("genius,kugou,lrclib,musixmatch")),
            Some("genius,kugou,lrclib,musixmatch".to_string())
        );
        assert_eq!(
            restrict_sources(Some(" musixmatch , kugou ")),
            Some("musixmatch,kugou".to_string())
        );
    }

    #[test]
    fn elrc_renders_word_timings() {
        assert_eq!(
            build_elrc(&[synced_line()]),
            "[00:01.00]<00:01.00>hello<00:02.00>world<00:03.00>"
        );
    }

    #[test]
    fn elrc_ends_on_the_last_word_not_on_the_line() {
        let lines = vec![Line {
            text: "Yeah".into(),
            start: Some(14075),
            end: Some(27672),
            words: Some(vec![word("Yeah", 14075, 14252)]),
        }];
        assert_eq!(build_elrc(&lines), "[00:14.08]<00:14.08>Yeah<00:14.25>");
    }

    #[test]
    fn elrc_falls_back_to_the_line_end_when_the_last_word_has_none() {
        let lines = vec![Line {
            text: "hello".into(),
            start: Some(1000),
            end: Some(3000),
            words: Some(vec![Word {
                text: "hello".into(),
                start: 1000,
                end: None,
            }]),
        }];
        assert_eq!(build_elrc(&lines), "[00:01.00]<00:01.00>hello<00:03.00>");
    }

    #[test]
    fn elrc_falls_back_to_the_next_word_before_the_line_end() {
        let lines = vec![Line {
            text: "hello world".into(),
            start: Some(1000),
            end: Some(9000),
            words: Some(vec![
                Word {
                    text: "hello ".into(),
                    start: 1000,
                    end: None,
                },
                word("world", 2000, 3000),
            ]),
        }];
        assert_eq!(
            build_elrc(&lines),
            "[00:01.00]<00:01.00>hello <00:02.00>world<00:03.00>"
        );
    }

    #[test]
    fn lrc_renders_line_timestamps() {
        let lines = vec![
            Line {
                text: "first".into(),
                start: Some(1000),
                end: None,
                words: None,
            },
            Line {
                text: "second".into(),
                start: Some(2500),
                end: None,
                words: None,
            },
        ];
        assert_eq!(build_lrc(&lines), "[00:01.00] first\n[00:02.50] second");
    }

    #[test]
    fn a_build_is_empty_when_no_line_has_timings() {
        let lines = vec![Line {
            text: "plain".into(),
            start: None,
            end: None,
            words: None,
        }];
        assert!(build_elrc(&lines).is_empty());
        assert!(build_lrc(&lines).is_empty());
    }
}
