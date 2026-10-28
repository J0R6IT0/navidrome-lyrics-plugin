use std::borrow::Cow;

const KEEP_TAGS: &[&str] = &["offset"];

const CREDIT_ROLES: &[&str] = &[
    "adapt",
    "album",
    "arrang",
    "artist",
    "author",
    "beat",
    "choir",
    "chorus",
    "compil",
    "compos",
    "conduct",
    "copyright",
    "direct",
    "edit",
    "engineer",
    "harmon",
    "instrument",
    "lyric",
    "master",
    "melod",
    "mix",
    "music",
    "orchestrat",
    "perform",
    "produc",
    "program",
    "publish",
    "record",
    "repertoire",
    "sampl",
    "sing",
    "songwrit",
    "sound",
    "title",
    "translat",
    "vocal",
    "word",
    "writ",
];

const CREDIT_INSTRUMENTS: &[&str] = &[
    "accordion",
    "banjo",
    "bass",
    "bongo",
    "brass",
    "cello",
    "clarinet",
    "conga",
    "cymbal",
    "drum",
    "fiddle",
    "flute",
    "guitar",
    "harp",
    "horn",
    "kalimba",
    "key",
    "mandolin",
    "marimba",
    "oboe",
    "orchestra",
    "organ",
    "percussion",
    "piano",
    "sax",
    "shaker",
    "sitar",
    "string",
    "synth",
    "tambourine",
    "timpani",
    "trombone",
    "trumpet",
    "tuba",
    "turntable",
    "ukulele",
    "viol",
    "whistle",
    "woodwind",
    "xylophone",
];

const CREDIT_FILLERS: &[&str] = &[
    "acoustic",
    "additional",
    "addl",
    "all",
    "and",
    "assistant",
    "associate",
    "asst",
    "at",
    "background",
    "backing",
    "by",
    "co",
    "digital",
    "electric",
    "executive",
    "extra",
    "feat",
    "featuring",
    "for",
    "ft",
    "guest",
    "in",
    "junior",
    "lead",
    "live",
    "main",
    "of",
    "on",
    "or",
    "original",
    "other",
    "owner",
    "second",
    "senior",
    "session",
    "slide",
    "steel",
    "studio",
    "the",
    "uncredited",
    "with",
];

const CREDIT_LABELS_CJK: &[&str] = &[
    "作词", "作曲", "编曲", "制作", "监制", "录音", "混音", "母带", "出品", "翻译", "和声", "演唱",
    "吉他", "贝斯", "键盘", "钢琴", "弦乐", "鼓",
];

/// A leading "Artist - Title" header is only stripped when its timestamp is
/// within this many seconds of the start.
const TITLE_HEADER_MAX_SECS: f64 = 5.0;

const INSTRUMENTAL_MARKERS: &[&str] = &["instrumental", "纯音乐", "no lyrics"];

/// More than this many timed lines means the track has real lyrics, not just an
/// instrumental marker.
const MAX_INSTRUMENTAL_TIMED_LINES: usize = 3;

/// A blank (timestamp-only) line is kept only when the gap to the next line is
/// at least this long. Shorter gaps are provider noise, not a real instrumental
/// pause, and a long enough gap can also let a stripped section label leave one
/// blank line behind.
pub(crate) const BLANK_GAP_MIN_SECS: f64 = 5.0;

pub fn sanitize(lrc: &str) -> String {
    let mut first_time_tag_seen = false;

    let kept: Vec<&str> = lrc
        .lines()
        .filter(|line| keep_line(line, &mut first_time_tag_seen))
        .collect();

    let mut out: Vec<&str> = Vec::with_capacity(kept.len());
    for (i, &line) in kept.iter().enumerate() {
        if is_blank_timed_line(line) {
            let next = kept.get(i + 1).and_then(|l| time_tag_secs(l));
            let long_gap = match (time_tag_secs(line), next) {
                (Some(_), None) => true, // Trailing blank timestamp
                (Some(start), Some(end)) => end - start >= BLANK_GAP_MIN_SECS,
                _ => false,
            };

            if !long_gap {
                continue;
            }
        }
        out.push(line);
    }

    out.join("\n")
}

pub fn is_instrumental(lyrics: &str) -> bool {
    let timed_lines = lyrics
        .lines()
        .filter(|line| matches!(parse_line(line), Some((Some(_), _))))
        .count();

    if timed_lines > MAX_INSTRUMENTAL_TIMED_LINES {
        return false;
    }

    let lower = lyrics.to_lowercase();

    INSTRUMENTAL_MARKERS.iter().any(|m| lower.contains(m))
}

pub fn is_synced(lyrics: &str) -> bool {
    lyrics
        .lines()
        .any(|line| matches!(parse_line(line), Some((Some(_), _))))
}

pub(crate) fn time_tag_secs(line: &str) -> Option<f64> {
    match parse_line(line) {
        Some((Some(secs), _)) => Some(secs),
        _ => None,
    }
}

pub fn format_timestamp(ms: i64) -> String {
    let cs = (ms.max(0) + 5) / 10;
    let hundredths = cs % 100;
    let total_secs = cs / 100;
    let secs = total_secs % 60;
    let total_mins = total_secs / 60;

    if total_mins < 100 {
        return format!("{total_mins:02}:{secs:02}.{hundredths:02}");
    }
    let (hours, mins) = (total_mins / 60, total_mins % 60);
    format!("{hours:02}:{mins:02}:{secs:02}.{hundredths:02}")
}

pub(crate) fn is_blank_timed_line(line: &str) -> bool {
    matches!(
        parse_line(line),
        Some((Some(_), text)) if strip_word_tags(text).trim().is_empty()
    )
}

pub(crate) fn timestamp_only(line: &str) -> String {
    match line.find(']') {
        Some(end) => line[..=end].to_string(),
        None => line.to_string(),
    }
}

fn keep_line(line: &str, first_time_tag_seen: &mut bool) -> bool {
    let trimmed = line.trim_start_matches('\u{feff}').trim();

    let lyric_text = match parse_line(trimmed) {
        Some((Some(secs), text)) => {
            if !*first_time_tag_seen {
                *first_time_tag_seen = true;
                if secs < TITLE_HEADER_MAX_SECS && is_title_header(text) {
                    return false;
                }
            }
            text
        }
        Some((None, _)) => {
            if is_droppable_metadata(trimmed) {
                return false;
            }
            return true;
        }
        None => trimmed,
    };

    !is_credit_line(lyric_text)
}

fn is_title_header(text: &str) -> bool {
    let plain = strip_word_tags(text);
    (plain.contains(" - ") || plain.contains(" — ")) && plain.chars().any(char::is_alphabetic)
}

fn is_droppable_metadata(line: &str) -> bool {
    let content = line
        .strip_prefix('[')
        .and_then(|rest| rest.split(']').next())
        .unwrap_or_default();

    match content.split_once(':') {
        Some((key, _)) => !KEEP_TAGS.contains(&key),
        None => true,
    }
}

fn is_credit_line(text: &str) -> bool {
    let plain = strip_word_tags(text);

    let Some(label) = credit_label(plain.trim_start()) else {
        return false;
    };

    if CREDIT_LABELS_CJK.iter().any(|l| label.contains(l)) {
        return true;
    }

    let mut named_role = false;
    for word in label.split(|c: char| !c.is_alphanumeric()) {
        if word.chars().nth(1).is_none() {
            continue;
        }
        let word = word.to_ascii_lowercase();

        if starts_with_any(&word, CREDIT_ROLES) || starts_with_any(&word, CREDIT_INSTRUMENTS) {
            named_role = true;
        } else if !is_credit_filler(&word) {
            return false;
        }
    }

    named_role
}

fn credit_label(text: &str) -> Option<&str> {
    let end = text.find([':', '：'])?;
    Some(text[..end].trim_end())
}

fn starts_with_any(word: &str, stems: &[&str]) -> bool {
    stems.iter().any(|stem| word.starts_with(stem))
}

fn is_credit_filler(word: &str) -> bool {
    CREDIT_FILLERS.contains(&word) || word.chars().all(|c| c.is_ascii_digit())
}

pub(crate) fn strip_word_tags(text: &str) -> Cow<'_, str> {
    if !text.contains('<') {
        return Cow::Borrowed(text);
    }

    let mut out = String::with_capacity(text.len());
    let mut rest = text;

    while let Some(open) = rest.find('<') {
        let after = &rest[open + 1..];
        let is_tag = after.starts_with(|c: char| c.is_ascii_digit());

        if let Some(close) = is_tag.then(|| after.find('>')).flatten() {
            out.push_str(&rest[..open]);
            rest = &after[close + 1..];
        } else {
            out.push_str(&rest[..=open]);
            rest = &rest[open + 1..];
        }
    }

    out.push_str(rest);
    Cow::Owned(out)
}

fn parse_line(line: &str) -> Option<(Option<f64>, &str)> {
    let trimmed = line.trim_start_matches('\u{feff}').trim();

    let rest = trimmed.strip_prefix('[')?;
    let bracket_end = rest.find(']')?;

    let content = &rest[..bracket_end];
    let text = &rest[bracket_end + 1..];

    Some((parse_time_tag(content), text))
}

pub(crate) fn parse_time_tag(content: &str) -> Option<f64> {
    let mut fields = content.split(':');
    let (first, second, third) = (fields.next()?, fields.next()?, fields.next());
    if fields.next().is_some() {
        return None;
    }

    let (hours, mins, secs) = match third {
        Some(secs) => (first, second, secs),
        None => ("0", first, second),
    };

    let whole = |f: &str| !f.is_empty() && f.chars().all(|c| c.is_ascii_digit());
    if !whole(hours) || !whole(mins) {
        return None;
    }
    if !secs.contains('.') || !secs.chars().all(|c| c.is_ascii_digit() || c == '.') {
        return None;
    }

    let secs = secs.parse::<f64>().ok()?;
    Some((hours.parse::<f64>().ok()? * 60.0 + mins.parse::<f64>().ok()?) * 60.0 + secs)
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[track_caller]
    fn check_sanitize(input: &str, expected: &str) {
        assert_eq!(sanitize(&trim_indent(input)), trim_indent(expected));
    }

    #[track_caller]
    fn check_instrumental(input: &str, expected: bool) {
        assert_eq!(is_instrumental(&trim_indent(input)), expected);
    }

    #[track_caller]
    fn check_synced(input: &str, expected: bool) {
        assert_eq!(is_synced(&trim_indent(input)), expected);
    }

    #[track_caller]
    fn check_word_tags(input: &str, expected: &str) {
        assert_eq!(strip_word_tags(input), expected);
    }

    #[track_caller]
    fn check_timestamp(ms: i64, expected: &str) {
        assert_eq!(format_timestamp(ms), expected);
    }

    #[test]
    fn smoke() {
        check_sanitize("", "");
        check_sanitize(
            "
            [00:10.00] Hello world
            [00:15.00] Foo bar",
            "
            [00:10.00] Hello world
            [00:15.00] Foo bar",
        );
    }

    mod timestamps {
        use super::*;

        #[test]
        fn rounds_to_centiseconds() {
            check_timestamp(0, "00:00.00");
            check_timestamp(736, "00:00.74");
            check_timestamp(65_000, "01:05.00");
        }

        #[test]
        fn negative_offsets_clamp_to_zero() {
            check_timestamp(-5, "00:00.00");
        }

        #[test]
        fn roll_into_hours_past_99_minutes() {
            check_timestamp(99 * 60_000 + 59_990, "99:59.99");
            check_timestamp(100 * 60_000, "01:40:00.00");
            check_timestamp(3 * 3_600_000 + 25 * 60_000 + 7_120, "03:25:07.12");
        }

        #[test]
        fn hour_form_time_tags_are_understood() {
            let line = "[01:40:05.00]<01:40:05.00>past <01:40:06.00>the hour<01:40:07.00>";
            check_sanitize(line, line);
            check_synced(line, true);
        }
    }

    mod metadata {
        use super::*;

        #[test]
        fn tags_are_stripped() {
            check_sanitize(
                "
                [ar:Artist Name]
                [al:Album]
                [ti:Song Title]
                [00:10.00] Hello",
                "[00:10.00] Hello",
            );
        }

        #[test]
        fn offset_tag_is_kept() {
            check_sanitize(
                "
                [offset:-500]
                [ar:Artist]
                [00:10.00] Hello",
                "
                [offset:-500]
                [00:10.00] Hello",
            );
        }

        #[test]
        fn bracketed_label_without_colon_is_stripped() {
            check_sanitize(
                "
                [SomethingWithoutColon]
                [00:10.00] Hello",
                "[00:10.00] Hello",
            );
        }
    }

    mod title_header {
        use super::*;

        #[test]
        fn only_leading_artist_title_line_is_stripped() {
            check_sanitize(
                "
                [00:01.00] Artist - Title
                [00:05.00] First verse",
                "[00:05.00] First verse",
            );

            check_sanitize(
                "
                [00:01.00] First verse
                [00:03.00] Artist - Title",
                "
                [00:01.00] First verse
                [00:03.00] Artist - Title",
            );
        }

        #[test]
        fn line_without_a_dash_is_kept() {
            check_sanitize("[00:01.00] First verse", "[00:01.00] First verse");
        }
    }

    mod credits {
        use super::*;

        #[test]
        fn are_stripped_with_and_without_time_tags() {
            check_sanitize(
                "
                Lyrics by: Someone
                [00:05.00] Hello",
                "[00:05.00] Hello",
            );
            check_sanitize(
                "
                [00:01.00] Lyrics by: Someone
                [00:05.00] Hello",
                "[00:05.00] Hello",
            );
        }

        #[test]
        fn by_is_optional() {
            check_sanitize(
                "
                Producer: Someone
                [00:05.00] Hello",
                "[00:05.00] Hello",
            );
            check_sanitize(
                "
                [00:01.00] Mix: Someone
                [00:05.00] Hello",
                "[00:05.00] Hello",
            );
        }

        #[test]
        fn case_and_spacing_are_ignored() {
            check_sanitize(
                "
                LYRICS BY: Someone
                [00:05.00] Hello",
                "[00:05.00] Hello",
            );
            check_sanitize(
                "
                Written by : Someone
                [00:05.00] Hello",
                "[00:05.00] Hello",
            );
            check_sanitize(
                "
                Written by\u{FF1A} Someone
                [00:05.00] Hello",
                "[00:05.00] Hello",
            );
        }

        #[test]
        fn roles_are_matched_by_stem() {
            check_sanitize(
                "
                [00:01.00] Music by: Composer
                [00:02.00] Arranged by: Arranger
                [00:03.00] Mixing Engineer: Someone
                [00:04.00] Vocal Production: Someone
                [00:10.00] Verse",
                "[00:10.00] Verse",
            );
        }

        #[test]
        fn instruments_are_recognised() {
            check_sanitize(
                "
                [00:01.00] Guitar, Drums, Bass, Keys, Piano and Programming by: Someone
                [00:02.00] ACOUSTIC GUITAR: Someone
                [00:03.00] 12 String Guitar by : Someone
                [00:04.00] Additional Percussion & Synths: Someone
                [00:10.00] Hello",
                "[00:10.00] Hello",
            );
        }

        #[test]
        fn chinese_labels_need_no_spaces() {
            check_sanitize(
                "
                [00:00.000] 作词 : Freddie Mercury
                [00:01.000]作曲:Freddie Mercury
                [00:02.000]编曲 : Queen
                [00:06.600]He's a fairy feller",
                "[00:06.600]He's a fairy feller",
            );
        }

        #[test]
        fn netease_preamble_is_stripped() {
            check_sanitize(
                "
                [00:00.000] 作词 : Freddie Mercury
                [00:00.000] 作曲 : Freddie Mercury
                [00:01.000]作曲 : Freddie Mercury
                [00:02.000]编曲 : Queen
                [00:06.600]He's a fairy feller
                [00:20.860]Ah ah the fairy folk have gathered
                [00:22.590]Round the new moon's shine",
                "
                [00:06.600]He's a fairy feller
                [00:20.860]Ah ah the fairy folk have gathered
                [00:22.590]Round the new moon's shine",
            );
        }

        #[test]
        fn lyrics_containing_a_colon_are_kept() {
            let lyrics = "
                [00:05.00] And then he said: run
                [00:10.00] Baby: I love you
                [00:15.00] And: so it goes";
            check_sanitize(lyrics, lyrics);
        }

        #[test]
        fn unknown_word_in_the_label_keeps_the_line() {
            let lyrics = "[00:05.00] Producer of nightmares: me";
            check_sanitize(lyrics, lyrics);
        }

        #[test]
        fn whole_header_block_is_stripped() {
            check_sanitize(
                "
                [ar:Artist]
                [al:Album]
                [offset:500]
                [00:00.50] Artist - Title
                [00:10.00] Lyrics by: Someone
                [00:15.00] Hello
                [00:20.00] World",
                "
                [offset:500]
                [00:15.00] Hello
                [00:20.00] World",
            );
        }
    }

    mod blank_lines {
        use super::*;

        #[test]
        fn short_gap_is_dropped() {
            check_sanitize(
                "
                [00:44.95]A song in every breath
                [00:47.60]
                [00:48.36]Sing me",
                "
                [00:44.95]A song in every breath
                [00:48.36]Sing me",
            );
        }

        #[test]
        fn long_gap_is_kept() {
            check_sanitize(
                "
                [00:44.95]A song
                [00:47.60]
                [00:55.00]Sing me",
                "
                [00:44.95]A song
                [00:47.60]
                [00:55.00]Sing me",
            );
        }

        #[test]
        fn trailing_blank_line_is_kept() {
            check_sanitize(
                "
                [00:44.95]Last line
                [00:47.60]",
                "
                [00:44.95]Last line
                [00:47.60]",
            );
        }
    }

    mod word_timings {
        use super::*;

        #[test]
        fn survive_sanitizing() {
            let line = "[00:06.00]<00:06.00>Hello <00:06.50>world";
            check_sanitize(line, line);
        }

        #[test]
        fn do_not_hide_a_title_header() {
            check_sanitize(
                "
                [00:00.00]<00:00.00>Artist <00:00.50>- <00:01.00>Title
                [00:06.00]<00:06.00>First <00:06.50>line",
                "[00:06.00]<00:06.00>First <00:06.50>line",
            );
        }

        #[test]
        fn do_not_hide_a_credit() {
            check_sanitize(
                "
                [00:01.00]<00:01.00>Composed <00:01.50>by<00:02.00>: <00:02.50>Someone
                [00:06.00]<00:06.00>Hello",
                "[00:06.00]<00:06.00>Hello",
            );
        }

        #[test]
        fn are_stripped_from_the_text_being_inspected() {
            check_word_tags("<00:00.00>Hello <00:00.50>world", "Hello world");
        }

        #[test]
        fn angle_brackets_that_are_not_tags_are_left_alone() {
            check_word_tags("a <b> c", "a <b> c");
            check_word_tags("I <3 it", "I <3 it");
            check_word_tags("done <12:34", "done <12:34");
        }
    }

    mod instrumental {
        use super::*;

        #[test]
        fn markers_are_detected() {
            check_instrumental("Instrumental", true);
            check_instrumental("[00:00.00]No Lyrics", true);
            check_instrumental("[00:01.00]纯音乐，请欣赏", true);
            check_instrumental("\u{feff}[00:01.00]纯音乐", true);
        }

        #[test]
        fn lyrics_are_not_a_marker() {
            check_instrumental(
                "
                [00:01.00]Hello
                [00:05.00]World",
                false,
            );
        }

        #[test]
        fn more_than_three_timed_lines_outweigh_a_marker() {
            check_instrumental(
                "
                [00:01.00]纯音乐，请欣赏
                [00:02.00]...
                [00:03.00]...
                [00:04.00]...",
                false,
            );
        }

        #[test]
        fn metadata_and_untimed_lines_are_not_counted() {
            check_instrumental(
                "
                [ar:test]
                [ti:test]
                [offset:0]
                [00:01.00]纯音乐，请欣赏",
                true,
            );
            check_instrumental(
                "
                hello
                world
                [00:01.00]instrumental",
                true,
            );
        }
    }

    mod sync {
        use super::*;

        #[test]
        fn time_tagged_lines_are_synced() {
            check_synced(
                "
                [00:01.00]Hello
                [00:05.00]World",
                true,
            );
            check_synced(
                "
                [ar:Artist]
                [al:Album]
                [ti:Title]
                [00:01.00]Hello",
                true,
            );
            check_synced("\u{feff}[00:01.00]Hello", true);
        }

        #[test]
        fn plain_text_is_not_synced() {
            check_synced("", false);
            check_synced(
                "
                Hello
                World",
                false,
            );
            check_synced(
                "
                [Verse 1]
                Hello
                World",
                false,
            );
        }
    }
}
