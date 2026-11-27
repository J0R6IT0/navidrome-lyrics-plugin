use nd_pdk::lyrics::TrackInfo;
use std::time::Duration;

pub trait TrackInfoExt {
    fn label(&self) -> String;
    fn has_artist(&self) -> bool;
    fn first_artist(&self) -> Option<&str>;
    fn all_artists(&self, separator: &str) -> String;
    fn duration(&self) -> Duration;
    fn clean_title(&self) -> String;
    fn matches_duration(&self, other: Duration, tolerance: Duration) -> bool;
    fn matches_album(&self, other: &str) -> bool;
}

impl TrackInfoExt for TrackInfo {
    fn label(&self) -> String {
        track_label(&self.id, &self.artist, &self.title)
    }

    fn has_artist(&self) -> bool {
        !self.artists.is_empty()
    }

    fn first_artist(&self) -> Option<&str> {
        self.artists.first().map(|a| a.name.as_str())
    }

    fn all_artists(&self, separator: &str) -> String {
        self.artists
            .iter()
            .map(|a| a.name.as_str())
            .collect::<Vec<_>>()
            .join(separator)
    }

    fn duration(&self) -> Duration {
        Duration::try_from_secs_f32(self.duration).unwrap_or_default()
    }

    fn clean_title(&self) -> String {
        let mut out = String::with_capacity(self.title.len());
        let mut depth = 0u32;

        for c in self.title.chars() {
            match c {
                '(' | '[' | '{' => depth += 1,
                ')' | ']' | '}' => depth = depth.saturating_sub(1),
                _ if depth == 0 => out.push(c),
                _ => {}
            }
        }

        out.split_whitespace()
            .take_while(|word| !matches!(*word, "-" | "‐" | "‒" | "–" | "—" | "―"))
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn matches_duration(&self, other: Duration, tolerance: Duration) -> bool {
        other.abs_diff(self.duration()) <= tolerance
    }

    fn matches_album(&self, other: &str) -> bool {
        albums_match(&self.album, other)
    }
}

pub fn track_label(id: &str, artist: &str, title: &str) -> String {
    match (artist.trim(), title.trim()) {
        ("", "") => id.to_string(),
        ("", title) => title.to_string(),
        (artist, "") => artist.to_string(),
        (artist, title) => format!("{artist} - {title}"),
    }
}

const ALBUM_STOPWORDS: &[&str] = &["the", "a", "an", "and"];

const EDITION_TERMS: &[&str] = &[
    "deluxe",
    "edition",
    "remaster",
    "remastered",
    "anniversary",
    "expanded",
    "bonus",
    "special",
    "explicit",
    "clean",
    "mono",
    "stereo",
    "digital",
    "vinyl",
    "reissue",
    "version",
    "ver",
    "collectors",
    "disc",
    "cd",
    "part",
    "volume",
    "vol",
    "track",
    "tracks",
    "of",
    "from",
    "super",
    "box",
    "set",
    "complete",
    "ultimate",
    "definitive",
    "legacy",
    "platinum",
    "gold",
    "single",
    "ep",
    "album",
    "黑胶版", // "vinyl edition", seen from chinese providers
];

const VERSION_MARKERS: &[&str] = &[
    "live",
    "remix",
    "acoustic",
    "instrumental",
    "demo",
    "karaoke",
    "cover",
    "unplugged",
    "session",
    "alternate",
    "bootleg",
    "extended",
    "orchestral",
    "reimagined",
];

const ALBUM_MIN_MATCHED_PERCENT: usize = 80;

fn albums_match(mine: &str, other: &str) -> bool {
    let mine = normalize_album(mine);
    let other = normalize_album(other);

    if mine.is_empty() || other.is_empty() {
        return false;
    }
    if mine == other {
        return true;
    }

    let (mine_numbers, mine_words) = significant_terms(&mine);
    let (other_numbers, other_words) = significant_terms(&other);

    if mine_numbers != other_numbers {
        return false;
    }

    let (smaller, larger) = if mine_words.len() <= other_words.len() {
        (&mine_words, &other_words)
    } else {
        (&other_words, &mine_words)
    };

    if smaller.is_empty() {
        return larger.is_empty();
    }

    if smaller.len() == 1 && mine_numbers.is_empty() {
        return larger.len() == 1 && words_similar(smaller[0], larger[0]);
    }

    if version_markers_differ(smaller, larger) {
        return false;
    }

    let mut unmatched = larger.clone();
    let matched = smaller
        .iter()
        .filter(|w| {
            let similar = unmatched.iter().position(|o| words_similar(w, o));
            similar.map(|i| unmatched.swap_remove(i)).is_some()
        })
        .count();

    matched * 100 >= smaller.len() * ALBUM_MIN_MATCHED_PERCENT
}

fn normalize_album(album: &str) -> String {
    let album = strip_edition_brackets(&album.to_lowercase());
    words(&album).collect::<Vec<_>>().join(" ")
}

/// Drops bracketed edition annotations and unwraps any other bracketed text.
fn strip_edition_brackets(mut text: &str) -> String {
    let mut out = String::with_capacity(text.len());

    while let Some((before, inner, after)) = split_first_bracket_group(text) {
        out.push_str(before);
        if !is_edition_annotation(inner) {
            out.push(' ');
            out.push_str(inner);
            out.push(' ');
        }
        text = after;
    }

    out.push_str(text);
    out
}

/// Splits `text` around its first balanced bracket group, as `(before, inside, after)`.
fn split_first_bracket_group(text: &str) -> Option<(&str, &str, &str)> {
    text.char_indices().find_map(|(start, open)| {
        let close = closing_bracket(open)?;
        let inner_start = start + open.len_utf8();
        let mut depth = 0u32;

        for (offset, c) in text[inner_start..].char_indices() {
            if c == open {
                depth += 1;
            } else if c == close {
                if depth == 0 {
                    let end = inner_start + offset;
                    return Some((
                        &text[..start],
                        &text[inner_start..end],
                        &text[end + close.len_utf8()..],
                    ));
                }
                depth -= 1;
            }
        }
        None
    })
}

fn closing_bracket(open: char) -> Option<char> {
    Some(match open {
        '(' => ')',
        '[' => ']',
        '{' => '}',
        '（' => '）',
        '【' => '】',
        _ => return None,
    })
}

fn is_edition_annotation(text: &str) -> bool {
    let mut words = words(text).peekable();
    words.peek().is_some()
        && words.all(|w| EDITION_TERMS.contains(&w) || w.starts_with(|c: char| c.is_ascii_digit()))
}

fn words(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
}

/// Splits a normalized album into its sorted numeric tokens and its distinctive words.
fn significant_terms(album: &str) -> (Vec<&str>, Vec<&str>) {
    let (mut numbers, words): (Vec<_>, Vec<_>) = album
        .split_whitespace()
        .filter(|w| !ALBUM_STOPWORDS.contains(w) && !EDITION_TERMS.contains(w))
        .partition(|w| w.contains(|c: char| c.is_ascii_digit()));
    numbers.sort_unstable();
    (numbers, words)
}

fn version_markers_differ(a: &[&str], b: &[&str]) -> bool {
    let has_unmatched_marker = |words: &[&str], others: &[&str]| {
        words
            .iter()
            .any(|w| VERSION_MARKERS.contains(w) && !others.iter().any(|o| words_similar(w, o)))
    };
    has_unmatched_marker(a, b) || has_unmatched_marker(b, a)
}

fn words_similar(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }

    let longest = a.chars().count().max(b.chars().count());
    longest >= 4 && levenshtein(a, b) * 5 <= longest
}

fn levenshtein(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut curr = vec![0; b.len() + 1];

    for (i, ca) in a.chars().enumerate() {
        curr[0] = i + 1;
        for (j, &cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            curr[j + 1] = (prev[j + 1] + 1).min(curr[j] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }

    prev[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[track_caller]
    fn check_label(id: &str, artist: &str, title: &str, expected: &str) {
        let track = TrackInfo {
            id: id.to_string(),
            artist: artist.to_string(),
            title: title.to_string(),
            ..Default::default()
        };
        assert_eq!(track.label(), expected);
    }

    #[track_caller]
    fn check_duration(secs: f32, expected: Duration) {
        let track = TrackInfo {
            duration: secs,
            ..Default::default()
        };
        assert_eq!(track.duration(), expected, "{secs}");
    }

    #[track_caller]
    fn check_clean_title(title: &str, expected: &str) {
        let track = TrackInfo {
            title: title.to_string(),
            ..Default::default()
        };
        assert_eq!(track.clean_title(), expected, "{title:?}");
    }

    #[track_caller]
    fn check_album_match(a: &str, b: &str, expected: bool) {
        for (mine, other) in [(a, b), (b, a)] {
            let track = TrackInfo {
                album: mine.to_string(),
                ..Default::default()
            };
            assert_eq!(
                track.matches_album(other),
                expected,
                "{mine:?} vs {other:?}"
            );
        }
    }

    #[test]
    fn label_uses_whatever_is_tagged() {
        check_label("id", "Artist", "Song", "Artist - Song");
        check_label("id", " ", "Song", "Song");
        check_label("id", "Artist", "", "Artist");
        check_label("id", "", "", "id");
    }

    #[test]
    fn duration_is_in_seconds() {
        check_duration(215.5, Duration::from_millis(215_500));
        check_duration(0.0, Duration::ZERO);
    }

    #[test]
    fn invalid_duration_is_zero() {
        check_duration(-1.0, Duration::ZERO);
        check_duration(f32::NAN, Duration::ZERO);
        check_duration(f32::INFINITY, Duration::ZERO);
        check_duration(1e20, Duration::ZERO);
    }

    #[test]
    fn clean_title_drops_brackets() {
        check_clean_title("Song (Live)", "Song");
        check_clean_title("Song [Remastered 2020]", "Song");
        check_clean_title("Song {Deluxe}", "Song");
        check_clean_title("Song (feat. Artist [Live])", "Song");
        check_clean_title("Song [Remix] (2020)", "Song");
    }

    #[test]
    fn clean_title_drops_dash_suffix() {
        check_clean_title("Song - Remastered 2011", "Song");
        check_clean_title("Song - Live - 2011 Remaster", "Song");
        check_clean_title("Song \u{2013} Live", "Song");
        check_clean_title("Song \u{2014} Live", "Song");
        check_clean_title("Song (Live) - Remastered", "Song");
    }

    #[test]
    fn clean_title_keeps_attached_dashes() {
        check_clean_title("Song-Title", "Song-Title");
        check_clean_title("Song -Title", "Song -Title");
    }

    #[test]
    fn clean_title_collapses_whitespace() {
        check_clean_title("Song   (Live)   Version", "Song Version");
    }

    #[test]
    fn clean_title_keeps_plain_titles() {
        check_clean_title("Song Title", "Song Title");
        check_clean_title("", "");
    }

    #[test]
    fn albums_match_ignoring_case_and_punctuation() {
        check_album_match("Born to Run", "born to run", true);
        check_album_match("Born to Run", "Born, To Run!", true);
    }

    #[test]
    fn albums_match_ignoring_edition() {
        check_album_match(
            "Born to Run",
            "Born to Run (30th Anniversary Edition)",
            true,
        );
        check_album_match("Born to Run", "Born To Run - Deluxe Edition", true);
        check_album_match("Born to Run", "Born to Run (Remastered)", true);
        check_album_match("Nevermind", "Nevermind (Super Deluxe Edition)", true);
        check_album_match("Metallica", "Metallica (Deluxe Box Set)", true);
        check_album_match("Fearless", "Fearless (Platinum Edition)", true);
        check_album_match("Paradise", "Paradise - Single", true);
        check_album_match("Some Title", "Some Title - EP", true);
    }

    #[test]
    fn albums_match_ignoring_artist_prefix() {
        check_album_match("Hotel California", "Eagles - Hotel California", true);
        check_album_match("Bohemian Rhapsody", "Queen - Bohemian Rhapsody", true);
    }

    #[test]
    fn albums_match_despite_typos() {
        check_album_match("Highway 61 Revisited", "Highway 61 Revisted", true);
    }

    #[test]
    fn albums_match_with_fewer_or_reordered_words() {
        check_album_match(
            "Bruce Springsteen & The E Street Band Live 1975-85",
            "Live/1975-85",
            true,
        );
        check_album_match("The Beatles", "Beatles, The", true);
        check_album_match("Thunder Road (Live)", "Thunder Road Live", true);
    }

    #[test]
    fn different_albums_do_not_match() {
        check_album_match("Born to Run", "Born in the U.S.A.", false);
        check_album_match("Highway 61 Revisited", "Highway to Hell", false);
    }

    #[test]
    fn other_versions_do_not_match() {
        check_album_match("Fearless", "Fearless (Taylor's Version)", false);
        check_album_match(
            "Ultimate Singer-Songwriters",
            "Ultimate Singer-Songwriters (Live)",
            false,
        );
        check_album_match("Born to Run", "Born to Run Live", false);
        check_album_match("Dynamite", "Dynamite (Remix)", false);
        check_album_match(
            "Dynamite (DayTime Ver.)",
            "Dynamite (NightTime Ver.)",
            false,
        );
        check_album_match("Live in New York City", "Love in New York City", false);
    }

    #[test]
    fn numbers_must_match_exactly() {
        check_album_match(
            "Bruce Springsteen & The E Street Band Live 1975-85",
            "Bruce Springsteen & The E Street Band Live 1978-85",
            false,
        );
        check_album_match("Live at Wembley 1985", "Live at Wembley", false);
        check_album_match("Hits 1 1 2", "Hits 1 2 2", false);
    }

    #[test]
    fn repeated_words_match_once() {
        check_album_match("Love Love Love", "Love Me Tender", false);
        check_album_match("Bang Bang", "Bang Your Head", false);
    }

    #[test]
    fn short_generic_names_do_not_match_longer_ones() {
        check_album_match("Live", "Wrecking Ball Live", false);
        check_album_match("Meet the Beatles", "The Beatles", false);
    }

    #[test]
    fn empty_albums_never_match() {
        check_album_match("", "Born to Run", false);
        check_album_match("", "", false);
    }
}
