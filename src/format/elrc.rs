use crate::format::lrc::format_timestamp;
use crate::types::Word;

const AUDIBLE_PAUSE_MS: i64 = 150;
const IMPLAUSIBLE_WORD_MS: i64 = 40;

fn holds_pause(word_ms: i64, gap_ms: i64) -> bool {
    gap_ms >= AUDIBLE_PAUSE_MS && word_ms >= IMPLAUSIBLE_WORD_MS
}

pub fn render_line(line_start_ms: i64, words: &[Word]) -> Option<String> {
    let tokens = split_pauses(&fold_separators(words));
    let end = tokens.last()?.end_ms;

    let mut buf = format!("[{}]", format_timestamp(line_start_ms));
    for token in &tokens {
        buf.push_str(&format!(
            "<{}>{}",
            format_timestamp(token.start_ms),
            token.text
        ));
    }
    buf.push_str(&format!("<{}>", format_timestamp(end)));
    Some(buf)
}

/// Folds whitespace-only tokens into the word before them unless they are
/// holding a real pause.
fn fold_separators(words: &[Word]) -> Vec<Word> {
    let mut out: Vec<Word> = Vec::with_capacity(words.len());
    for word in words {
        let fold = word.is_separator()
            && out
                .last()
                .is_some_and(|prev| !holds_pause(prev.duration(), word.duration()));

        if fold && let Some(prev) = out.last_mut() {
            prev.text.push_str(&word.text);
            prev.end_ms = word.end_ms;
            continue;
        }
        out.push(Word::new(word.text.clone(), word.start_ms, word.end_ms));
    }
    out
}

/// Splits a word's trailing whitespace onto the pause that follows it.
fn split_pauses(words: &[Word]) -> Vec<Word> {
    let mut out: Vec<Word> = Vec::with_capacity(words.len());
    for (i, word) in words.iter().enumerate() {
        let gap = words.get(i + 1).map(|next| next.start_ms - word.end_ms);
        let spoken = word.text.trim_end();

        if let Some(next_start) = words.get(i + 1).map(|next| next.start_ms)
            && holds_pause(word.duration(), gap.unwrap_or(0))
            && !spoken.is_empty()
            && spoken.len() < word.text.len()
        {
            out.push(Word::new(spoken, word.start_ms, word.end_ms));
            out.push(Word::new(
                &word.text[spoken.len()..],
                word.end_ms,
                next_start,
            ));
            continue;
        }
        out.push(Word::new(word.text.clone(), word.start_ms, word.end_ms));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[track_caller]
    fn check(words: &[(&str, i64, i64)], expected: &str) {
        let line_start_ms = words.first().map_or(0, |&(_, start_ms, _)| start_ms);
        let words: Vec<Word> = words
            .iter()
            .map(|&(text, start_ms, end_ms)| Word::new(text, start_ms, end_ms))
            .collect();

        let rendered = render_line(line_start_ms, &words);
        assert_eq!(rendered.as_deref(), Some(expected));
    }

    #[test]
    fn every_word_gets_a_tag_and_the_line_gets_a_closing_one() {
        check(
            &[("hello ", 1000, 2000), ("world", 2000, 3000)],
            "[00:01.00]<00:01.00>hello <00:02.00>world<00:03.00>",
        );
    }

    #[test]
    fn a_line_without_words_renders_nothing() {
        assert!(render_line(1000, &[]).is_none());
    }

    #[test]
    fn an_audible_pause_moves_onto_the_trailing_space() {
        check(
            &[("hello ", 129_364, 129_828), ("world", 134_956, 135_316)],
            "[02:09.36]<02:09.36>hello<02:09.83> <02:14.96>world<02:15.32>",
        );
    }

    #[test]
    fn a_short_gap_stays_with_the_word() {
        check(
            &[("hello ", 1000, 1400), ("world", 1500, 1800)],
            "[00:01.00]<00:01.00>hello <00:01.50>world<00:01.80>",
        );
    }

    #[test]
    fn a_gap_after_an_implausibly_short_word_is_merged_to_the_word() {
        check(
            &[("hello ", 1000, 1020), ("world", 1800, 2000)],
            "[00:01.00]<00:01.00>hello <00:01.80>world<00:02.00>",
        );
    }

    #[test]
    fn a_pause_needs_a_space_between_words() {
        check(
            &[("hello-", 1000, 1400), ("world", 2000, 2400)],
            "[00:01.00]<00:01.00>hello-<00:02.00>world<00:02.40>",
        );
    }

    #[test]
    fn separators_are_merged_into_implausibly_short_words() {
        check(
            &[
                ("Hello", 42_350, 42_362),
                (" ", 42_362, 42_726),
                ("fucking", 42_726, 42_746),
                (" ", 42_746, 42_969),
                ("world!", 42_969, 42_983),
            ],
            "[00:42.35]<00:42.35>Hello <00:42.73>fucking <00:42.97>world!<00:42.98>",
        );
    }

    #[test]
    fn well_timed_separators_keep_their_own_segment() {
        check(
            &[
                ("hello", 20_490, 20_870),
                (" ", 20_870, 21_020),
                ("world", 21_020, 21_845),
            ],
            "[00:20.49]<00:20.49>hello<00:20.87> <00:21.02>world<00:21.85>",
        );
    }

    #[test]
    fn a_leading_separator_does_not_fold() {
        check(
            &[(" ", 1000, 1100), ("hi", 1100, 2000)],
            "[00:01.00]<00:01.00> <00:01.10>hi<00:02.00>",
        );
    }
}
