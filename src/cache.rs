use crate::{
    config::PluginConfig,
    types::{Lyrics, LyricsKind},
};
use extism_pdk::warn;
use flate2::{Compression, read::DeflateDecoder, write::DeflateEncoder};
use nd_pdk::{host::kvstore, lyrics::Error as LyricsError};
use std::io::{self, Read, Write};

const LYRICS_KEY_PREFIX: &str = "lyrics:";
const NEGATIVE_KEY_PREFIX: &str = "miss:";

pub struct LyricsCache<'a> {
    cfg: &'a PluginConfig,
}

impl<'a> LyricsCache<'a> {
    pub fn new(cfg: &'a PluginConfig) -> Self {
        Self { cfg }
    }

    pub fn lookup(&self, track_id: &str) -> Option<Lyrics> {
        let key = lyrics_key(track_id);
        let entry = kvstore::get(&key)
            .inspect_err(|e| warn!("failed to read cache entry '{key}': {e}"))
            .ok()??;

        let lyrics = decode(&entry)
            .inspect_err(|e| warn!("unreadable cache entry '{key}': {e}"))
            .ok()?;

        self.cfg.accepts(&lyrics).then_some(lyrics)
    }

    pub fn write(&self, track_id: &str, lyrics: &Lyrics) -> Result<(), LyricsError> {
        let entry =
            encode(lyrics).map_err(|e| LyricsError::new(format!("compression failed: {e}")))?;

        store(
            &lyrics_key(track_id),
            entry,
            self.cfg.cache_ttl_hours_for(lyrics.kind()),
        )
    }

    pub fn is_negative(&self, track_id: &str, provider: &str) -> bool {
        let key = negative_key(track_id, provider);
        kvstore::has(&key)
            .inspect_err(|e| warn!("failed to read cache entry '{key}': {e}"))
            .unwrap_or(false)
    }

    pub fn write_negative(&self, track_id: &str, provider: &str) -> Result<(), LyricsError> {
        store(
            &negative_key(track_id, provider),
            vec![1],
            self.cfg.negative_cache_ttl_hours,
        )
    }
}

fn store(key: &str, value: Vec<u8>, ttl_hours: i64) -> Result<(), LyricsError> {
    kvstore::set_with_ttl(key, value, ttl_hours.saturating_mul(3600))
        .map_err(|e| LyricsError::new(format!("failed to write cache entry '{key}': {e}")))
}

fn lyrics_key(track_id: &str) -> String {
    format!("{LYRICS_KEY_PREFIX}{track_id}")
}

fn negative_key(track_id: &str, provider: &str) -> String {
    format!("{NEGATIVE_KEY_PREFIX}{provider}:{track_id}")
}

/// Deflates `"{kind}\n{text}"`.
fn encode(lyrics: &Lyrics) -> io::Result<Vec<u8>> {
    let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
    writeln!(encoder, "{}", lyrics.kind().slug())?;
    encoder.write_all(lyrics.content().unwrap_or_default().as_bytes())?;
    encoder.finish()
}

fn decode(entry: &[u8]) -> io::Result<Lyrics> {
    let mut text = String::new();
    DeflateDecoder::new(entry).read_to_string(&mut text)?;

    let (kind, header_len) = text
        .split_once('\n')
        .and_then(|(slug, _)| Some((LyricsKind::from_slug(slug)?, slug.len() + 1)))
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing lyrics type"))?;
    text.drain(..header_len);

    Ok(match kind {
        LyricsKind::Plain => Lyrics::Plain(text),
        LyricsKind::Lrc => Lyrics::Lrc(text),
        LyricsKind::Elrc => Lyrics::Elrc(text),
        LyricsKind::Ttml => Lyrics::Ttml(text),
        LyricsKind::Srt => Lyrics::Srt(text),
        LyricsKind::Lyricsfile => Lyrics::Lyricsfile(text),
        LyricsKind::Instrumental => Lyrics::Instrumental,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[track_caller]
    fn check_roundtrip(lyrics: Lyrics) {
        let entry = encode(&lyrics).expect("encoding should not fail");

        assert_eq!(decode(&entry).expect("decoding should not fail"), lyrics);
    }

    #[track_caller]
    fn check_unreadable(raw: &[u8]) {
        let mut encoder = DeflateEncoder::new(Vec::new(), Compression::fast());
        encoder.write_all(raw).expect("compression should not fail");
        let entry = encoder.finish().expect("compression should not fail");

        assert!(decode(&entry).is_err(), "{raw:?}");
    }

    #[test]
    fn an_entry_is_keyed_by_track() {
        assert_eq!(lyrics_key("abc123"), "lyrics:abc123");
    }

    #[test]
    fn a_miss_is_keyed_by_provider_and_track() {
        assert_eq!(negative_key("abc123", "provider"), "miss:provider:abc123");
    }

    #[test]
    fn cache_roundtrip_does_not_alter_lyrics() {
        let text = "[00:12.34] Hello, world!\n[00:15.67] Testing...";
        check_roundtrip(Lyrics::Plain(text.into()));
        check_roundtrip(Lyrics::Lrc(text.into()));
        check_roundtrip(Lyrics::Elrc(text.into()));
        check_roundtrip(Lyrics::Ttml(text.into()));
        check_roundtrip(Lyrics::Srt(text.into()));
        check_roundtrip(Lyrics::Lyricsfile(text.into()));
        check_roundtrip(Lyrics::Plain("大丈夫 🎵".into()));
        check_roundtrip(Lyrics::Plain(String::new()));
        check_roundtrip(Lyrics::Instrumental);
    }

    #[test]
    fn lyrics_are_compressed() {
        let text = "Lorem ipsum ".repeat(8_000);
        let entry = encode(&Lyrics::Plain(text.clone())).expect("encoding should not fail");

        assert!(
            entry.len() < text.len(),
            "compressed {} bytes into {}",
            text.len(),
            entry.len()
        );
        check_roundtrip(Lyrics::Plain(text));
    }

    #[test]
    fn a_corrupt_entry_is_reported() {
        assert!(decode(&[0xFF, 0xFE, 0xFD, 0xFC]).is_err());
    }

    #[test]
    fn an_entry_that_is_not_utf8_is_reported() {
        check_unreadable(b"lrc\n\x80\x81\x82");
    }

    #[test]
    fn an_entry_without_a_known_type_is_reported() {
        check_unreadable(b"[00:12.34] Hello");
        check_unreadable(b"karaoke\n[00:12.34] Hello");
    }
}
