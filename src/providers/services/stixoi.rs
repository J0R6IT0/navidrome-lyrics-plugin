use crate::{
    config::{PluginConfig, ProviderParams},
    ext::TrackInfoExt,
    providers::{LyricsProvider, ProviderLyrics, ProviderResult, error::ProviderError, http::Http},
    types::{Lyrics, LyricsKind},
};
use nd_pdk::lyrics::TrackInfo;
use regex::Regex;
use serde::Deserialize;
use std::collections::HashSet;

const BASE_URL: &str = "https://stixoi.info";

const MAX_PROBE: usize = 8;

#[derive(Debug, Default, Deserialize, PartialEq)]
struct SongPage {
    #[serde(default)]
    title: String,
    #[serde(default)]
    lyrics: String,
    #[serde(default)]
    singers: Vec<String>,
    #[serde(default)]
    composers: Vec<String>,
    #[serde(default)]
    lyricists: Vec<String>,
}

impl SongPage {
    fn credits(&self) -> Vec<&str> {
        self.singers
            .iter()
            .chain(&self.composers)
            .chain(&self.lyricists)
            .map(String::as_str)
            .collect()
    }
}

pub struct Stixoi;

impl Stixoi {
    pub fn create(_params: &ProviderParams) -> Box<dyn LyricsProvider> {
        Box::new(Self)
    }

    fn find_lyrics(
        &self,
        query: &str,
        title: &str,
        artist: &str,
    ) -> ProviderResult<Option<String>> {
        let candidate_ids = self.search(query)?;

        for id in candidate_ids.iter().take(MAX_PROBE) {
            let Some(song) = self.get(id)? else {
                continue;
            };

            if song.lyrics.trim().is_empty() || !title_equal(title, &song.title) {
                continue;
            }

            if artist_matches(artist, &song.credits()) {
                return Ok(Some(song.lyrics));
            }
        }

        Ok(None)
    }

    fn search(&self, query: &str) -> ProviderResult<Vec<String>> {
        let response = Http::get(format!("{BASE_URL}/search"))
            .browser()
            .header("RSC", "1")
            .param("q", query)
            .param("scope", "songs")
            .send()?;

        match response.status {
            200 => extract_song_ids(&response.text()),
            429 => Err(response.rate_limited()),
            _ => Err(response.unexpected_status("the search")),
        }
    }

    fn get(&self, id: &str) -> ProviderResult<Option<SongPage>> {
        let response = Http::get(format!("{BASE_URL}/songs/{id}"))
            .browser()
            .header("RSC", "1")
            .send()?;

        match response.status {
            200 => parse_song_page(&response.text(), id),
            404 => Ok(None),
            429 => Err(response.rate_limited()),
            _ => Err(response.unexpected_status("the song page")),
        }
    }
}

impl LyricsProvider for Stixoi {
    fn supported_kinds(&self) -> &'static [LyricsKind] {
        &[LyricsKind::Plain]
    }

    fn fetch_lyrics(
        &self,
        track: &TrackInfo,
        _cfg: &PluginConfig,
    ) -> ProviderResult<Option<ProviderLyrics>> {
        let title = track.clean_title();
        if title.is_empty() {
            return Ok(None);
        }

        let artist = track.first_artist().unwrap_or_default();

        let combined = if artist.is_empty() {
            title.clone()
        } else {
            format!("{title} {artist}")
        };

        if let Some(lyrics) = self.find_lyrics(&combined, &title, artist)? {
            return Ok(Some(Lyrics::Plain(lyrics).into()));
        }

        if combined != title
            && let Some(lyrics) = self.find_lyrics(&title, &title, artist)?
        {
            return Ok(Some(Lyrics::Plain(lyrics).into()));
        }

        Ok(None)
    }
}

fn extract_song_ids(body: &str) -> ProviderResult<Vec<String>> {
    let song_link_re = Regex::new(r#""href":"/songs/(\d+)""#)
        .map_err(|e| ProviderError::other(format!("invalid song link regex: {e}")))?;

    let mut ids = Vec::new();
    for cap in song_link_re.captures_iter(body) {
        if let Some(m) = cap.get(1) {
            let id = m.as_str().to_string();
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
    }

    Ok(ids)
}

fn parse_song_page(body: &str, id: &str) -> ProviderResult<Option<SongPage>> {
    let anchor = format!(r#""song":{{"id":{id}"#);
    let Some(song_json) = extract_json_object(body, &anchor) else {
        return Ok(None);
    };

    let mut song: SongPage = serde_json::from_str(song_json)
        .map_err(|e| ProviderError::other(format!("failed to parse the song payload: {e}")))?;

    if let Some(resolved) = resolve_rsc_reference(body, &song.lyrics) {
        song.lyrics = resolved;
    }

    Ok(Some(song))
}

fn extract_json_object<'a>(haystack: &'a str, anchor: &str) -> Option<&'a str> {
    let start = find_object_start(haystack, anchor)?;
    let end = find_object_end(haystack, start)?;
    Some(&haystack[start..end])
}

fn find_object_start(haystack: &str, anchor: &str) -> Option<usize> {
    let anchor_pos = haystack.find(anchor)?;
    let brace_offset = haystack[anchor_pos..].find('{')?;
    Some(anchor_pos + brace_offset)
}

fn find_object_end(haystack: &str, start: usize) -> Option<usize> {
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escaped = false;

    for (offset, &byte) in haystack.as_bytes()[start..].iter().enumerate() {
        if in_string {
            match byte {
                b'\\' if !escaped => escaped = true,
                b'"' if !escaped => in_string = false,
                _ => escaped = false,
            }
            continue;
        }

        match byte {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(start + offset + 1);
                }
            }
            _ => {}
        }
    }

    None
}

fn resolve_rsc_reference(body: &str, value: &str) -> Option<String> {
    let chunk_id = value.strip_prefix('$')?;
    if chunk_id.is_empty() || !chunk_id.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }

    let marker = format!("{chunk_id}:T");
    let marker_pos = find_line_start(body, &marker)?;
    let after_marker = &body[marker_pos + marker.len()..];
    let comma = after_marker.find(',')?;
    let hexlen = usize::from_str_radix(&after_marker[..comma], 16).ok()?;

    let text_start = marker_pos + marker.len() + comma + 1;
    let text_bytes = body.as_bytes().get(text_start..)?;
    let chunk = text_bytes.get(..hexlen)?;
    String::from_utf8(chunk.to_vec()).ok()
}

/// Finds `marker` only where it starts a line, to avoid matching
/// unrelated content.
fn find_line_start(haystack: &str, marker: &str) -> Option<usize> {
    let mut search_from = 0;
    loop {
        let idx = haystack[search_from..].find(marker)? + search_from;
        if idx == 0 || haystack.as_bytes()[idx - 1] == b'\n' {
            return Some(idx);
        }
        search_from = idx + 1;
    }
}

fn fold(c: char) -> char {
    match c {
        'ά' => 'α',
        'έ' => 'ε',
        'ή' => 'η',
        'ί' | 'ϊ' | 'ΐ' => 'ι',
        'ό' => 'ο',
        'ύ' | 'ϋ' | 'ΰ' => 'υ',
        'ώ' => 'ω',
        'ς' => 'σ',

        'á' | 'à' | 'â' | 'ä' | 'ã' | 'å' => 'a',
        'é' | 'è' | 'ê' | 'ë' => 'e',
        'í' | 'ì' | 'î' | 'ï' => 'i',
        'ó' | 'ò' | 'ô' | 'ö' | 'õ' => 'o',
        'ú' | 'ù' | 'û' | 'ü' => 'u',
        'ñ' => 'n',
        'ç' => 'c',
        other => other,
    }
}

fn normalize(s: &str) -> String {
    s.chars()
        .flat_map(char::to_lowercase)
        .map(fold)
        .filter(|c| c.is_alphanumeric())
        .collect()
}

fn tokens(s: &str) -> Vec<String> {
    s.chars()
        .flat_map(char::to_lowercase)
        .map(fold)
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .map(collapse_doubles)
        .collect()
}

fn collapse_doubles(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last = None;

    for c in s.chars() {
        if Some(c) != last {
            out.push(c);
        }
        last = Some(c);
    }

    out
}

fn title_equal(a: &str, b: &str) -> bool {
    let a = normalize(a);
    !a.is_empty() && a == normalize(b)
}

fn artist_matches(artist: &str, credits: &[&str]) -> bool {
    let want = tokens(artist);
    if want.is_empty() {
        return false;
    }

    credits.iter().any(|name| {
        let have: HashSet<String> = tokens(name).into_iter().collect();
        want.iter().all(|t| have.contains(t))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[track_caller]
    fn check_artist_matches(artist: &str, credits: &[&str], expected: bool) {
        assert_eq!(
            artist_matches(artist, credits),
            expected,
            "{artist:?} against {credits:?}"
        );
    }

    #[test]
    fn an_artist_matches_regardless_of_word_order() {
        check_artist_matches("Δήμητρα Γαλάνη", &["Γαλάνη Δήμητρα"], true);
    }

    #[test]
    fn a_doubled_consonant_spelling_variant_still_matches() {
        check_artist_matches("Νατάσσα Μποφίλιου", &["Μποφίλιου Νατάσα"], true);
    }

    #[test]
    fn an_unrelated_artist_does_not_match() {
        check_artist_matches("Νατάσσα Μποφίλιου", &["Ρουβάς Σάκης"], false);
    }

    #[track_caller]
    fn check_song_ids(body: &str, expected: &[&str]) {
        let actual = extract_song_ids(body).unwrap();
        let expected: Vec<String> = expected.iter().map(|s| s.to_string()).collect();
        assert_eq!(actual, expected);
    }

    #[test]
    fn song_ids_are_extracted_from_rsc_chunk() {
        check_song_ids(
            r#"1:["$","a",null,{"href":"/songs/101","children":"Song A"}]
2:["$","a",null,{"href":"/songs/202","children":"Song B"}]"#,
            &["101", "202"],
        );
    }

    #[test]
    fn song_ids_are_deduplicated() {
        check_song_ids(
            r#"{"href":"/songs/5"} ... {"href":"/songs/5"} ... {"href":"/songs/9"}"#,
            &["5", "9"],
        );
    }

    #[test]
    fn song_ids_ignore_unrelated_links() {
        check_song_ids(
            r#"{"href":"/artists/9"} {"href":"/songs/42"} {"href":"/about"}"#,
            &["42"],
        );
    }

    #[test]
    fn song_ids_empty_for_no_results() {
        check_song_ids(r#"1:{"results":[],"count":0}"#, &[]);
    }

    #[track_caller]
    fn check_parsed_song(body: &str, id: &str, expected: SongPage) {
        assert_eq!(parse_song_page(body, id).unwrap(), Some(expected));
    }

    #[track_caller]
    fn check_song_not_found(body: &str, id: &str) {
        assert_eq!(parse_song_page(body, id).unwrap(), None);
    }

    #[track_caller]
    fn check_song_parse_fails(body: &str, id: &str) {
        assert!(parse_song_page(body, id).is_err());
    }

    #[test]
    fn parses_a_full_song_out_of_a_realistic_page_body() {
        let body = r#"1:["$","div",null,{"count":1,"song":{"id":777,"title":"Ένα Τραγούδι","lyrics":"Πρώτη γραμμή\nΔεύτερη γραμμή","singers":["Χάρις Αλεξίου"],"composers":["Μάνος Χατζιδάκις"],"lyricists":["Νίκος Γκάτσος"]}}]"#;

        check_parsed_song(
            body,
            "777",
            SongPage {
                title: "Ένα Τραγούδι".to_string(),
                lyrics: "Πρώτη γραμμή\nΔεύτερη γραμμή".to_string(),
                singers: vec!["Χάρις Αλεξίου".to_string()],
                composers: vec!["Μάνος Χατζιδάκις".to_string()],
                lyricists: vec!["Νίκος Γκάτσος".to_string()],
            },
        );
    }

    #[test]
    fn missing_fields_default_to_empty() {
        let body = r#"{"song":{"id":42,"unrelatedField":true}}"#;
        check_parsed_song(body, "42", SongPage::default());
    }

    #[test]
    fn returns_none_when_the_id_is_not_present() {
        let body = r#"1:["$","div",null,{"count":0}]"#;
        check_song_not_found(body, "777");
    }

    #[test]
    fn returns_none_for_a_truncated_payload() {
        let body = r#"{"song":{"id":5,"title":"Broken"#;
        check_song_not_found(body, "5");
    }

    #[test]
    fn an_escaped_brace_inside_a_string_does_not_end_the_object_early() {
        let body = r#"{"song":{"id":9,"title":"He said \"Hello}\" world","lyrics":"","singers":[],"composers":[],"lyricists":[]}}"#;

        check_parsed_song(
            body,
            "9",
            SongPage {
                title: "He said \"Hello}\" world".to_string(),
                ..SongPage::default()
            },
        );
    }

    #[test]
    fn invalid_json_is_an_error() {
        let body = r#"{"song":{"id":6,"title":"X",}}"#;
        check_song_parse_fails(body, "6");
    }

    #[test]
    fn lyrics_streamed_as_an_rsc_chunk_are_resolved() {
        let body = concat!(
            r#"1:["$","div",null,{"song":{"id":777,"title":"Title","lyrics":"$50","singers":[],"composers":[],"lyricists":[]}}]"#,
            "\n50:Td,Line 1\nLine 2\n",
        );

        check_parsed_song(
            body,
            "777",
            SongPage {
                title: "Title".to_string(),
                lyrics: "Line 1\nLine 2".to_string(),
                ..SongPage::default()
            },
        );
    }

    #[track_caller]
    fn check_resolve_rsc_reference(body: &str, value: &str, expected: Option<&str>) {
        assert_eq!(resolve_rsc_reference(body, value).as_deref(), expected);
    }

    #[test]
    fn an_rsc_reference_is_resolved_from_its_chunk() {
        check_resolve_rsc_reference(
            "50:Td,Line 1\nLine 2\ntrailer",
            "$50",
            Some("Line 1\nLine 2"),
        );
    }

    #[test]
    fn an_rsc_reference_chunk_id_can_be_hex() {
        check_resolve_rsc_reference("4f:T5,hello\ntrailer", "$4f", Some("hello"));
    }

    #[test]
    fn a_chunk_marker_is_only_matched_at_the_start_of_a_line() {
        check_resolve_rsc_reference("x50:Td,not this one\n", "$50", None);
    }

    #[test]
    fn a_plain_lyrics_value_is_not_treated_as_a_reference() {
        check_resolve_rsc_reference("50:Td,Line 1\nLine 2\n", "Πρώτη γραμμή", None);
    }

    #[test]
    fn a_missing_chunk_resolves_to_nothing() {
        check_resolve_rsc_reference("no chunks here", "$50", None);
    }

    #[track_caller]
    fn check_extract_json_object(haystack: &str, anchor: &str, expected: Option<&str>) {
        assert_eq!(extract_json_object(haystack, anchor), expected);
    }

    #[test]
    fn extract_json_object_finds_the_object_after_the_anchor() {
        check_extract_json_object(
            r#"prefix "song":{"id":70,"title":"A","nested":{"x":1}} suffix"#,
            r#""song":{"id":70"#,
            Some(r#"{"id":70,"title":"A","nested":{"x":1}}"#),
        );
    }

    #[test]
    fn extract_json_object_ignores_braces_inside_strings() {
        check_extract_json_object(
            r#""song":{"id":1,"lyrics":"a {b} \"c\" }","ok":true}"#,
            r#""song":{"id":1"#,
            Some(r#"{"id":1,"lyrics":"a {b} \"c\" }","ok":true}"#),
        );
    }

    #[test]
    fn extract_json_object_none_when_the_anchor_is_missing() {
        check_extract_json_object("nothing here", r#""song":{"id":9"#, None);
    }
}
