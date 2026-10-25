use std::collections::HashMap;

use crate::format::lrc;

pub fn merge(original: &str, translation: &str) -> String {
    let translations = translations_by_time(translation);
    if translations.is_empty() {
        return original.to_string();
    }

    let mut out: Vec<String> = Vec::new();
    for line in original.lines() {
        out.push(line.to_string());

        let Some(start_ms) = time_ms(line) else {
            continue;
        };
        let Some(text) = translations.get(&start_ms) else {
            continue;
        };
        if *text == line_text(line) {
            continue;
        }

        out.push(format!("{}{text}", lrc::timestamp_only(line)));
    }

    out.join("\n")
}

fn translations_by_time(translation: &str) -> HashMap<i64, String> {
    let mut out = HashMap::new();

    for line in translation.lines() {
        let line = line.trim_start_matches('\u{feff}').trim();
        let (times, text) = split_leading_timestamps(line);
        if times.is_empty() {
            continue;
        }

        let text = lrc::strip_word_tags(text).trim().to_string();
        if text.is_empty() {
            continue;
        }

        for ms in times {
            out.insert(ms, text.clone());
        }
    }

    out
}

fn split_leading_timestamps(mut line: &str) -> (Vec<i64>, &str) {
    let mut times = Vec::new();

    while let Some(rest) = line.strip_prefix('[') {
        let Some(end) = rest.find(']') else {
            break;
        };
        let Some(ms) = time_ms(&line[..end + 2]) else {
            break;
        };

        times.push(ms);
        line = rest[end + 1..].trim_start();
    }

    (times, line)
}

fn time_ms(line: &str) -> Option<i64> {
    lrc::time_tag_secs(line).map(|secs| (secs * 1000.0).round() as i64)
}

fn line_text(line: &str) -> String {
    let text = line
        .trim_start_matches('\u{feff}')
        .trim()
        .split_once(']')
        .map(|(_, text)| text)
        .unwrap_or_default();

    lrc::strip_word_tags(text).trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[track_caller]
    fn check(original: &str, translation: &str, expected: &str) {
        assert_eq!(merge(original, translation), expected);
    }

    #[test]
    fn translations_are_matched_by_timestamp() {
        check(
            "[00:02.00]two\n[00:01.00]one",
            "[00:01.00]一\n[00:02.00]二",
            "[00:02.00]two\n[00:02.00]二\n[00:01.00]one\n[00:01.00]一",
        );
    }

    #[test]
    fn the_original_timestamp_format_is_reused() {
        check(
            "[00:06.19]Hello",
            "[00:06.190]你好",
            "[00:06.19]Hello\n[00:06.19]你好",
        );
    }

    #[test]
    fn untimed_translation_lines_are_ignored() {
        check(
            "[00:01.00]one",
            "[by:someone]\n[00:01.00]一\nplain text",
            "[00:01.00]one\n[00:01.00]一",
        );
    }

    #[test]
    fn lines_without_a_translation_are_kept_as_is() {
        check(
            "[00:01.00]one\n[00:02.00]two",
            "[00:02.00]二",
            "[00:01.00]one\n[00:02.00]two\n[00:02.00]二",
        );
    }

    #[test]
    fn a_translation_identical_to_the_original_is_skipped() {
        check("[00:01.00]Hello", "[00:01.00]Hello", "[00:01.00]Hello");
        check(
            "[00:01.00]<00:01.00>Hello<00:02.00>",
            "[00:01.00]Hello",
            "[00:01.00]<00:01.00>Hello<00:02.00>",
        );
    }

    #[test]
    fn an_enhanced_line_gets_its_translation() {
        check(
            "[00:01.00]<00:01.00>Hel<00:01.50>lo<00:02.00>",
            "[00:01.00]你好",
            "[00:01.00]<00:01.00>Hel<00:01.50>lo<00:02.00>\n[00:01.00]你好",
        );
    }

    #[test]
    fn multiple_timestamps_share_one_translation() {
        check(
            "[00:01.00]one\n[00:02.00]two",
            "[00:01.00][00:02.00]一",
            "[00:01.00]one\n[00:01.00]一\n[00:02.00]two\n[00:02.00]一",
        );
    }

    #[test]
    fn empty_translations_leave_the_original_untouched() {
        check("[00:01.00]one", "", "[00:01.00]one");
        check("[00:01.00]one", "[00:01.00]  ", "[00:01.00]one");
    }
}
