use regex::Regex;

pub mod bilingual;
pub mod censor;
pub mod elrc;
pub mod lrc;
pub mod lyricsfile;
pub mod ttml;

pub fn strip_section_labels(lyrics: &str) -> String {
    let re = Regex::new(
        r"(?i)^(?:verse|pre[- ]?verse|chorus|pre[- ]?chorus|post[- ]?chorus|bridge|hook|refrain|intro|outro|coda|interlude|instrumental|breakdown|solo|drop|chant|skit|ad-?lib|overture|finale|couplet|coro|pre[- ]?coro|post[- ]?coro|verso|puente|refrán|interludio)\b"
    )
    .unwrap();

    let mut out: Vec<String> = Vec::new();

    let mut pending: Vec<String> = Vec::new();
    let mut run_has_label = false;

    for line in lyrics.lines() {
        let stripped = strip_labels_in_line(line, &re);
        let cleaned = stripped.replace("  ", " ");

        let cleaned = if line.trim_start().starts_with('[') {
            cleaned.trim().to_string()
        } else {
            cleaned.trim_end().to_string()
        };

        if cleaned.trim().is_empty() {
            continue;
        }

        if lrc::is_blank_timed_line(&cleaned) {
            run_has_label |= !lrc::is_blank_timed_line(line);
            pending.push(lrc::timestamp_only(&cleaned));
            continue;
        }

        if run_has_label {
            if gap_warrants_blank(&pending, &cleaned) {
                out.push(pending.remove(0));
            }
            pending.clear();
        } else {
            out.append(&mut pending);
        }
        run_has_label = false;

        out.push(cleaned);
    }

    if !run_has_label {
        out.append(&mut pending);
    }

    out.join("\n")
}

fn strip_labels_in_line(line: &str, re: &Regex) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;

    while let Some(open) = rest.find('[') {
        let Some(close) = rest[open..].find(']').map(|i| open + i) else {
            break;
        };

        let content = lrc::strip_word_tags(&rest[open + 1..close]);

        if re.is_match(content.trim()) {
            out.push_str(&rest[..open]);
        } else {
            out.push_str(&rest[..=close]);
        }

        rest = &rest[close + 1..];
    }

    out.push_str(rest);
    out
}

fn gap_warrants_blank(pending: &[String], next: &str) -> bool {
    let (Some(start), Some(end)) = (
        pending.first().and_then(|l| lrc::time_tag_secs(l)),
        lrc::time_tag_secs(next),
    ) else {
        return false;
    };

    end - start >= lrc::BLANK_GAP_MIN_SECS
}

#[cfg(test)]
mod tests {
    use super::strip_section_labels;

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
    fn check(input: &str, expected: &str) {
        assert_eq!(
            strip_section_labels(&trim_indent(input)),
            trim_indent(expected)
        );
    }

    #[track_caller]
    fn check_label(label: &str) {
        check(&format!("[{label}]\nHello world"), "Hello world");
    }

    #[test]
    fn label_lines_are_dropped() {
        check(
            "
            [Verse 1]
            Hello there
            [Chorus]
            We will rock you",
            "
            Hello there
            We will rock you",
        );

        check(
            "
            [Verse 2]
            [Pre-Chorus: Lead]
            [Hook - Artist]
            [Chorus 3x]
            [Outro (Fade)]",
            "",
        );

        check(
            "
            [00:10.00][Verse 1]First line
            [00:15.00][Chorus]Second line",
            "
            [00:10.00]First line
            [00:15.00]Second line",
        );

        check(
            "
            [00:00.00][Verse]
            [00:02.00]Hi",
            "[00:02.00]Hi",
        );
    }

    #[test]
    fn labels_are_detected() {
        for label in [
            "Verse 1",
            "VERSE 1",
            "chorus",
            "Pre-Chorus",
            "Pre Chorus",
            "Prechorus",
            "Post-Chorus",
            "Ad-lib",
            "Adlib",
            "Hook - Artist",
            "Chorus 3x",
            "Outro (Fade)",
            "Coro",
            "Verso",
            "Puente",
            "Interludio",
            "Refrán",
        ] {
            check_label(label);
        }
    }

    #[test]
    fn normal_words_that_start_like_labels_are_kept() {
        check(
            "
            [00:10.00]Breaking the [Chains]
            [Outrageous] behavior",
            "
            [00:10.00]Breaking the [Chains]
            [Outrageous] behavior",
        );
        check(
            "
            [00:00.00]Outro line
            [00:02.00][Outro]
            [00:04.00]End",
            "
            [00:00.00]Outro line
            [00:04.00]End",
        );
    }

    #[test]
    fn a_label_is_removed_from_a_line() {
        check("[Chorus] We will [Chorus] rock you", "We will rock you");
    }

    #[test]
    fn labels_inside_timestamps_are_dropped() {
        check(
            "
            [00:00.06]<00:00.06>[<00:00.13>Intro<00:00.20>]<00:00.27>
            [00:00.27]<00:00.27>Getting <00:00.72>late",
            "[00:00.27]<00:00.27>Getting <00:00.72>late",
        );
    }

    #[test]
    fn normal_words_are_not_modified() {
        let line = "[00:10.00]<00:10.00>Hello <00:10.50>world";
        check(line, line);
    }

    #[test]
    fn blank_lines_next_to_labels_are_removed() {
        check(
            "
            [00:00.00]Intro
            [00:02.00][Verse]
            [00:04.00]
            [00:06.00]First",
            "
            [00:00.00]Intro
            [00:06.00]First",
        );
        check(
            "
            [00:00.00]A
            [00:02.00]
            [00:04.00][Verse]
            [00:06.00]B",
            "
            [00:00.00]A
            [00:06.00]B",
        );
        check(
            "
            [00:00.00]A
            [00:02.00]
            [00:04.00][Verse]
            [00:06.00]
            [00:08.00]B",
            "
            [00:00.00]A
            [00:02.00]
            [00:08.00]B",
        );
    }

    #[test]
    fn a_long_gap_keeps_one_blank_line() {
        check(
            "
            [00:01.00]Foo
            [00:03.00]
            [00:03.00][Chorus]
            [00:10.00]Bar",
            "
            [00:01.00]Foo
            [00:03.00]
            [00:10.00]Bar",
        );
        check(
            "
            [00:01.00]Foo
            [00:03.00]
            [00:03.00][Chorus]
            [00:04.00]Bar",
            "
            [00:01.00]Foo
            [00:04.00]Bar",
        );
    }

    #[test]
    fn a_regular_blank_line_is_kept() {
        let lines = "
            [00:00.00]A
            [00:02.00]
            [00:04.00]B";
        check(lines, lines);
    }
}
