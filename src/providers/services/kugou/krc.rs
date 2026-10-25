use crate::format::{elrc, lrc};
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

pub struct Lyrics {
    pub elrc: String,
    pub lrc: String,
    pub translation: Option<String>,
}

struct Line {
    start_ms: i64,
    words: Vec<elrc::Word>,
}

#[derive(Deserialize)]
struct Language {
    #[serde(default)]
    content: Vec<LanguageContent>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LanguageContent {
    #[serde(rename = "type", default)]
    kind: i64,
    #[serde(default)]
    lyric_content: Vec<Vec<String>>,
}

pub fn parse(decoded: &[u8]) -> Result<Lyrics, String> {
    let krc = decrypt(decoded)?;
    Ok(parse_text(&krc))
}

fn decrypt(decoded: &[u8]) -> Result<String, String> {
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

fn parse_text(krc: &str) -> Lyrics {
    let line_re = Regex::new(r"^\[(\d+),\d+\](.*)$").unwrap();
    let word_re = Regex::new(r"<(\d+),(\d+),-?\d+>([^<]*)").unwrap();

    let mut lines = Vec::new();
    let mut translation = None;

    for raw in krc.lines() {
        let raw = raw.trim();

        if let Some(encoded) = raw
            .strip_prefix("[language:")
            .and_then(|line| line.strip_suffix(']'))
        {
            translation = parse_translation(encoded);
            continue;
        }

        let Some(caps) = line_re.captures(raw) else {
            continue;
        };

        let start: i64 = caps[1].parse().unwrap_or(0);
        let words: Vec<elrc::Word> = word_re
            .captures_iter(&caps[2])
            .map(|word| {
                let offset: i64 = word[1].parse().unwrap_or(0);
                let duration: i64 = word[2].parse().unwrap_or(0);
                elrc::Word {
                    text: word[3].to_string(),
                    start_ms: start + offset,
                    end_ms: start + offset + duration,
                }
            })
            .collect();

        lines.push(Line {
            start_ms: start,
            words,
        });
    }

    let elrc = lines
        .iter()
        .filter_map(|line| elrc::render_line(line.start_ms, &line.words))
        .collect::<Vec<_>>()
        .join("\n");

    let lrc = lines
        .iter()
        .map(|line| {
            let text: String = line.words.iter().map(|word| word.text.as_str()).collect();
            format!("[{}]{text}", lrc::format_timestamp(line.start_ms))
        })
        .collect::<Vec<_>>()
        .join("\n");

    let translation = translation.and_then(|content| {
        let translated: Vec<String> = lines
            .iter()
            .zip(content)
            .filter_map(|(line, text)| {
                let text = text.trim();
                (!text.is_empty())
                    .then(|| format!("[{}]{text}", lrc::format_timestamp(line.start_ms)))
            })
            .collect();

        (!translated.is_empty()).then(|| translated.join("\n"))
    });

    Lyrics {
        elrc,
        lrc,
        translation,
    }
}

fn parse_translation(encoded: &str) -> Option<Vec<String>> {
    let json = STANDARD.decode(encoded).ok()?;
    let language: Language = serde_json::from_slice(&json).ok()?;
    let content = language.content.into_iter().find(|item| item.kind == 1)?;

    Some(
        content
            .lyric_content
            .into_iter()
            .map(|parts| parts.concat())
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[track_caller]
    fn check(krc: &str, expected: &str) {
        assert_eq!(parse_text(krc).elrc, expected);
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
        let parsed = parse_text("");
        assert_eq!(parsed.elrc, "");
        assert_eq!(parsed.lrc, "");
        assert_eq!(parsed.translation, None);
    }

    #[test]
    fn translations_are_aligned_by_line() {
        let language = STANDARD.encode(
            r#"{"content":[{"language":9,"type":0,"lyricContent":[["romaji"]]},{"language":0,"type":1,"lyricContent":[["你好"],[" "],["世界"]]}]}"#,
        );
        let krc = format!(
            "[0,1000]<0,500,0>Hello<500,500,0>\n[1000,1000]<0,1000,0>World\n[2000,1000]<0,1000,0>Again\n[language:{language}]"
        );

        let parsed = parse_text(&krc);

        assert_eq!(
            parsed.lrc,
            "[00:00.00]Hello\n[00:01.00]World\n[00:02.00]Again"
        );
        assert_eq!(
            parsed.translation.as_deref(),
            Some("[00:00.00]你好\n[00:02.00]世界")
        );
    }

    #[test]
    fn test_decrypt_rejects_non_krc() {
        assert!(parse(b"not a krc payload").is_err());
    }
}
