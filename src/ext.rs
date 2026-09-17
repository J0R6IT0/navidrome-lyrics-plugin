use nd_pdk::lyrics::TrackInfo;
use std::time::Duration;

const ALBUM_STOPWORDS: &[&str] = &["the", "a", "an", "and", "of", "&"];

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

const SAFE_EDITION_TERMS: &[&str] = &[
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

const ALBUM_MATCH_THRESHOLD: f64 = 0.8;

pub trait TrackInfoExt {
    fn label(&self) -> String;
    fn has_artist(&self) -> bool;
    fn first_artist(&self) -> Option<&str>;
    fn all_artists(&self) -> String;
    fn duration(&self) -> Duration;
    fn clean_title(&self) -> String;
    fn matches_duration(&self, other: Duration, tolerance: Duration) -> bool;
    fn matches_album(&self, other: &str) -> bool;
}

impl TrackInfoExt for TrackInfo {
    fn label(&self) -> String {
        match (self.artist.trim(), self.title.trim()) {
            ("", "") => self.id.clone(),
            ("", title) => title.to_string(),
            (artist, "") => artist.to_string(),
            (artist, title) => format!("{artist} - {title}"),
        }
    }

    fn has_artist(&self) -> bool {
        !self.artists.is_empty()
    }

    fn first_artist(&self) -> Option<&str> {
        self.artists.first().map(|a| a.name.as_str())
    }

    fn all_artists(&self) -> String {
        self.artists
            .iter()
            .map(|a| a.name.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn duration(&self) -> Duration {
        Duration::from_secs_f32(self.duration.max(0.0))
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

fn strip_or_unwrap_brackets(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;

    while i < chars.len() {
        if is_open_bracket(chars[i])
            && let Some(end) = matching_close(&chars, i)
        {
            let inner: String = chars[i + 1..end].iter().collect();
            if !is_safe_to_discard(&inner) {
                out.push(' ');
                out.push_str(&inner);
                out.push(' ');
            }
            i = end + 1;
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }

    out
}

fn is_open_bracket(c: char) -> bool {
    matches!(c, '(' | '[' | '{' | '（' | '【')
}

fn matching_close(chars: &[char], open_index: usize) -> Option<usize> {
    let (open, close) = match chars[open_index] {
        '(' => ('(', ')'),
        '[' => ('[', ']'),
        '（' => ('（', '）'),
        '【' => ('【', '】'),
        _ => ('{', '}'),
    };

    let mut depth = 0u32;
    for (offset, &c) in chars[open_index..].iter().enumerate() {
        if c == open {
            depth += 1;
        } else if c == close {
            depth -= 1;
            if depth == 0 {
                return Some(open_index + offset);
            }
        }
    }
    None
}

fn is_safe_to_discard(s: &str) -> bool {
    let lower = s.to_lowercase();
    let words: Vec<&str> = lower
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();

    !words.is_empty()
        && words.iter().all(|w| {
            SAFE_EDITION_TERMS.contains(w) || w.chars().next().is_some_and(|c| c.is_ascii_digit())
        })
}

fn normalize(s: &str) -> String {
    strip_or_unwrap_brackets(s)
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn is_numeric_token(word: &str) -> bool {
    word.chars().any(|c| c.is_ascii_digit())
}

fn word_similar(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }

    let max_len = a.chars().count().max(b.chars().count());
    if max_len < 4 {
        return false;
    }

    levenshtein(a, b) * 5 <= max_len
}

fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let (n, m) = (a.len(), b.len());

    if n == 0 {
        return m;
    }
    if m == 0 {
        return n;
    }

    let mut prev: Vec<usize> = (0..=m).collect();
    let mut curr = vec![0; m + 1];

    for i in 1..=n {
        curr[0] = i;
        for j in 1..=m {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            curr[j] = (prev[j] + 1).min(curr[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }

    prev[m]
}

fn albums_match(mine: &str, other: &str) -> bool {
    let mine = normalize(mine);
    let other = normalize(other);

    if mine.is_empty() || other.is_empty() {
        return false;
    }
    if mine == other {
        return true;
    }

    let mine_tokens: Vec<&str> = mine
        .split_whitespace()
        .filter(|w| !ALBUM_STOPWORDS.contains(w))
        .collect();
    let other_tokens: Vec<&str> = other
        .split_whitespace()
        .filter(|w| !ALBUM_STOPWORDS.contains(w))
        .collect();

    let mine_numbers: Vec<&str> = mine_tokens
        .iter()
        .copied()
        .filter(|w| is_numeric_token(w))
        .collect();
    let other_numbers: Vec<&str> = other_tokens
        .iter()
        .copied()
        .filter(|w| is_numeric_token(w))
        .collect();

    if !same_elements(&mine_numbers, &other_numbers) {
        return false;
    }

    let mine_words: Vec<&str> = mine_tokens
        .into_iter()
        .filter(|w| !is_numeric_token(w) && !SAFE_EDITION_TERMS.contains(w))
        .collect();
    let other_words: Vec<&str> = other_tokens
        .into_iter()
        .filter(|w| !is_numeric_token(w) && !SAFE_EDITION_TERMS.contains(w))
        .collect();

    let (smaller, larger) = if mine_words.len() <= other_words.len() {
        (&mine_words, &other_words)
    } else {
        (&other_words, &mine_words)
    };

    if smaller.is_empty() {
        return larger.is_empty();
    }

    if smaller.len() < 2 && mine_numbers.is_empty() {
        return larger.len() == 1 && word_similar(smaller[0], larger[0]);
    }

    if larger
        .iter()
        .any(|w| VERSION_MARKERS.contains(w) && !smaller.iter().any(|s| word_similar(s, w)))
    {
        return false;
    }

    let matched = smaller
        .iter()
        .filter(|w| larger.iter().any(|o| word_similar(w, o)))
        .count();

    matched as f64 / smaller.len() as f64 >= ALBUM_MATCH_THRESHOLD
}

fn same_elements(a: &[&str], b: &[&str]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().all(|x| b.contains(x))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(title: &str) -> TrackInfo {
        TrackInfo {
            title: title.to_string(),
            ..Default::default()
        }
    }

    fn track_with_album(album: &str) -> TrackInfo {
        TrackInfo {
            album: album.to_string(),
            ..Default::default()
        }
    }

    #[track_caller]
    fn check_clean_title(input: &str, expected: &str) {
        assert_eq!(track(input).clean_title(), expected, "title {input:?}");
    }

    #[track_caller]
    fn check_album_match(mine: &str, other: &str, expected: bool) {
        assert_eq!(
            track_with_album(mine).matches_album(other),
            expected,
            "{mine:?} vs {other:?}"
        );
    }

    #[test]
    fn a_clean_title_drops_bracketed_segments() {
        check_clean_title("Song (Live)", "Song");
        check_clean_title("Song [Remastered 2020]", "Song");
        check_clean_title("Song {Deluxe}", "Song");
        check_clean_title("Song (feat. Artist [Live])", "Song");
        check_clean_title("Song [Remix] (2020)", "Song");
    }

    #[test]
    fn a_clean_title_drops_dash_suffixes() {
        check_clean_title("Song - Remastered 2011", "Song");
        check_clean_title("Song - Live - 2011 Remaster", "Song");
        check_clean_title("Song \u{2013} Live", "Song");
        check_clean_title("Song \u{2014} Live", "Song");
        check_clean_title("Song (Live) - Remastered", "Song");
    }

    #[test]
    fn a_clean_title_keeps_dashes_inside_words() {
        check_clean_title("Song-Title", "Song-Title");
        check_clean_title("Song -Title", "Song -Title");
    }

    #[test]
    fn a_clean_title_collapses_whitespace() {
        check_clean_title("Song   (Live)   Version", "Song Version");
    }

    #[test]
    fn a_clean_title_leaves_plain_titles_untouched() {
        check_clean_title("Song Title", "Song Title");
        check_clean_title("", "");
    }

    #[test]
    fn identical_albums_match_regardless_of_case_or_punctuation() {
        check_album_match("Born to Run", "born to run", true);
        check_album_match("Born to Run", "Born, To Run!", true);
    }

    #[test]
    fn edition_and_remaster_annotations_are_ignored() {
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
    fn a_different_version_is_not_treated_as_the_same_release() {
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
    }

    #[test]
    fn small_typos_are_tolerated() {
        check_album_match("Highway 61 Revisited", "Highway 61 Revisted", true);
    }

    #[test]
    fn shorter_or_reordered_phrasing_still_matches() {
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
    fn distinguishing_numbers_must_match_exactly() {
        check_album_match(
            "Bruce Springsteen & The E Street Band Live 1975-85",
            "Bruce Springsteen & The E Street Band Live 1978-85",
            false,
        );
        check_album_match("Live at Wembley 1985", "Live at Wembley", false);
    }

    #[test]
    fn short_generic_album_names_do_not_match_longer_ones() {
        check_album_match("Live", "Wrecking Ball Live", false);
        check_album_match("Meet the Beatles", "The Beatles", false);
    }

    #[test]
    fn an_untagged_album_never_matches() {
        check_album_match("", "Born to Run", false);
        check_album_match("Born to Run", "", false);
        check_album_match("", "", false);
    }

    #[test]
    fn an_artist_album_prefix_is_ignored() {
        check_album_match("Hotel California", "Eagles - Hotel California", true);
        check_album_match("Bohemian Rhapsody", "Queen - Bohemian Rhapsody", true);
    }
}
