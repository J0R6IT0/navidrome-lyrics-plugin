//! QQ Music QRC lyrics: a Triple-DES encrypted, zlib compressed XML payload.
//!
//! The DES variant used by QQ Music is non-standard (custom permutation order).
//! Once decrypted, the XML carries a `LyricContent` string whose timed lines
//! look like:
//!   `[lineStart,lineDuration]word(wordStart,wordDuration)word(...)...`
//! where every timestamp is in absolute milliseconds and the timing tuple comes
//! after the word it belongs to.

use crate::format::{elrc, lrc};
use crate::types::Word;
use flate2::read::ZlibDecoder;
use regex::Regex;
use std::io::Read;

/// 24-byte key, split into three 8-byte DES keys.
const QRC_KEY: &[u8; 24] = b"!@#)(*$%123ZXC!@!@#)(NHL";

const ENCRYPT: u8 = 1;
const DECRYPT: u8 = 0;

type RoundKey = [u8; 6];
type DesSchedule = [RoundKey; 16];
type TripleSchedule = [DesSchedule; 3];

#[rustfmt::skip]
const SBOX: [[u8; 64]; 8] = [
    [14, 4, 13, 1, 2, 15, 11, 8, 3, 10, 6, 12, 5, 9, 0, 7,
     0, 15, 7, 4, 14, 2, 13, 1, 10, 6, 12, 11, 9, 5, 3, 8,
     4, 1, 14, 8, 13, 6, 2, 11, 15, 12, 9, 7, 3, 10, 5, 0,
     15, 12, 8, 2, 4, 9, 1, 7, 5, 11, 3, 14, 10, 0, 6, 13],
    [15, 1, 8, 14, 6, 11, 3, 4, 9, 7, 2, 13, 12, 0, 5, 10,
     3, 13, 4, 7, 15, 2, 8, 15, 12, 0, 1, 10, 6, 9, 11, 5,
     0, 14, 7, 11, 10, 4, 13, 1, 5, 8, 12, 6, 9, 3, 2, 15,
     13, 8, 10, 1, 3, 15, 4, 2, 11, 6, 7, 12, 0, 5, 14, 9],
    [10, 0, 9, 14, 6, 3, 15, 5, 1, 13, 12, 7, 11, 4, 2, 8,
     13, 7, 0, 9, 3, 4, 6, 10, 2, 8, 5, 14, 12, 11, 15, 1,
     13, 6, 4, 9, 8, 15, 3, 0, 11, 1, 2, 12, 5, 10, 14, 7,
     1, 10, 13, 0, 6, 9, 8, 7, 4, 15, 14, 3, 11, 5, 2, 12],
    [7, 13, 14, 3, 0, 6, 9, 10, 1, 2, 8, 5, 11, 12, 4, 15,
     13, 8, 11, 5, 6, 15, 0, 3, 4, 7, 2, 12, 1, 10, 14, 9,
     10, 6, 9, 0, 12, 11, 7, 13, 15, 1, 3, 14, 5, 2, 8, 4,
     3, 15, 0, 6, 10, 10, 13, 8, 9, 4, 5, 11, 12, 7, 2, 14],
    [2, 12, 4, 1, 7, 10, 11, 6, 8, 5, 3, 15, 13, 0, 14, 9,
     14, 11, 2, 12, 4, 7, 13, 1, 5, 0, 15, 10, 3, 9, 8, 6,
     4, 2, 1, 11, 10, 13, 7, 8, 15, 9, 12, 5, 6, 3, 0, 14,
     11, 8, 12, 7, 1, 14, 2, 13, 6, 15, 0, 9, 10, 4, 5, 3],
    [12, 1, 10, 15, 9, 2, 6, 8, 0, 13, 3, 4, 14, 7, 5, 11,
     10, 15, 4, 2, 7, 12, 9, 5, 6, 1, 13, 14, 0, 11, 3, 8,
     9, 14, 15, 5, 2, 8, 12, 3, 7, 0, 4, 10, 1, 13, 11, 6,
     4, 3, 2, 12, 9, 5, 15, 10, 11, 14, 1, 7, 6, 0, 8, 13],
    [4, 11, 2, 14, 15, 0, 8, 13, 3, 12, 9, 7, 5, 10, 6, 1,
     13, 0, 11, 7, 4, 9, 1, 10, 14, 3, 5, 12, 2, 15, 8, 6,
     1, 4, 11, 13, 12, 3, 7, 14, 10, 15, 6, 8, 0, 5, 9, 2,
     6, 11, 13, 8, 1, 4, 10, 7, 9, 5, 0, 15, 14, 2, 3, 12],
    [13, 2, 8, 4, 6, 15, 11, 1, 10, 9, 3, 14, 5, 0, 12, 7,
     1, 15, 13, 8, 10, 3, 7, 4, 12, 5, 6, 11, 0, 14, 9, 2,
     7, 11, 4, 1, 9, 12, 14, 2, 0, 6, 10, 13, 15, 3, 5, 8,
     2, 1, 14, 7, 4, 10, 8, 13, 15, 12, 9, 0, 3, 5, 6, 11],
];

/// Decrypts a hex-encoded QRC payload and returns the inner XML string.
pub fn decrypt(hex: &str) -> Result<String, String> {
    let bytes = hex_decode(hex)?;
    if bytes.is_empty() || !bytes.len().is_multiple_of(8) {
        return Err("qrc payload length is not a multiple of 8".to_string());
    }

    let schedule = triple_des_key_setup(QRC_KEY, DECRYPT);

    let mut data = Vec::with_capacity(bytes.len());
    for block in bytes.as_chunks::<8>().0 {
        let mut input = [0u8; 8];
        input.copy_from_slice(block);
        data.extend_from_slice(&triple_des_crypt(&input, &schedule));
    }

    let mut decoder = ZlibDecoder::new(&data[..]);
    let mut out = Vec::new();
    decoder
        .read_to_end(&mut out)
        .map_err(|e| format!("qrc inflate failed: {e}"))?;

    String::from_utf8(out).map_err(|e| format!("qrc is not valid UTF-8: {e}"))
}

struct QrcLine {
    start_ms: u64,
    words: Vec<QrcWord>,
}

struct QrcWord {
    text: String,
    start_ms: u64,
    duration_ms: u64,
}

pub fn to_enhanced_lrc(content: &str) -> String {
    let mut out = Vec::new();

    for line in parse_lines(content) {
        let words: Vec<Word> = line
            .words
            .iter()
            .map(|word| Word {
                text: word.text.clone(),
                start_ms: word.start_ms as i64,
                end_ms: (word.start_ms + word.duration_ms) as i64,
            })
            .collect();

        if let Some(rendered) = elrc::render_line(line.start_ms as i64, &words) {
            out.push(rendered);
        }
    }

    out.join("\n")
}

pub fn to_lrc(content: &str) -> String {
    let mut out = Vec::new();

    for line in parse_lines(content) {
        let text: String = line.words.iter().map(|w| w.text.as_str()).collect();
        out.push(format!(
            "[{}]{}",
            lrc::format_timestamp(line.start_ms as i64),
            text
        ));
    }

    out.join("\n")
}

fn parse_lines(content: &str) -> Vec<QrcLine> {
    let line_re = Regex::new(r"^\[(\d+),(\d+)\](.*)$").unwrap();
    let word_re = Regex::new(r"\((\d+),(\d+)\)").unwrap();

    let mut lines = Vec::new();

    for raw in content.lines() {
        let Some(caps) = line_re.captures(raw.trim()) else {
            continue;
        };

        let start_ms: u64 = caps[1].parse().unwrap_or(0);
        let body = caps.get(3).map_or("", |m| m.as_str());

        let mut words = Vec::new();
        let mut cursor = 0;
        for m in word_re.captures_iter(body) {
            let whole = m.get(0).unwrap();
            let text = body[cursor..whole.start()].to_string();
            cursor = whole.end();
            words.push(QrcWord {
                text,
                start_ms: m[1].parse().unwrap_or(0),
                duration_ms: m[2].parse().unwrap_or(0),
            });
        }

        if !words.is_empty() {
            lines.push(QrcLine { start_ms, words });
        }
    }

    lines
}

pub fn extract_lyric_content(xml: &str) -> Option<String> {
    let re = Regex::new(r#"LyricContent="([\s\S]*?)"\s*/>"#).unwrap();
    re.captures(xml)
        .map(|c| c[1].to_string())
        .filter(|s| !s.trim().is_empty())
}

pub fn is_qrc(content: &str) -> bool {
    let re = Regex::new(r"(?m)^\[\d+,\d+\]").unwrap();
    re.is_match(content)
}

fn hex_decode(hex: &str) -> Result<Vec<u8>, String> {
    let hex = hex.trim().as_bytes();
    if !hex.len().is_multiple_of(2) {
        return Err("qrc payload is not valid hex (odd length)".to_string());
    }

    hex.as_chunks::<2>()
        .0
        .iter()
        .map(|pair| {
            let hi = (pair[0] as char).to_digit(16);
            let lo = (pair[1] as char).to_digit(16);
            match (hi, lo) {
                (Some(hi), Some(lo)) => Ok((hi * 16 + lo) as u8),
                _ => Err("qrc payload is not valid hex".to_string()),
            }
        })
        .collect()
}

fn bitnum(a: &[u8; 8], b: usize, c: u32) -> u32 {
    (((a[(b / 32) * 4 + 3 - (b % 32) / 8] >> (7 - b % 8)) & 1) as u32) << c
}

fn bitnum_intr(a: u32, b: u32, c: u32) -> u32 {
    ((a >> (31 - b)) & 1) << c
}

fn bitnum_intl(a: u32, b: u32, c: u32) -> u32 {
    (a.wrapping_shl(b) & 0x8000_0000) >> c
}

fn sbox_bit(a: u32) -> usize {
    ((a & 32) | ((a & 31) >> 1) | ((a & 1) << 4)) as usize
}

fn initial_permutation(input: &[u8; 8]) -> (u32, u32) {
    let s0 = bitnum(input, 57, 31)
        | bitnum(input, 49, 30)
        | bitnum(input, 41, 29)
        | bitnum(input, 33, 28)
        | bitnum(input, 25, 27)
        | bitnum(input, 17, 26)
        | bitnum(input, 9, 25)
        | bitnum(input, 1, 24)
        | bitnum(input, 59, 23)
        | bitnum(input, 51, 22)
        | bitnum(input, 43, 21)
        | bitnum(input, 35, 20)
        | bitnum(input, 27, 19)
        | bitnum(input, 19, 18)
        | bitnum(input, 11, 17)
        | bitnum(input, 3, 16)
        | bitnum(input, 61, 15)
        | bitnum(input, 53, 14)
        | bitnum(input, 45, 13)
        | bitnum(input, 37, 12)
        | bitnum(input, 29, 11)
        | bitnum(input, 21, 10)
        | bitnum(input, 13, 9)
        | bitnum(input, 5, 8)
        | bitnum(input, 63, 7)
        | bitnum(input, 55, 6)
        | bitnum(input, 47, 5)
        | bitnum(input, 39, 4)
        | bitnum(input, 31, 3)
        | bitnum(input, 23, 2)
        | bitnum(input, 15, 1)
        | bitnum(input, 7, 0);

    let s1 = bitnum(input, 56, 31)
        | bitnum(input, 48, 30)
        | bitnum(input, 40, 29)
        | bitnum(input, 32, 28)
        | bitnum(input, 24, 27)
        | bitnum(input, 16, 26)
        | bitnum(input, 8, 25)
        | bitnum(input, 0, 24)
        | bitnum(input, 58, 23)
        | bitnum(input, 50, 22)
        | bitnum(input, 42, 21)
        | bitnum(input, 34, 20)
        | bitnum(input, 26, 19)
        | bitnum(input, 18, 18)
        | bitnum(input, 10, 17)
        | bitnum(input, 2, 16)
        | bitnum(input, 60, 15)
        | bitnum(input, 52, 14)
        | bitnum(input, 44, 13)
        | bitnum(input, 36, 12)
        | bitnum(input, 28, 11)
        | bitnum(input, 20, 10)
        | bitnum(input, 12, 9)
        | bitnum(input, 4, 8)
        | bitnum(input, 62, 7)
        | bitnum(input, 54, 6)
        | bitnum(input, 46, 5)
        | bitnum(input, 38, 4)
        | bitnum(input, 30, 3)
        | bitnum(input, 22, 2)
        | bitnum(input, 14, 1)
        | bitnum(input, 6, 0);

    (s0, s1)
}

fn inverse_permutation(s0: u32, s1: u32) -> [u8; 8] {
    let mut d = [0u8; 8];
    d[3] = (bitnum_intr(s1, 7, 7)
        | bitnum_intr(s0, 7, 6)
        | bitnum_intr(s1, 15, 5)
        | bitnum_intr(s0, 15, 4)
        | bitnum_intr(s1, 23, 3)
        | bitnum_intr(s0, 23, 2)
        | bitnum_intr(s1, 31, 1)
        | bitnum_intr(s0, 31, 0)) as u8;
    d[2] = (bitnum_intr(s1, 6, 7)
        | bitnum_intr(s0, 6, 6)
        | bitnum_intr(s1, 14, 5)
        | bitnum_intr(s0, 14, 4)
        | bitnum_intr(s1, 22, 3)
        | bitnum_intr(s0, 22, 2)
        | bitnum_intr(s1, 30, 1)
        | bitnum_intr(s0, 30, 0)) as u8;
    d[1] = (bitnum_intr(s1, 5, 7)
        | bitnum_intr(s0, 5, 6)
        | bitnum_intr(s1, 13, 5)
        | bitnum_intr(s0, 13, 4)
        | bitnum_intr(s1, 21, 3)
        | bitnum_intr(s0, 21, 2)
        | bitnum_intr(s1, 29, 1)
        | bitnum_intr(s0, 29, 0)) as u8;
    d[0] = (bitnum_intr(s1, 4, 7)
        | bitnum_intr(s0, 4, 6)
        | bitnum_intr(s1, 12, 5)
        | bitnum_intr(s0, 12, 4)
        | bitnum_intr(s1, 20, 3)
        | bitnum_intr(s0, 20, 2)
        | bitnum_intr(s1, 28, 1)
        | bitnum_intr(s0, 28, 0)) as u8;
    d[7] = (bitnum_intr(s1, 3, 7)
        | bitnum_intr(s0, 3, 6)
        | bitnum_intr(s1, 11, 5)
        | bitnum_intr(s0, 11, 4)
        | bitnum_intr(s1, 19, 3)
        | bitnum_intr(s0, 19, 2)
        | bitnum_intr(s1, 27, 1)
        | bitnum_intr(s0, 27, 0)) as u8;
    d[6] = (bitnum_intr(s1, 2, 7)
        | bitnum_intr(s0, 2, 6)
        | bitnum_intr(s1, 10, 5)
        | bitnum_intr(s0, 10, 4)
        | bitnum_intr(s1, 18, 3)
        | bitnum_intr(s0, 18, 2)
        | bitnum_intr(s1, 26, 1)
        | bitnum_intr(s0, 26, 0)) as u8;
    d[5] = (bitnum_intr(s1, 1, 7)
        | bitnum_intr(s0, 1, 6)
        | bitnum_intr(s1, 9, 5)
        | bitnum_intr(s0, 9, 4)
        | bitnum_intr(s1, 17, 3)
        | bitnum_intr(s0, 17, 2)
        | bitnum_intr(s1, 25, 1)
        | bitnum_intr(s0, 25, 0)) as u8;
    d[4] = (bitnum_intr(s1, 0, 7)
        | bitnum_intr(s0, 0, 6)
        | bitnum_intr(s1, 8, 5)
        | bitnum_intr(s0, 8, 4)
        | bitnum_intr(s1, 16, 3)
        | bitnum_intr(s0, 16, 2)
        | bitnum_intr(s1, 24, 1)
        | bitnum_intr(s0, 24, 0)) as u8;
    d
}

fn feistel(state: u32, key: &RoundKey) -> u32 {
    let t1 = bitnum_intl(state, 31, 0)
        | ((state & 0xf000_0000) >> 1)
        | bitnum_intl(state, 4, 5)
        | bitnum_intl(state, 3, 6)
        | ((state & 0x0f00_0000) >> 3)
        | bitnum_intl(state, 8, 11)
        | bitnum_intl(state, 7, 12)
        | ((state & 0x00f0_0000) >> 5)
        | bitnum_intl(state, 12, 17)
        | bitnum_intl(state, 11, 18)
        | ((state & 0x000f_0000) >> 7)
        | bitnum_intl(state, 16, 23);

    let t2 = bitnum_intl(state, 15, 0)
        | ((state & 0x0000_f000) << 15)
        | bitnum_intl(state, 20, 5)
        | bitnum_intl(state, 19, 6)
        | ((state & 0x0000_0f00) << 13)
        | bitnum_intl(state, 24, 11)
        | bitnum_intl(state, 23, 12)
        | ((state & 0x0000_00f0) << 11)
        | bitnum_intl(state, 28, 17)
        | bitnum_intl(state, 27, 18)
        | ((state & 0x0000_000f) << 9)
        | bitnum_intl(state, 0, 23);

    let lrg = [
        ((t1 >> 24) & 0xff) ^ key[0] as u32,
        ((t1 >> 16) & 0xff) ^ key[1] as u32,
        ((t1 >> 8) & 0xff) ^ key[2] as u32,
        ((t2 >> 24) & 0xff) ^ key[3] as u32,
        ((t2 >> 16) & 0xff) ^ key[4] as u32,
        ((t2 >> 8) & 0xff) ^ key[5] as u32,
    ];

    let state = ((SBOX[0][sbox_bit(lrg[0] >> 2)] as u32) << 28)
        | ((SBOX[1][sbox_bit(((lrg[0] & 0x03) << 4) | (lrg[1] >> 4))] as u32) << 24)
        | ((SBOX[2][sbox_bit(((lrg[1] & 0x0f) << 2) | (lrg[2] >> 6))] as u32) << 20)
        | ((SBOX[3][sbox_bit(lrg[2] & 0x3f)] as u32) << 16)
        | ((SBOX[4][sbox_bit(lrg[3] >> 2)] as u32) << 12)
        | ((SBOX[5][sbox_bit(((lrg[3] & 0x03) << 4) | (lrg[4] >> 4))] as u32) << 8)
        | ((SBOX[6][sbox_bit(((lrg[4] & 0x0f) << 2) | (lrg[5] >> 6))] as u32) << 4)
        | (SBOX[7][sbox_bit(lrg[5] & 0x3f)] as u32);

    bitnum_intl(state, 15, 0)
        | bitnum_intl(state, 6, 1)
        | bitnum_intl(state, 19, 2)
        | bitnum_intl(state, 20, 3)
        | bitnum_intl(state, 28, 4)
        | bitnum_intl(state, 11, 5)
        | bitnum_intl(state, 27, 6)
        | bitnum_intl(state, 16, 7)
        | bitnum_intl(state, 0, 8)
        | bitnum_intl(state, 14, 9)
        | bitnum_intl(state, 22, 10)
        | bitnum_intl(state, 25, 11)
        | bitnum_intl(state, 4, 12)
        | bitnum_intl(state, 17, 13)
        | bitnum_intl(state, 30, 14)
        | bitnum_intl(state, 9, 15)
        | bitnum_intl(state, 1, 16)
        | bitnum_intl(state, 7, 17)
        | bitnum_intl(state, 23, 18)
        | bitnum_intl(state, 13, 19)
        | bitnum_intl(state, 31, 20)
        | bitnum_intl(state, 26, 21)
        | bitnum_intl(state, 2, 22)
        | bitnum_intl(state, 8, 23)
        | bitnum_intl(state, 18, 24)
        | bitnum_intl(state, 12, 25)
        | bitnum_intl(state, 29, 26)
        | bitnum_intl(state, 5, 27)
        | bitnum_intl(state, 21, 28)
        | bitnum_intl(state, 10, 29)
        | bitnum_intl(state, 3, 30)
        | bitnum_intl(state, 24, 31)
}

fn crypt(input: &[u8; 8], schedule: &DesSchedule) -> [u8; 8] {
    let (mut s0, mut s1) = initial_permutation(input);

    for round in &schedule[..15] {
        let previous = s1;
        s1 = feistel(s1, round) ^ s0;
        s0 = previous;
    }
    s0 ^= feistel(s1, &schedule[15]);

    inverse_permutation(s0, s1)
}

#[rustfmt::skip]
const KEY_PERM_C: [usize; 28] = [
    56, 48, 40, 32, 24, 16, 8, 0, 57, 49, 41, 33, 25, 17,
    9, 1, 58, 50, 42, 34, 26, 18, 10, 2, 59, 51, 43, 35,
];
#[rustfmt::skip]
const KEY_PERM_D: [usize; 28] = [
    62, 54, 46, 38, 30, 22, 14, 6, 61, 53, 45, 37, 29, 21,
    13, 5, 60, 52, 44, 36, 28, 20, 12, 4, 27, 19, 11, 3,
];
#[rustfmt::skip]
const KEY_COMPRESSION: [u32; 48] = [
    13, 16, 10, 23, 0, 4, 2, 27, 14, 5, 20, 9, 22, 18, 11, 3,
    25, 7, 15, 6, 26, 19, 12, 1, 40, 51, 30, 36, 46, 54, 29, 39,
    50, 44, 32, 47, 43, 48, 38, 55, 33, 52, 45, 41, 49, 35, 28, 31,
];
const KEY_RND_SHIFT: [u32; 16] = [1, 1, 2, 2, 2, 2, 2, 2, 1, 2, 2, 2, 2, 2, 2, 1];

fn key_schedule(key: &[u8; 8], mode: u8) -> DesSchedule {
    let mut schedule = [[0u8; 6]; 16];

    let mut c: u32 = (0..28)
        .map(|i| bitnum(key, KEY_PERM_C[i], 31 - i as u32))
        .sum();
    let mut d: u32 = (0..28)
        .map(|i| bitnum(key, KEY_PERM_D[i], 31 - i as u32))
        .sum();

    for (i, &shift) in KEY_RND_SHIFT.iter().enumerate() {
        c = (c.wrapping_shl(shift) | (c >> (28 - shift))) & 0xffff_fff0;
        d = (d.wrapping_shl(shift) | (d >> (28 - shift))) & 0xffff_fff0;

        let togen = if mode == DECRYPT { 15 - i } else { i };
        let round = &mut schedule[togen];

        for j in 0..24 {
            round[j / 8] |= bitnum_intr(c, KEY_COMPRESSION[j], 7 - (j as u32 % 8)) as u8;
        }
        for j in 24..48 {
            round[j / 8] |= bitnum_intr(d, KEY_COMPRESSION[j] - 27, 7 - (j as u32 % 8)) as u8;
        }
    }

    schedule
}

fn triple_des_key_setup(key: &[u8; 24], mode: u8) -> TripleSchedule {
    let k0: [u8; 8] = key[0..8].try_into().unwrap();
    let k1: [u8; 8] = key[8..16].try_into().unwrap();
    let k2: [u8; 8] = key[16..24].try_into().unwrap();

    if mode == ENCRYPT {
        [
            key_schedule(&k0, ENCRYPT),
            key_schedule(&k1, DECRYPT),
            key_schedule(&k2, ENCRYPT),
        ]
    } else {
        [
            key_schedule(&k2, DECRYPT),
            key_schedule(&k1, ENCRYPT),
            key_schedule(&k0, DECRYPT),
        ]
    }
}

fn triple_des_crypt(data: &[u8; 8], schedule: &TripleSchedule) -> [u8; 8] {
    let mut block = *data;
    for des in schedule {
        block = crypt(&block, des);
    }
    block
}

#[cfg(test)]
mod tests {
    use super::*;

    #[track_caller]
    fn check_to_elrc(qrc: &str, expected: &str) {
        assert_eq!(to_enhanced_lrc(qrc), expected);
    }

    #[test]
    fn qrc_line_to_elrc() {
        check_to_elrc(
            "[370,2624]Is (370,336)this (706,285)the (991,355)real (1346,904)life(2250,744)",
            "[00:00.37]<00:00.37>Is <00:00.71>this <00:00.99>the <00:01.35>real <00:02.25>life<00:02.99>",
        );
    }

    #[track_caller]
    fn check_to_lrc(qrc: &str, expected: &str) {
        assert_eq!(to_lrc(qrc), expected);
    }

    #[test]
    fn test_to_lrc_concatenates_words() {
        check_to_lrc(
            "[370,2624]Is (370,336)this (706,285)the (991,355)real (1346,904)life(2250,744)",
            "[00:00.37]Is this the real life",
        );
    }

    #[test]
    fn keeps_parens_in_text() {
        check_to_lrc(
            "[0,90]((0,2)Remastered(2,21))(23,2)",
            "[00:00.00](Remastered)",
        );
    }

    #[test]
    fn skips_metadata_lines() {
        check_to_lrc("[ti:Song]\n[ar:Artist]\n[0,1000]hi(0,500)", "[00:00.00]hi");
    }

    #[track_caller]
    fn check_is_qrc(qrc: &str, expected: bool) {
        assert_eq!(is_qrc(qrc), expected);
    }

    #[test]
    fn qrc_is_detected() {
        check_is_qrc("[0,90]hi(0,16)", true);
        check_is_qrc("[00:01.00]hi", false);
    }

    #[track_caller]
    fn check_extract_lyric_content(xml: &str, expected: Option<&str>) {
        assert_eq!(extract_lyric_content(xml).as_deref(), expected);
    }

    #[test]
    fn lyric_content_is_extracted() {
        check_extract_lyric_content(
            r#"<Lyric_1 LyricType="1" LyricContent="[0,90]hi(0,16)"/>"#,
            Some("[0,90]hi(0,16)"),
        );
    }

    #[test]
    fn empty_lyric_content_is_none() {
        check_extract_lyric_content(r#"<Lyric_1 LyricType="1" LyricContent=""/>"#, None);
    }

    #[test]
    fn empty_test() {
        check_to_elrc("", "");
        check_to_elrc("", "");
    }
}
