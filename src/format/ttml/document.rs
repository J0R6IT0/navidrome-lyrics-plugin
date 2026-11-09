use crate::format::lrc;
use crate::types::TrackKind;
use quick_xml::Writer;
use quick_xml::events::{BytesEnd, BytesStart, BytesText, Event};
use std::io;

const TRANSLATION_MATCH_TOLERANCE_MS: i64 = 50;

const TTML_NAMESPACE: &str = "http://www.w3.org/ns/ttml";
const ITUNES_NAMESPACE: &str = "http://music.apple.com/lyric-ttml-internal";

type XmlWriter = Writer<Vec<u8>>;

impl TrackKind {
    fn elements(self) -> (&'static str, &'static str) {
        match self {
            TrackKind::Translation => ("translations", "translation"),
            TrackKind::Transliteration => ("transliterations", "transliteration"),
        }
    }
}

pub struct Document {
    lines: Vec<Line>,
    tracks: Vec<Track>,
}

struct Track {
    kind: TrackKind,
    lang: String,
    texts: Vec<Option<String>>,
}

struct Line {
    start_ms: Option<i64>,
    text: String,
    /// The words of a word synced line.
    spans: Vec<Span>,
}

struct Span {
    /// Cleaned but keeps the whitespace around it.
    text: String,
    start_ms: i64,
    end_ms: Option<i64>,
}

impl Line {
    fn plain(text: &str) -> Self {
        Self {
            start_ms: None,
            text: clean_trimmed(text),
            spans: Vec::new(),
        }
    }

    fn synced(start_ms: i64, text: &str) -> Self {
        Self {
            start_ms: Some(start_ms),
            ..Self::plain(text)
        }
    }

    fn word_synced(start_ms: i64, mut spans: Vec<Span>) -> Self {
        for span in &mut spans {
            span.text = clean(&span.text);
        }
        let text: String = spans.iter().map(|span| span.text.as_str()).collect();

        Self {
            start_ms: Some(start_ms),
            text: clean_trimmed(&text),
            spans,
        }
    }

    fn is_blank(&self) -> bool {
        self.text.is_empty()
    }

    fn is_word_synced(&self) -> bool {
        !self.spans.is_empty()
    }

    fn write(&self, w: &mut XmlWriter, key: usize, next_start: Option<i64>) -> io::Result<()> {
        let words = timed_words(&self.spans, next_start);

        let begin = words
            .iter()
            .map(|word| word.start_ms)
            .chain(self.start_ms)
            .min();
        let end = words
            .iter()
            .filter_map(|word| word.end_ms)
            .max()
            .or(next_start);

        let mut p = w.create_element("p");
        if let Some(begin) = begin {
            p = p.with_attribute(("begin", clock(begin).as_str()));
            if let Some(end) = end {
                p = p.with_attribute(("end", clock(end.max(begin)).as_str()));
            }
        }

        p.with_attribute(("itunes:key", key_id(key).as_str()))
            .write_inner_content(|w| {
                if self.is_word_synced() {
                    write_words(w, &words)
                } else {
                    w.write_event(Event::Text(BytesText::new(&self.text)))
                }
            })
            .map(drop)
    }
}

/// A word ready to be written, trimmed and with its end resolved.
struct TimedWord<'a> {
    text: &'a str,
    start_ms: i64,
    end_ms: Option<i64>,
    /// Whether a space separates it from the word before.
    spaced: bool,
}

/// Whitespace is taken out of the words so it can go between the spans.
fn timed_words(spans: &[Span], line_end: Option<i64>) -> Vec<TimedWord<'_>> {
    let mut words = Vec::with_capacity(spans.len());
    let mut space = false;

    for (i, span) in spans.iter().enumerate() {
        let word = span.text.trim();
        if word.is_empty() {
            space |= !span.text.is_empty();
            continue;
        }

        words.push(TimedWord {
            text: word,
            start_ms: span.start_ms,
            end_ms: span
                .end_ms
                .or_else(|| spans.get(i + 1).map(|next| next.start_ms))
                .or(line_end)
                .map(|end| end.max(span.start_ms)),
            spaced: !words.is_empty() && (space || span.text.starts_with(' ')),
        });
        space = span.text.ends_with(' ');
    }

    words
}

fn write_words(w: &mut XmlWriter, words: &[TimedWord]) -> io::Result<()> {
    for word in words {
        if word.spaced {
            w.write_event(Event::Text(BytesText::new(" ")))?;
        }

        let mut span = w
            .create_element("span")
            .with_attribute(("begin", clock(word.start_ms).as_str()));
        if let Some(end) = word.end_ms {
            span = span.with_attribute(("end", clock(end).as_str()));
        }
        span.write_text_content(BytesText::new(word.text))?;
    }

    Ok(())
}

impl Document {
    fn from_lines(lines: Vec<Line>) -> Self {
        Self {
            lines,
            tracks: Vec::new(),
        }
    }

    #[allow(dead_code)]
    pub fn from_plain(text: &str) -> Self {
        Self::from_lines(strip_bom(text).lines().map(Line::plain).collect())
    }

    /// Word time tags are dropped.
    pub fn from_lrc(text: &str) -> Self {
        parse_lrc(text, false)
    }

    pub fn from_elrc(text: &str) -> Self {
        parse_lrc(text, true)
    }

    pub fn has_tracks(&self) -> bool {
        !self.tracks.is_empty()
    }

    pub fn add_track_by_time(&mut self, kind: TrackKind, lang: &str, lrc: &str) {
        let texts = self.match_by_time(lrc);
        if texts.iter().all(Option::is_none) {
            return;
        }

        let lang = clean_trimmed(lang);
        match self
            .tracks
            .iter_mut()
            .find(|track| track.kind == kind && track.lang.eq_ignore_ascii_case(&lang))
        {
            Some(track) => {
                for (have, new) in track.texts.iter_mut().zip(texts) {
                    if have.is_none() {
                        *have = new;
                    }
                }
            }
            None => self.tracks.push(Track { kind, lang, texts }),
        }
    }

    fn match_by_time(&self, lrc: &str) -> Vec<Option<String>> {
        let mut entries: Vec<(i64, Option<String>)> = Document::from_lrc(lrc)
            .lines
            .into_iter()
            .filter(|line| !line.is_blank())
            .filter_map(|line| Some((line.start_ms?, Some(line.text))))
            .collect();

        let mut pairs = Vec::new();
        for (line_index, line) in self.lines.iter().enumerate() {
            let Some(start) = line.start_ms.filter(|_| !line.is_blank()) else {
                continue;
            };
            for (entry_index, (time, _)) in entries.iter().enumerate() {
                let distance = (start - time).abs();
                if distance <= TRANSLATION_MATCH_TOLERANCE_MS {
                    pairs.push((distance, line_index, entry_index));
                }
            }
        }
        pairs.sort_unstable();

        let mut texts = vec![None; self.lines.len()];
        for (_, line_index, entry_index) in pairs {
            if texts[line_index].is_none() {
                texts[line_index] = entries[entry_index].1.take();
            }
        }

        texts
    }

    pub fn to_ttml(&self) -> String {
        let mut w = Writer::new(Vec::new());
        self.write(&mut w).expect("writing to memory cannot fail");
        String::from_utf8(w.into_inner()).expect("the writer only writes UTF-8")
    }

    fn write(&self, w: &mut XmlWriter) -> io::Result<()> {
        let keys = self.keys();

        w.create_element("tt")
            .with_attributes([
                ("xmlns", TTML_NAMESPACE),
                ("xmlns:itunes", ITUNES_NAMESPACE),
                ("itunes:timing", self.timing()),
            ])
            .write_inner_content(|w| {
                self.write_head(w, &keys)?;
                self.write_body(w, &keys)
            })
            .map(drop)
    }

    fn keys(&self) -> Vec<Option<usize>> {
        let mut key = 0;
        self.lines
            .iter()
            .map(|line| {
                (!line.is_blank()).then(|| {
                    key += 1;
                    key
                })
            })
            .collect()
    }

    fn timing(&self) -> &'static str {
        let lines = || self.lines.iter().filter(|line| !line.is_blank());

        if lines().any(Line::is_word_synced) {
            "Word"
        } else if lines().any(|line| line.start_ms.is_some()) {
            "Line"
        } else {
            "None"
        }
    }

    fn next_starts(&self) -> Vec<Option<i64>> {
        let mut next = None;
        let mut out = vec![None; self.lines.len()];
        for (i, line) in self.lines.iter().enumerate().rev() {
            out[i] = next;
            next = line.start_ms.or(next);
        }
        out
    }

    fn write_head(&self, w: &mut XmlWriter, keys: &[Option<usize>]) -> io::Result<()> {
        if !self.has_tracks() {
            return Ok(());
        }

        w.write_event(Event::Start(BytesStart::new("head")))?;
        w.write_event(Event::Start(BytesStart::new("metadata")))?;
        w.write_event(Event::Start(
            BytesStart::new("iTunesMetadata").with_attributes([("xmlns", ITUNES_NAMESPACE)]),
        ))?;

        for kind in [TrackKind::Translation, TrackKind::Transliteration] {
            let of_kind = || self.tracks.iter().filter(|track| track.kind == kind);
            if of_kind().next().is_none() {
                continue;
            }

            let (group, element) = kind.elements();
            w.create_element(group).write_inner_content(|w| {
                for track in of_kind() {
                    let mut el = w.create_element(element);
                    if !track.lang.is_empty() {
                        el = el.with_attribute(("xml:lang", track.lang.as_str()));
                    }
                    el.write_inner_content(|w| track.write_texts(w, keys))?;
                }
                Ok(())
            })?;
        }

        w.write_event(Event::End(BytesEnd::new("iTunesMetadata")))?;
        w.write_event(Event::End(BytesEnd::new("metadata")))?;
        w.write_event(Event::End(BytesEnd::new("head")))
    }

    fn write_body(&self, w: &mut XmlWriter, keys: &[Option<usize>]) -> io::Result<()> {
        let next_starts = self.next_starts();

        w.create_element("body")
            .write_inner_content(|w| {
                for stanza in stanzas(keys) {
                    w.create_element("div").write_inner_content(|w| {
                        for (i, key) in stanza {
                            self.lines[i].write(w, key, next_starts[i])?;
                        }
                        Ok(())
                    })?;
                }
                Ok(())
            })
            .map(drop)
    }
}

impl Track {
    fn write_texts(&self, w: &mut XmlWriter, keys: &[Option<usize>]) -> io::Result<()> {
        for (text, key) in self.texts.iter().zip(keys) {
            if let (Some(text), Some(key)) = (text, key) {
                w.create_element("text")
                    .with_attribute(("for", key_id(*key).as_str()))
                    .write_text_content(BytesText::new(text))?;
            }
        }
        Ok(())
    }
}

fn stanzas(keys: &[Option<usize>]) -> Vec<Vec<(usize, usize)>> {
    let mut stanzas = vec![Vec::new()];
    for (i, key) in keys.iter().enumerate() {
        match key {
            Some(key) => stanzas
                .last_mut()
                .expect("there is always a stanza")
                .push((i, *key)),
            None => stanzas.push(Vec::new()),
        }
    }
    stanzas.retain(|stanza| !stanza.is_empty());
    stanzas
}

fn parse_lrc(text: &str, word_synced: bool) -> Document {
    let offset_ms = offset_ms(text);
    let mut timed = Vec::new();
    let mut untimed = Vec::new();

    for raw in strip_bom(text).lines() {
        let (times, rest) = leading_times(raw);

        let Some(&first) = times.first() else {
            if !is_tag_line(raw) {
                untimed.push(Line::plain(raw));
            }
            continue;
        };

        for &time in &times {
            let start_ms = time - offset_ms;
            timed.push(if word_synced {
                word_line(start_ms, rest, start_ms - first)
            } else {
                Line::synced(start_ms, &lrc::strip_word_tags(rest))
            });
        }
    }

    if timed.is_empty() {
        return Document::from_lines(untimed);
    }

    timed.sort_by_key(|line| line.start_ms);
    Document::from_lines(timed)
}

fn word_line(start_ms: i64, text: &str, shift_ms: i64) -> Line {
    let mut spans = Vec::new();
    let mut segment = String::new();
    let mut segment_start = start_ms;
    let mut rest = text;
    let mut tagged = false;

    while let Some((before, time, after)) = next_word_tag(rest) {
        segment.push_str(before);
        let time = time + shift_ms;

        if !segment.is_empty() {
            spans.push(Span {
                text: std::mem::take(&mut segment),
                start_ms: segment_start,
                end_ms: Some(time),
            });
        }

        segment_start = time;
        rest = after;
        tagged = true;
    }
    segment.push_str(rest);

    if !tagged {
        return Line::synced(start_ms, &segment);
    }

    if !segment.is_empty() {
        spans.push(Span {
            text: segment,
            start_ms: segment_start,
            end_ms: None,
        });
    }

    Line::word_synced(start_ms, spans)
}

fn next_word_tag(text: &str) -> Option<(&str, i64, &str)> {
    let mut from = 0;

    while let Some(open) = text[from..].find('<').map(|i| from + i) {
        let after = &text[open + 1..];
        if after.starts_with(|c: char| c.is_ascii_digit())
            && let Some(close) = after.find('>')
            && let Some(secs) = lrc::parse_time_tag(&after[..close])
        {
            return Some((&text[..open], secs_to_ms(secs), &after[close + 1..]));
        }
        from = open + 1;
    }

    None
}

fn leading_times(line: &str) -> (Vec<i64>, &str) {
    let mut rest = strip_bom(line).trim_start();
    let mut times = Vec::new();

    while let Some(after) = rest.strip_prefix('[')
        && let Some(close) = after.find(']')
        && let Some(secs) = lrc::parse_time_tag(&after[..close])
    {
        times.push(secs_to_ms(secs));
        rest = &after[close + 1..];
    }

    (times, rest)
}

/// The first valid `[offset:±ms]` tag.
fn offset_ms(text: &str) -> i64 {
    text.lines()
        .find_map(|line| {
            let inner = strip_bom(line)
                .trim()
                .strip_prefix('[')?
                .strip_suffix(']')?;
            let (key, value) = inner.split_once(':')?;
            if !key.trim().eq_ignore_ascii_case("offset") {
                return None;
            }
            value.trim().parse().ok()
        })
        .unwrap_or(0)
}

fn is_tag_line(line: &str) -> bool {
    let trimmed = strip_bom(line).trim();
    trimmed.starts_with('[') && trimmed.find(']') == Some(trimmed.len() - 1)
}

fn strip_bom(text: &str) -> &str {
    text.trim_start_matches('\u{feff}')
}

fn secs_to_ms(secs: f64) -> i64 {
    (secs * 1000.0).round() as i64
}

fn key_id(key: usize) -> String {
    format!("L{key}")
}

fn clock(ms: i64) -> String {
    let ms = ms.max(0);
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        ms / 3_600_000,
        ms / 60_000 % 60,
        ms / 1000 % 60,
        ms % 1000
    )
}

fn clean(text: &str) -> String {
    let mut out = String::with_capacity(text.len());

    for c in text.chars() {
        let c = match c {
            '\t' | '\n' | '\r' => ' ',
            c => c,
        };
        if !is_xml_char(c) || (c == ' ' && out.ends_with(' ')) {
            continue;
        }
        out.push(c);
    }

    out
}

fn clean_trimmed(text: &str) -> String {
    clean(text).trim().to_owned()
}

fn is_xml_char(c: char) -> bool {
    matches!(c, '\u{20}'..='\u{D7FF}' | '\u{E000}'..='\u{FFFD}' | '\u{10000}'..)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::ttml::sync_level;
    use crate::types::SyncLevel;

    fn trim_indent(text: &str) -> String {
        let text = text.strip_prefix('\n').unwrap_or(text);
        let indent = text
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| line.len() - line.trim_start().len())
            .min()
            .unwrap_or(0);

        text.lines()
            .map(|line| line.get(indent..).unwrap_or_default())
            .collect::<Vec<_>>()
            .join("\n")
            .trim_end()
            .to_owned()
    }

    fn plain(text: &str) -> Document {
        Document::from_plain(&trim_indent(text))
    }

    fn lrc(text: &str) -> Document {
        Document::from_lrc(&trim_indent(text))
    }

    fn elrc(text: &str) -> Document {
        Document::from_elrc(&trim_indent(text))
    }

    fn indent(xml: &str) -> String {
        let mut out = String::new();
        let mut depth = 0;
        let mut inline: Option<&str> = None;
        let mut rest = xml;

        while !rest.is_empty() {
            let len = match rest.starts_with('<') {
                true => rest.find('>').expect("tags are closed") + 1,
                false => rest.find('<').unwrap_or(rest.len()),
            };
            let (token, tail) = rest.split_at(len);
            rest = tail;

            if inline.is_some() || !token.starts_with('<') {
                out.push_str(token);
                if inline.is_some_and(|name| token == format!("</{name}>")) {
                    inline = None;
                }
                continue;
            }

            let closing = token.starts_with("</");
            if closing {
                depth -= 1;
            }
            out.push('\n');
            out.push_str(&"  ".repeat(depth));
            out.push_str(token);

            if closing || token.ends_with("/>") {
                continue;
            }
            match token[1..].split([' ', '>']).next() {
                Some(name @ ("p" | "text")) => inline = Some(name),
                _ => depth += 1,
            }
        }

        out.trim_start().to_string()
    }

    #[track_caller]
    fn render(doc: &Document) -> String {
        let ttml = doc.to_ttml();
        if let Err(e) = roxmltree::Document::parse(&ttml) {
            panic!("not well formed ({e}):\n{ttml}");
        }

        indent(&ttml)
            .replace(
                &format!(r#" xmlns="{TTML_NAMESPACE}" xmlns:itunes="{ITUNES_NAMESPACE}""#),
                "",
            )
            .replace(&format!(r#" xmlns="{ITUNES_NAMESPACE}""#), "")
    }

    #[track_caller]
    fn check(doc: &Document, expected: &str) {
        assert_eq!(render(doc), trim_indent(expected));
    }

    /// Only compares the paragraphs.
    #[track_caller]
    fn check_lines(doc: &Document, expected: &str) {
        let ttml = render(doc);
        let paragraphs: Vec<&str> = ttml
            .lines()
            .map(str::trim)
            .filter(|line| line.starts_with("<p"))
            .collect();

        assert_eq!(paragraphs.join("\n"), trim_indent(expected));
    }

    /// Only compares what is inside `<iTunesMetadata>`.
    #[track_caller]
    fn check_tracks(doc: &Document, expected: &str) {
        let ttml = render(doc);
        let lines: Vec<&str> = ttml.lines().collect();
        let inside = match (
            lines.iter().position(|l| l.trim() == "<iTunesMetadata>"),
            lines.iter().position(|l| l.trim() == "</iTunesMetadata>"),
        ) {
            (Some(open), Some(close)) => trim_indent(&lines[open + 1..close].join("\n")),
            _ => String::new(),
        };

        assert_eq!(inside, trim_indent(expected));
    }

    mod structure {
        use super::*;

        #[test]
        fn an_empty_document_has_an_empty_body() {
            let expected = r#"
                <tt itunes:timing="None">
                  <body>
                  </body>
                </tt>"#;

            check(&plain(""), expected);
            check(&lrc("[ar:Artist]"), expected);
        }

        #[test]
        fn plain_lines_become_untimed_paragraphs() {
            check(
                &plain(
                    "
                    Hello
                    World",
                ),
                r#"
                <tt itunes:timing="None">
                  <body>
                    <div>
                      <p itunes:key="L1">Hello</p>
                      <p itunes:key="L2">World</p>
                    </div>
                  </body>
                </tt>"#,
            );
        }

        #[test]
        fn blank_lines_split_stanzas() {
            check(
                &Document::from_plain("\n\nA\nB\n\n\n \nC\n\n"),
                r#"
                <tt itunes:timing="None">
                  <body>
                    <div>
                      <p itunes:key="L1">A</p>
                      <p itunes:key="L2">B</p>
                    </div>
                    <div>
                      <p itunes:key="L3">C</p>
                    </div>
                  </body>
                </tt>"#,
            );
        }

        #[test]
        fn the_output_is_not_indented() {
            let doc = elrc(
                "
                [00:01.00]<00:01.00>Hel<00:01.20>lo<00:01.40>
                [00:02.00]Hello",
            );

            let ttml = doc.to_ttml();
            assert!(!ttml.contains('\n') && !ttml.contains("> "), "{ttml}");
        }

        #[test]
        fn one_word_synced_line_makes_the_document_word_synced() {
            check(
                &elrc(
                    "
                    [00:01.00]<00:01.00>Hello<00:02.00>
                    [00:03.00]World",
                ),
                r#"
                <tt itunes:timing="Word">
                  <body>
                    <div>
                      <p begin="00:00:01.000" end="00:00:02.000" itunes:key="L1"><span begin="00:00:01.000" end="00:00:02.000">Hello</span></p>
                      <p begin="00:00:03.000" itunes:key="L2">World</p>
                    </div>
                  </body>
                </tt>"#,
            );
        }

        #[test]
        fn ttml_sync_level_matches_timing() {
            for (doc, level) in [
                (plain("Hello"), SyncLevel::Plain),
                (lrc("[00:01.00]Hello"), SyncLevel::Line),
                (elrc("[00:01.00]<00:01.00>Hello<00:02.00>"), SyncLevel::Word),
            ] {
                assert_eq!(sync_level(&doc.to_ttml()), level);
            }
        }
    }

    mod from_plain {
        use super::*;

        #[test]
        fn lines_are_trimmed_and_the_bom_dropped() {
            check_lines(
                &Document::from_plain("\u{feff}  Hello  \n\tWorld"),
                r#"
                <p itunes:key="L1">Hello</p>
                <p itunes:key="L2">World</p>"#,
            );
        }

        #[test]
        fn timestamps_are_just_text() {
            check_lines(
                &plain("[00:01.00]Hello"),
                r#"<p itunes:key="L1">[00:01.00]Hello</p>"#,
            );
        }
    }

    mod from_lrc {
        use super::*;

        #[test]
        fn a_line_ends_when_the_next_starts() {
            check_lines(
                &lrc("
                    [00:01.00]Hello
                    [00:02.50]World"),
                r#"
                <p begin="00:00:01.000" end="00:00:02.500" itunes:key="L1">Hello</p>
                <p begin="00:00:02.500" itunes:key="L2">World</p>"#,
            );
        }

        #[test]
        fn a_blank_line_ends_the_previous_and_starts_a_stanza() {
            check(
                &lrc("
                    [00:01.00]A
                    [00:03.00]
                    [00:10.00]B"),
                r#"
                <tt itunes:timing="Line">
                  <body>
                    <div>
                      <p begin="00:00:01.000" end="00:00:03.000" itunes:key="L1">A</p>
                    </div>
                    <div>
                      <p begin="00:00:10.000" itunes:key="L2">B</p>
                    </div>
                  </body>
                </tt>"#,
            );
        }

        #[test]
        fn a_line_with_several_timestamps_is_repeated_in_time_order() {
            check_lines(
                &lrc("
                    [00:01.00]Verse
                    [00:05.00][00:20.00]Chorus
                    [00:10.00]Verse 2"),
                r#"
                <p begin="00:00:01.000" end="00:00:05.000" itunes:key="L1">Verse</p>
                <p begin="00:00:05.000" end="00:00:10.000" itunes:key="L2">Chorus</p>
                <p begin="00:00:10.000" end="00:00:20.000" itunes:key="L3">Verse 2</p>
                <p begin="00:00:20.000" itunes:key="L4">Chorus</p>"#,
            );
        }

        #[test]
        fn lines_are_sorted_and_equal_timestamps_keep_their_order() {
            check_lines(
                &lrc("
                    [00:05.00]B
                    [00:01.00]A
                    [00:05.00]B2"),
                r#"
                <p begin="00:00:01.000" end="00:00:05.000" itunes:key="L1">A</p>
                <p begin="00:00:05.000" end="00:00:05.000" itunes:key="L2">B</p>
                <p begin="00:00:05.000" itunes:key="L3">B2</p>"#,
            );
        }

        #[test]
        fn offset_is_applied() {
            check_lines(
                &lrc("
                    [offset:+500]
                    [00:01.00]A
                    [00:02.00]B"),
                r#"
                <p begin="00:00:00.500" end="00:00:01.500" itunes:key="L1">A</p>
                <p begin="00:00:01.500" itunes:key="L2">B</p>"#,
            );

            check_lines(
                &lrc("
                    [offset:-250]
                    [00:01.00]A"),
                r#"<p begin="00:00:01.250" itunes:key="L1">A</p>"#,
            );
        }

        #[test]
        fn an_offset_cannot_move_lyrics_before_the_start() {
            check_lines(
                &lrc("
                    [offset:2000]
                    [00:01.00]A
                    [00:03.00]B"),
                r#"
                <p begin="00:00:00.000" end="00:00:01.000" itunes:key="L1">A</p>
                <p begin="00:00:01.000" itunes:key="L2">B</p>"#,
            );
        }

        #[test]
        fn metadata_and_untimed_lines_are_ignored() {
            check_lines(
                &lrc("
                    [ar:Artist]
                    [ti:Title]
                    Some untimed line
                    [Chorus]
                    [00:01.00]Hello"),
                r#"<p begin="00:00:01.000" itunes:key="L1">Hello</p>"#,
            );
        }

        #[test]
        fn unsynced_lyrics_become_plain_lines() {
            check(
                &lrc("
                    [ar:Artist]
                    [Intro] Hello

                    World"),
                r#"
                <tt itunes:timing="None">
                  <body>
                    <div>
                      <p itunes:key="L1">[Intro] Hello</p>
                    </div>
                    <div>
                      <p itunes:key="L2">World</p>
                    </div>
                  </body>
                </tt>"#,
            );
        }

        #[test]
        fn word_tags_are_dropped() {
            check(
                &lrc("[00:01.00]<00:01.00>Hello <00:01.50>world<00:02.00>"),
                r#"
                <tt itunes:timing="Line">
                  <body>
                    <div>
                      <p begin="00:00:01.000" itunes:key="L1">Hello world</p>
                    </div>
                  </body>
                </tt>"#,
            );
        }

        #[test]
        fn text_is_trimmed_and_the_bom_dropped() {
            check_lines(
                &Document::from_lrc("\u{feff}[00:01.00]   Hello  "),
                r#"<p begin="00:00:01.000" itunes:key="L1">Hello</p>"#,
            );
        }
    }

    mod from_elrc {
        use super::*;

        #[test]
        fn words_become_spans_with_the_spaces_between_them() {
            check_lines(
                &elrc("[00:01.00]<00:01.00>Hello <00:01.50>world<00:02.00>"),
                r#"<p begin="00:00:01.000" end="00:00:02.000" itunes:key="L1"><span begin="00:00:01.000" end="00:00:01.500">Hello</span> <span begin="00:00:01.500" end="00:00:02.000">world</span></p>"#,
            );
        }

        #[test]
        fn words_without_spaces_stay_together() {
            check_lines(
                &elrc("[00:01.00]<00:01.00>Hel<00:01.20>lo<00:01.40>"),
                r#"<p begin="00:00:01.000" end="00:00:01.400" itunes:key="L1"><span begin="00:00:01.000" end="00:00:01.200">Hel</span><span begin="00:00:01.200" end="00:00:01.400">lo</span></p>"#,
            );
        }

        #[test]
        fn a_timed_space_becomes_a_plain_space() {
            check_lines(
                &elrc("[00:01.00]<00:01.00>Hello<00:01.40> <00:01.50>world<00:02.00>"),
                r#"<p begin="00:00:01.000" end="00:00:02.000" itunes:key="L1"><span begin="00:00:01.000" end="00:00:01.400">Hello</span> <span begin="00:00:01.500" end="00:00:02.000">world</span></p>"#,
            );
        }

        #[test]
        fn text_before_the_first_tag_starts_with_the_line() {
            check_lines(
                &elrc("[00:01.00]Hello <00:01.50>world<00:02.00>"),
                r#"<p begin="00:00:01.000" end="00:00:02.000" itunes:key="L1"><span begin="00:00:01.000" end="00:00:01.500">Hello</span> <span begin="00:00:01.500" end="00:00:02.000">world</span></p>"#,
            );
        }

        #[test]
        fn an_unclosed_last_word_ends_with_the_next_line() {
            check_lines(
                &elrc(
                    "
                    [00:01.00]<00:01.00>Hello <00:01.50>world
                    [00:03.00]<00:03.00>Bye",
                ),
                r#"
                <p begin="00:00:01.000" end="00:00:03.000" itunes:key="L1"><span begin="00:00:01.000" end="00:00:01.500">Hello</span> <span begin="00:00:01.500" end="00:00:03.000">world</span></p>
                <p begin="00:00:03.000" itunes:key="L2"><span begin="00:00:03.000">Bye</span></p>"#,
            );
        }

        #[test]
        fn a_line_lasts_until_its_last_word() {
            check_lines(
                &elrc(
                    "
                    [00:01.00]<00:01.00>A<00:02.00>
                    [00:05.00]<00:05.00>B<00:06.00>",
                ),
                r#"
                <p begin="00:00:01.000" end="00:00:02.000" itunes:key="L1"><span begin="00:00:01.000" end="00:00:02.000">A</span></p>
                <p begin="00:00:05.000" end="00:00:06.000" itunes:key="L2"><span begin="00:00:05.000" end="00:00:06.000">B</span></p>"#,
            );
        }

        #[test]
        fn a_line_of_only_tags_is_a_stanza_break() {
            check(
                &elrc(
                    "
                    [00:01.00]<00:01.00>A<00:02.00>
                    [00:05.00]<00:05.00>
                    [00:09.00]<00:09.00>B<00:10.00>",
                ),
                r#"
                <tt itunes:timing="Word">
                  <body>
                    <div>
                      <p begin="00:00:01.000" end="00:00:02.000" itunes:key="L1"><span begin="00:00:01.000" end="00:00:02.000">A</span></p>
                    </div>
                    <div>
                      <p begin="00:00:09.000" end="00:00:10.000" itunes:key="L2"><span begin="00:00:09.000" end="00:00:10.000">B</span></p>
                    </div>
                  </body>
                </tt>"#,
            );
        }

        #[test]
        fn adjacent_tags_do_not_make_empty_words() {
            check_lines(
                &elrc("[00:01.00]<00:01.00><00:01.20>Hello<00:02.00>"),
                r#"<p begin="00:00:01.000" end="00:00:02.000" itunes:key="L1"><span begin="00:00:01.200" end="00:00:02.000">Hello</span></p>"#,
            );
        }

        #[test]
        fn an_angle_bracket_that_is_not_a_tag_is_text() {
            check_lines(
                &elrc("[00:01.00]<00:01.00>I <3 you<00:02.00>"),
                r#"<p begin="00:00:01.000" end="00:00:02.000" itunes:key="L1"><span begin="00:00:01.000" end="00:00:02.000">I &lt;3 you</span></p>"#,
            );
        }

        #[test]
        fn a_word_before_the_line_start_moves_the_line_start() {
            check_lines(
                &elrc("[00:02.00]<00:01.50>Hi<00:03.00>"),
                r#"<p begin="00:00:01.500" end="00:00:03.000" itunes:key="L1"><span begin="00:00:01.500" end="00:00:03.000">Hi</span></p>"#,
            );
        }

        #[test]
        fn repeated_timestamps_shift_the_words() {
            check_lines(
                &elrc("[00:10.00][00:40.00]<00:10.00>La <00:10.50>la<00:11.00>"),
                r#"
                <p begin="00:00:10.000" end="00:00:11.000" itunes:key="L1"><span begin="00:00:10.000" end="00:00:10.500">La</span> <span begin="00:00:10.500" end="00:00:11.000">la</span></p>
                <p begin="00:00:40.000" end="00:00:41.000" itunes:key="L2"><span begin="00:00:40.000" end="00:00:40.500">La</span> <span begin="00:00:40.500" end="00:00:41.000">la</span></p>"#,
            );
        }

        #[test]
        fn the_offset_shifts_the_words() {
            check_lines(
                &elrc(
                    "
                    [offset:1000]
                    [00:02.00]<00:02.00>Hi<00:03.00>",
                ),
                r#"<p begin="00:00:01.000" end="00:00:02.000" itunes:key="L1"><span begin="00:00:01.000" end="00:00:02.000">Hi</span></p>"#,
            );
        }

        #[test]
        fn a_word_never_ends_before_it_starts() {
            check_lines(
                &elrc("[00:03.00]<00:03.00>Hi<00:02.50>"),
                r#"<p begin="00:00:03.000" end="00:00:03.000" itunes:key="L1"><span begin="00:00:03.000" end="00:00:03.000">Hi</span></p>"#,
            );
        }
    }

    mod text {
        use super::*;

        #[test]
        fn markup_characters_are_escaped() {
            check_lines(
                &plain(r#"Rock & "roll" <3 > all"#),
                r#"<p itunes:key="L1">Rock &amp; &quot;roll&quot; &lt;3 &gt; all</p>"#,
            );
        }

        #[test]
        fn characters_xml_cannot_hold_are_dropped() {
            check(
                &Document::from_plain("Hel\u{0}lo\u{1b} wo\u{fffe}rld\u{7} 🎵\n\u{1}\u{2}\nBye"),
                r#"
                <tt itunes:timing="None">
                  <body>
                    <div>
                      <p itunes:key="L1">Hello world 🎵</p>
                    </div>
                    <div>
                      <p itunes:key="L2">Bye</p>
                    </div>
                  </body>
                </tt>"#,
            );
        }

        #[test]
        fn tabs_and_runs_of_spaces_become_single_spaces() {
            check_lines(
                &Document::from_elrc(
                    "[00:00.00]Hello\t\tbig\r  world !\n[00:01.00]<00:01.00>Hel\tlo<00:02.00>",
                ),
                r#"
                <p begin="00:00:00.000" end="00:00:01.000" itunes:key="L1">Hello big world !</p>
                <p begin="00:00:01.000" end="00:00:02.000" itunes:key="L2"><span begin="00:00:01.000" end="00:00:02.000">Hel lo</span></p>"#,
            );
        }
    }

    mod tracks {
        use super::*;

        fn hello_world() -> Document {
            lrc("
                [00:01.00]Hello
                [00:02.00]World")
        }

        #[test]
        fn a_translation_points_at_its_lines() {
            let mut doc = hello_world();
            doc.add_track_by_time(
                TrackKind::Translation,
                "en-GB",
                "[00:01.00]Hi\n[00:02.00]Earth",
            );

            assert!(doc.has_tracks());
            check(
                &doc,
                r#"
                <tt itunes:timing="Line">
                  <head>
                    <metadata>
                      <iTunesMetadata>
                        <translations>
                          <translation xml:lang="en-GB">
                            <text for="L1">Hi</text>
                            <text for="L2">Earth</text>
                          </translation>
                        </translations>
                      </iTunesMetadata>
                    </metadata>
                  </head>
                  <body>
                    <div>
                      <p begin="00:00:01.000" end="00:00:02.000" itunes:key="L1">Hello</p>
                      <p begin="00:00:02.000" itunes:key="L2">World</p>
                    </div>
                  </body>
                </tt>"#,
            );
        }

        #[test]
        fn translations_come_before_transliterations() {
            let mut doc = lrc("[00:01.00]Hello world!");
            doc.add_track_by_time(
                TrackKind::Transliteration,
                "und-Latn",
                "[00:01.00]Heh-loh wurld!",
            );
            doc.add_track_by_time(TrackKind::Translation, "en-GB", "[00:01.00]Hi world!");

            check_tracks(
                &doc,
                r#"
                <translations>
                  <translation xml:lang="en-GB">
                    <text for="L1">Hi world!</text>
                  </translation>
                </translations>
                <transliterations>
                  <transliteration xml:lang="und-Latn">
                    <text for="L1">Heh-loh wurld!</text>
                  </transliteration>
                </transliterations>"#,
            );
        }

        #[test]
        fn a_track_without_texts_is_not_added() {
            let mut doc = hello_world();
            doc.add_track_by_time(
                TrackKind::Translation,
                "en-GB",
                "[00:01.00]   \n[00:05.00]Far away",
            );
            doc.add_track_by_time(TrackKind::Transliteration, "en-GB", "");

            assert!(!doc.has_tracks());
            check_tracks(&doc, "");
        }

        #[test]
        fn blank_lines_get_no_key_and_no_text() {
            let mut doc = lrc("
                [00:01.00]A
                [00:02.00]
                [00:10.00]B");
            doc.add_track_by_time(
                TrackKind::Translation,
                "en-GB",
                "[00:01.00]a\n[00:02.00]gap\n[00:10.00]b",
            );

            check(
                &doc,
                r#"
                <tt itunes:timing="Line">
                  <head>
                    <metadata>
                      <iTunesMetadata>
                        <translations>
                          <translation xml:lang="en-GB">
                            <text for="L1">a</text>
                            <text for="L2">b</text>
                          </translation>
                        </translations>
                      </iTunesMetadata>
                    </metadata>
                  </head>
                  <body>
                    <div>
                      <p begin="00:00:01.000" end="00:00:02.000" itunes:key="L1">A</p>
                    </div>
                    <div>
                      <p begin="00:00:10.000" itunes:key="L2">B</p>
                    </div>
                  </body>
                </tt>"#,
            );
        }

        #[test]
        fn adding_the_same_language_again_fills_the_gaps() {
            let mut doc = hello_world();
            doc.add_track_by_time(TrackKind::Translation, "en-GB", "[00:01.00]Hi");
            doc.add_track_by_time(
                TrackKind::Translation,
                "EN-gb",
                "[00:01.00]Hey\n[00:02.00]Earth",
            );
            doc.add_track_by_time(TrackKind::Translation, "en-AU", "[00:01.00]Howdy");

            check_tracks(
                &doc,
                r#"
                <translations>
                  <translation xml:lang="en-GB">
                    <text for="L1">Hi</text>
                    <text for="L2">Earth</text>
                  </translation>
                  <translation xml:lang="en-AU">
                    <text for="L1">Howdy</text>
                  </translation>
                </translations>"#,
            );
        }

        #[test]
        fn the_same_language_of_another_kind_is_another_track() {
            let mut doc = hello_world();
            doc.add_track_by_time(TrackKind::Translation, "en-GB", "[00:01.00]Hi");
            doc.add_track_by_time(TrackKind::Transliteration, "en-GB", "[00:01.00]Heh-loh");

            check_tracks(
                &doc,
                r#"
                <translations>
                  <translation xml:lang="en-GB">
                    <text for="L1">Hi</text>
                  </translation>
                </translations>
                <transliterations>
                  <transliteration xml:lang="en-GB">
                    <text for="L1">Heh-loh</text>
                  </transliteration>
                </transliterations>"#,
            );
        }

        #[test]
        fn a_track_without_a_language_has_no_lang_attribute() {
            let mut doc = hello_world();
            doc.add_track_by_time(TrackKind::Translation, "  ", "[00:01.00]Hi");

            check_tracks(
                &doc,
                r#"
                <translations>
                  <translation>
                    <text for="L1">Hi</text>
                  </translation>
                </translations>"#,
            );
        }

        #[test]
        fn texts_and_languages_are_cleaned_and_escaped() {
            let mut doc = hello_world();
            doc.add_track_by_time(
                TrackKind::Translation,
                "a\"b\u{1}",
                "[00:01.00] Tom &\t\"Jerry\" <3 ",
            );

            check_tracks(
                &doc,
                r#"
                <translations>
                  <translation xml:lang="a&quot;b">
                    <text for="L1">Tom &amp; &quot;Jerry&quot; &lt;3</text>
                  </translation>
                </translations>"#,
            );
        }
    }

    mod matching {
        use super::*;

        #[track_caller]
        fn check_matches(mut doc: Document, translation: &str, expected: &str) {
            doc.add_track_by_time(TrackKind::Translation, "en-GB", &trim_indent(translation));
            check_tracks(&doc, expected);
        }

        #[test]
        fn texts_go_to_the_lines_with_the_same_timestamp() {
            check_matches(
                lrc("
                    [00:01.00]Hello
                    [00:02.00]Big
                    [00:03.00]World"),
                "
                [by:Someone]
                [00:03.00]Earth
                [00:01.00]Hi",
                r#"
                <translations>
                  <translation xml:lang="en-GB">
                    <text for="L1">Hi</text>
                    <text for="L3">Earth</text>
                  </translation>
                </translations>"#,
            );
        }

        #[test]
        fn small_differences_are_tolerated() {
            check_matches(
                lrc("
                    [00:17.432]A
                    [00:20.00]B
                    [00:30.00]C"),
                "
                [00:17.43]a
                [00:20.05]b
                [00:30.06]c",
                r#"
                <translations>
                  <translation xml:lang="en-GB">
                    <text for="L1">a</text>
                    <text for="L2">b</text>
                  </translation>
                </translations>"#,
            );
        }

        #[test]
        fn a_text_goes_to_the_closest_line() {
            check_matches(
                lrc("
                    [00:01.00]A
                    [00:01.04]B"),
                "[00:01.04]b",
                r#"
                <translations>
                  <translation xml:lang="en-GB">
                    <text for="L2">b</text>
                  </translation>
                </translations>"#,
            );
        }

        #[test]
        fn a_tie_goes_to_the_earlier_line() {
            check_matches(
                lrc("
                    [00:01.00]A
                    [00:01.04]B"),
                "[00:01.02]a",
                r#"
                <translations>
                  <translation xml:lang="en-GB">
                    <text for="L1">a</text>
                  </translation>
                </translations>"#,
            );
        }

        #[test]
        fn a_text_is_used_only_once() {
            check_matches(
                lrc("
                    [00:01.00]Hello
                    [00:01.00]Hello again"),
                "[00:01.00]Hi",
                r#"
                <translations>
                  <translation xml:lang="en-GB">
                    <text for="L1">Hi</text>
                  </translation>
                </translations>"#,
            );
        }

        #[test]
        fn a_text_with_several_timestamps_goes_to_every_repeat() {
            check_matches(
                lrc("[00:01.00][00:05.00]Chorus"),
                "[00:01.00][00:05.00]Refrain",
                r#"
                <translations>
                  <translation xml:lang="en-GB">
                    <text for="L1">Refrain</text>
                    <text for="L2">Refrain</text>
                  </translation>
                </translations>"#,
            );
        }

        #[test]
        fn the_offset_of_the_translation_is_applied() {
            check_matches(
                lrc("[00:01.00]Hello"),
                "
                [offset:500]
                [00:01.50]Hi",
                r#"
                <translations>
                  <translation xml:lang="en-GB">
                    <text for="L1">Hi</text>
                  </translation>
                </translations>"#,
            );
        }

        #[test]
        fn blank_texts_do_not_skip_real_ones() {
            check_matches(
                lrc("[00:01.00]Hello"),
                "
                [00:01.00]
                [00:01.02]Hi",
                r#"
                <translations>
                  <translation xml:lang="en-GB">
                    <text for="L1">Hi</text>
                  </translation>
                </translations>"#,
            );
        }

        #[test]
        fn word_synced_lines_are_matched_by_their_start() {
            check_matches(
                elrc("[00:02.00]<00:01.50>Hi<00:03.00>"),
                "[00:02.00]Hey",
                r#"
                <translations>
                  <translation xml:lang="en-GB">
                    <text for="L1">Hey</text>
                  </translation>
                </translations>"#,
            );
        }

        #[test]
        fn nothing_matches_without_timestamps() {
            check_matches(lrc("[00:01.00]Hello"), "Hi", "");
            check_matches(plain("Hello"), "[00:00.00]Hi", "");
        }
    }
}
