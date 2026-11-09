use crate::format::{elrc, lrc};
use crate::types::Word;
use base64::{Engine, engine::general_purpose::STANDARD};
use flate2::read::ZlibDecoder;
use regex::Regex;
use serde::Deserialize;
use std::io::Read;

/// XOR key used to obfuscate KuGou KRC payloads.
const KRC_KEY: [u8; 16] = [
    0x40, 0x47, 0x61, 0x77, 0x5e, 0x32, 0x74, 0x47, 0x51, 0x36, 0x31, 0x2d, 0xce, 0xd2, 0x6e, 0x69,
];

const MAGIC: &[u8] = b"krc1";
const TRANSLATION_TYPE: i64 = 1;

#[derive(Debug, Deserialize)]
struct Languages {
    #[serde(default)]
    content: Vec<LanguageContent>,
}

#[derive(Debug, Deserialize)]
struct LanguageContent {
    #[serde(rename = "type")]
    kind: i64,
    /// One entry per timed line split in pieces.
    #[serde(default, rename = "lyricContent")]
    lines: Vec<Vec<String>>,
}

pub fn decrypt(decoded: &[u8]) -> Result<String, String> {
    if decoded.len() <= MAGIC.len() || &decoded[..MAGIC.len()] != MAGIC {
        return Err("missing krc1 magic header".to_string());
    }

    let body: Vec<u8> = decoded[MAGIC.len()..]
        .iter()
        .enumerate()
        .map(|(i, b)| b ^ KRC_KEY[i % KRC_KEY.len()])
        .collect();

    let mut decoder = ZlibDecoder::new(&body[..]);
    let mut bytes = Vec::new();
    decoder
        .read_to_end(&mut bytes)
        .map_err(|e| format!("krc inflate failed: {e}"))?;

    String::from_utf8(bytes).map_err(|e| format!("krc is not valid UTF-8: {e}"))
}

/// The start and the content of every timed line.
fn timed_lines(krc: &str) -> Vec<(i64, &str)> {
    let line_re = Regex::new(r"^\[(\d+),\d+\](.*)$").unwrap();

    krc.lines()
        .filter_map(|line| {
            let caps = line_re.captures(line.trim())?;
            Some((caps[1].parse().unwrap_or(0), caps.get(2)?.as_str()))
        })
        .collect()
}

pub fn to_enhanced_lrc(krc: &str) -> String {
    let word_re = Regex::new(r"<(\d+),(\d+),-?\d+>([^<]*)").unwrap();

    let mut out = Vec::new();

    for (start, content) in timed_lines(krc) {
        let words: Vec<Word> = word_re
            .captures_iter(content)
            .map(|word| {
                let offset: i64 = word[1].parse().unwrap_or(0);
                let duration: i64 = word[2].parse().unwrap_or(0);
                Word {
                    text: word[3].to_string(),
                    start_ms: start + offset,
                    end_ms: start + offset + duration,
                }
            })
            .collect();

        if let Some(rendered) = elrc::render_line(start, &words) {
            out.push(rendered);
        }
    }

    out.join("\n")
}

pub fn translation(krc: &str) -> Result<Option<String>, String> {
    let tag_re = Regex::new(r"(?m)^\s*\[language:([^\]]*)\]").unwrap();
    let Some(encoded) = tag_re
        .captures(krc)
        .map(|caps| caps[1].trim().to_string())
        .filter(|encoded| !encoded.is_empty())
    else {
        return Ok(None);
    };

    let json = STANDARD
        .decode(&encoded)
        .map_err(|e| format!("language tag is not valid base64: {e}"))?;
    let languages: Languages = serde_json::from_slice(&json)
        .map_err(|e| format!("language tag is not valid JSON: {e}"))?;

    let Some(translation) = languages
        .content
        .into_iter()
        .find(|content| content.kind == TRANSLATION_TYPE)
    else {
        return Ok(None);
    };

    let lines = timed_lines(krc);
    if translation.lines.len() != lines.len() {
        return Err(format!(
            "the translation has {} lines but the lyrics have {}",
            translation.lines.len(),
            lines.len()
        ));
    }

    let out: Vec<String> = lines
        .iter()
        .zip(translation.lines)
        .filter_map(|(&(start, _), pieces)| {
            let text = pieces.concat();
            let text = text.trim();
            (!text.is_empty()).then(|| format!("[{}]{text}", lrc::format_timestamp(start)))
        })
        .collect();

    Ok((!out.is_empty()).then(|| out.join("\n")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[track_caller]
    fn check(krc: &str, expected: &str) {
        assert_eq!(to_enhanced_lrc(krc), expected);
    }

    #[test]
    fn krc_line_to_elrc() {
        check(
            "[0,5890]<0,736,0>foo<736,736,0>bar<1472,736,0>baz",
            "[00:00.00]<00:00.00>foo<00:00.74>bar<00:01.47>baz<00:02.21>",
        );
    }

    #[test]
    fn multiple_lines_to_elrc() {
        check(
            "[5890,5900]<0,1180,0>foo<1180,1180,0>：\n[11790,5900]<0,1180,0>bar",
            "[00:05.89]<00:05.89>foo<00:07.07>：<00:08.25>\n[00:11.79]<00:11.79>bar<00:12.97>",
        );
    }

    #[test]
    fn last_word_is_closed() {
        check("[0,1000]<0,500,0>hi", "[00:00.00]<00:00.00>hi<00:00.50>");
    }

    #[test]
    fn converts_mid_line_pause_on_space() {
        check(
            "[129364,6000]<0,464,0>foo <5592,360,0>bar",
            "[02:09.36]<02:09.36>foo<02:09.83> <02:14.96>bar<02:15.32>",
        );
    }

    #[test]
    fn empty_check() {
        assert_eq!(to_enhanced_lrc(""), "");
    }

    #[test]
    fn test_decrypt_rejects_non_krc() {
        assert!(decrypt(b"not a krc payload").is_err());
    }

    #[track_caller]
    fn check_translation(krc: &str, expected: &str) {
        let krc: Vec<String> = krc
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(|line| match line.strip_prefix("[language:{") {
                Some(json) => {
                    let json = format!("{{{}", json.strip_suffix(']').unwrap());
                    format!("[language:{}]", STANDARD.encode(json))
                }
                None => line.to_string(),
            })
            .collect();

        let actual = match translation(&krc.join("\n")) {
            Ok(translation) => translation.unwrap_or_default(),
            Err(e) => format!("error: {e}"),
        };
        assert_eq!(actual, expected);
    }

    #[test]
    fn translations_line_up_with_the_lyrics_in_order() {
        check_translation(
            r#"
            [ar:foo]
            [language:{"content":[{"type":0,"lyricContent":[["foo"],["baz"],["qux"]]},{"type":1,"lyricContent":[[" "],["Ba","z"],["Qux"]]}],"version":1}]
            [64,640]<0,485,0>foo
            [1671,1767]<0,469,0>baz
            [3438,2987]<0,202,0>qux
            "#,
            "[00:01.67]Baz\n[00:03.44]Qux",
        );
    }

    #[test]
    fn romanizations_are_ignored() {
        check_translation(
            r#"
            [language:{"content":[{"type":0,"lyricContent":[["foo"],["bar"]]}]}]
            [0,1000]<0,500,0>foo
            [1000,1000]<0,500,0>bar
            "#,
            "",
        );
    }

    #[test]
    fn blank_translations_are_ignored() {
        check_translation(
            r#"
            [language:{"content":[{"type":1,"lyricContent":[[" "],[""],[]]}]}]
            [0,1000]<0,500,0>foo
            [1000,1000]<0,500,0>bar
            [2000,1000]<0,500,0>baz
            "#,
            "",
        );
    }

    #[test]
    fn lyrics_without_a_language_tag_are_untranslated() {
        check_translation("[0,1000]<0,500,0>foo", "");
        check_translation("[language:]\n[0,1000]<0,500,0>foo", "");
    }

    #[test]
    fn translations_with_a_different_line_count_fail() {
        check_translation(
            r#"
            [language:{"content":[{"type":1,"lyricContent":[["Foo"]]}]}]
            [0,1000]<0,500,0>foo
            [1000,1000]<0,500,0>bar
            "#,
            "error: the translation has 1 lines but the lyrics have 2",
        );
    }

    #[test]
    fn broken_language_tags_fail() {
        check_translation(
            "[language:%%%]\n[0,1000]<0,500,0>foo",
            "error: language tag is not valid base64: Invalid symbol 37, offset 0.",
        );
        // "not json" in base64.
        check_translation(
            "[language:bm90IGpzb24=]\n[0,1000]<0,500,0>foo",
            "error: language tag is not valid JSON: expected ident at line 1 column 2",
        );
    }
}
