use crate::types::{Lyrics, LyricsTrack, SyncLevel};
use document::Document;
use regex::Regex;

mod document;

pub fn with_tracks(lyrics: &Lyrics, tracks: &[LyricsTrack]) -> Option<String> {
    let mut doc = match lyrics {
        Lyrics::Lrc(text) => Document::from_lrc(text),
        Lyrics::Elrc(text) => Document::from_elrc(text),
        _ => return None,
    };

    for track in tracks {
        doc.add_track_by_time(track.kind, &track.lang, &track.lrc);
    }

    doc.has_tracks().then(|| doc.to_ttml())
}

pub fn sync_level(doc: &str) -> SyncLevel {
    if let Some(level) = itunes_timing(doc) {
        return level;
    }

    if is_match(doc, r#"(?i)<span\b[^>]*\bbegin\s*=\s*["']"#) {
        SyncLevel::Word
    } else if is_match(doc, r#"(?i)<p\b[^>]*\bbegin\s*=\s*["']"#) {
        SyncLevel::Line
    } else {
        SyncLevel::Plain
    }
}

fn itunes_timing(doc: &str) -> Option<SyncLevel> {
    let re = Regex::new(r#"(?i)\bitunes:timing\s*=\s*["']\s*([a-z]+)\s*["']"#).unwrap();
    let value = re.captures(doc)?.get(1)?.as_str();

    match value.to_ascii_lowercase().as_str() {
        "word" => Some(SyncLevel::Word),
        "line" => Some(SyncLevel::Line),
        "none" => Some(SyncLevel::Plain),
        _ => None,
    }
}

fn is_match(doc: &str, pattern: &str) -> bool {
    Regex::new(pattern).unwrap().is_match(doc)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[track_caller]
    fn check(doc: &str, expected: SyncLevel) {
        assert_eq!(sync_level(doc), expected, "{doc}");
    }

    #[test]
    fn the_itunes_timing_attribute_determines_the_level() {
        check(
            r#"<tt xmlns:itunes="http://music.apple.com/lyric-ttml-internal" itunes:timing="Word" xml:lang="en"><body></body></tt>"#,
            SyncLevel::Word,
        );
        check(
            r#"<tt itunes:timing="Line"><body><div><p>Hello</p></div></body></tt>"#,
            SyncLevel::Line,
        );
        check(
            r#"<tt itunes:timing="None"><body><div><p>Hello</p></div></body></tt>"#,
            SyncLevel::Plain,
        );
        check(r#"<tt ITUNES:TIMING='word'></tt>"#, SyncLevel::Word);
    }

    #[test]
    fn the_itunes_timing_attribute_wins_over_the_markup() {
        check(
            concat!(
                r#"<tt itunes:timing="Line"><body><div>"#,
                r#"<p begin="1.0" end="2.0"><span>Hello</span></p>"#,
                r#"</div></body></tt>"#
            ),
            SyncLevel::Line,
        );
    }

    #[test]
    fn an_unknown_itunes_timing_attribute_falls_back_to_the_markup() {
        check(
            r#"<tt itunes:timing="Bogus"><body><div><p begin="1.0">Hi</p></div></body></tt>"#,
            SyncLevel::Line,
        );
    }

    #[test]
    fn timed_spans_are_word_synced() {
        check(
            concat!(
                r#"<tt xmlns:itunes="http://music.apple.com/lyrics"><body><div>"#,
                r#"<p begin="00:00:11.011" end="00:00:13.643">"#,
                r#"<span begin="00:00:11.011" end="00:00:11.397">Say </span>"#,
                r#"</p></div></body></tt>"#
            ),
            SyncLevel::Word,
        );
    }

    #[test]
    fn timed_paragraphs_are_line_synced() {
        check(
            r#"<tt><body><div><p begin="45.452" end="49.551">Sleep well</p></div></body></tt>"#,
            SyncLevel::Line,
        );
        check(
            "<tt><body><div><p begin = '1.0'>Hi</p></div></body></tt>",
            SyncLevel::Line,
        );
    }

    #[test]
    fn untimed_translation_spans_do_not_count_as_words() {
        check(
            concat!(
                r#"<tt><body><div><p begin="1.0" end="2.0">Hello"#,
                r#"<span ttm:role="x-translation" xml:lang="es">Hola</span>"#,
                r#"</p></div></body></tt>"#
            ),
            SyncLevel::Line,
        );
    }

    #[test]
    fn no_timings_is_plain() {
        check(
            r#"<tt><body><div><p>I don't know what to do</p></div></body></tt>"#,
            SyncLevel::Plain,
        );
        check("", SyncLevel::Plain);
    }

    mod tracks {
        use super::*;
        use crate::types::TrackKind;

        fn translation(lrc: &str) -> LyricsTrack {
            LyricsTrack {
                kind: TrackKind::Translation,
                lang: "en-GB".to_string(),
                lrc: lrc.to_string(),
            }
        }

        #[track_caller]
        fn check_tracks(lyrics: Lyrics, tracks: &[LyricsTrack], expected: Option<SyncLevel>) {
            let ttml = with_tracks(&lyrics, tracks);
            assert_eq!(ttml.as_deref().map(sync_level), expected, "{ttml:?}");

            if let Some(ttml) = ttml {
                assert!(ttml.contains(r#"<text for="L1">Hi</text>"#), "{ttml}");
            }
        }

        #[test]
        fn lrc_becomes_line_synced_ttml() {
            check_tracks(
                Lyrics::Lrc("[00:01.00]Hello".to_string()),
                &[translation("[00:01.00]Hi")],
                Some(SyncLevel::Line),
            );
        }

        #[test]
        fn elrc_becomes_word_synced_ttml() {
            check_tracks(
                Lyrics::Elrc("[00:01.00]<00:01.00>Hello<00:02.00>".to_string()),
                &[translation("[00:01.00]Hi")],
                Some(SyncLevel::Word),
            );
        }

        #[test]
        fn lyrics_without_timestamps_cannot_carry_tracks() {
            let tracks = [translation("[00:00.00]Hi")];

            check_tracks(Lyrics::Plain("Hello".to_string()), &tracks, None);
            check_tracks(Lyrics::Instrumental, &tracks, None);
            check_tracks(
                Lyrics::Ttml(
                    r#"<tt><body><div><p begin="0.0">Hello</p></div></body></tt>"#.to_string(),
                ),
                &tracks,
                None,
            );
        }

        #[test]
        fn nothing_changes_when_no_track_matches() {
            let lyrics = || Lyrics::Lrc("[00:05.00]Hello".to_string());

            check_tracks(lyrics(), &[translation("[00:01.00]Hi")], None);
            check_tracks(lyrics(), &[], None);
        }
    }
}
