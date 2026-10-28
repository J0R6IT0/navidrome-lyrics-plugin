use crate::format::elrc;
use crate::types::Word;
use flate2::read::ZlibDecoder;
use regex::Regex;
use std::io::Read;

/// XOR key used to obfuscate KuGou KRC payloads.
const KRC_KEY: [u8; 16] = [
    0x40, 0x47, 0x61, 0x77, 0x5e, 0x32, 0x74, 0x47, 0x51, 0x36, 0x31, 0x2d, 0xce, 0xd2, 0x6e, 0x69,
];

const MAGIC: &[u8] = b"krc1";

pub fn to_enhanced_lrc(decoded: &[u8]) -> Result<String, String> {
    let krc = decrypt(decoded)?;
    Ok(convert(&krc))
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

fn convert(krc: &str) -> String {
    let line_re = Regex::new(r"^\[(\d+),\d+\](.*)$").unwrap();
    let word_re = Regex::new(r"<(\d+),(\d+),-?\d+>([^<]*)").unwrap();

    let mut out = Vec::new();

    for line in krc.lines() {
        let Some(caps) = line_re.captures(line.trim()) else {
            continue;
        };

        let start: i64 = caps[1].parse().unwrap_or(0);
        let words: Vec<Word> = word_re
            .captures_iter(&caps[2])
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

#[cfg(test)]
mod tests {
    use super::*;

    #[track_caller]
    fn check(krc: &str, expected: &str) {
        assert_eq!(convert(krc), expected);
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
        assert_eq!(convert(""), "");
    }

    #[test]
    fn test_decrypt_rejects_non_krc() {
        assert!(to_enhanced_lrc(b"not a krc payload").is_err());
    }
}
