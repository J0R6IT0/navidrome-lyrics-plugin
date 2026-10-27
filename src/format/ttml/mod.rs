use crate::types::SyncLevel;
use regex::Regex;

mod document;

pub use document::{Document, TrackKind};

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
}
