use std::time::Duration;

use crate::{
    config::ttl::{DEFAULT_CACHE_TTL, DEFAULT_NEGATIVE_CACHE_TTL},
    types::LyricsKind,
};
use extism_pdk::warn;
use host::{get_bool, get_f64, get_optional_i32, get_raw_string, get_string};
use nd_pdk::lyrics::Error;

mod host;
mod providers;
mod ttl;

pub use providers::{ProviderEntry, ProviderMode, ProviderParams};
pub use ttl::TypeCacheTtls;

const DEFAULT_LYRICS_FORMATS: [LyricsKind; 2] = [LyricsKind::Lrc, LyricsKind::Plain];

const DEFAULT_PLAIN_EXTENSION: &str = "txt";
const DEFAULT_INSTRUMENTAL_EXTENSION: &str = "txt";

const DEFAULT_DURATION_TOLERANCE: Duration = Duration::from_secs(3);
const MIN_DURATION_TOLERANCE: Duration = Duration::from_secs(1);
const MAX_DURATION_TOLERANCE: Duration = Duration::from_secs(3600);

const DEFAULT_INSTRUMENTAL_TEXT: &str = "Instrumental";
const DEFAULT_FOLDER_TEMPLATE: &str = "_lyrics/{type}/{track:album_artist} - {track:album}/{track:disc_number:2} - {track:track_number:2} {track:title}";

type Result<T> = std::result::Result<T, Error>;

pub struct PluginConfig {
    pub lyrics_type_priority: Vec<LyricsKind>,
    pub write_lyrics: bool,
    pub overwrite_lyrics: bool,
    pub plain_extension: String,
    pub instrumental_extension: String,
    pub enable_cache: bool,
    pub per_type_cache_ttl: bool,
    pub cache_ttl_hours: i64,
    pub type_cache_ttl_hours: TypeCacheTtls,
    pub negative_cache: bool,
    pub negative_cache_ttl_hours: i64,
    pub providers: Vec<ProviderEntry>,
    pub provider_mode: ProviderMode,
    pub prefer_uncensored: bool,
    pub write_to_specific_folder: bool,
    pub write_to_specific_folder_library_id: Option<i32>,
    pub write_to_specific_folder_template: String,
    pub strip_section_labels: bool,
    pub instrumental_text: Option<String>,
    pub duration_tolerance: Duration,
    pub require_album_match: bool,
}

impl Default for PluginConfig {
    fn default() -> Self {
        Self {
            lyrics_type_priority: DEFAULT_LYRICS_FORMATS.to_vec(),
            write_lyrics: false,
            overwrite_lyrics: false,
            plain_extension: DEFAULT_PLAIN_EXTENSION.to_string(),
            instrumental_extension: DEFAULT_INSTRUMENTAL_EXTENSION.to_string(),
            enable_cache: true,
            per_type_cache_ttl: false,
            cache_ttl_hours: DEFAULT_CACHE_TTL,
            type_cache_ttl_hours: TypeCacheTtls::default(),
            negative_cache: true,
            negative_cache_ttl_hours: DEFAULT_NEGATIVE_CACHE_TTL,
            providers: vec![],
            provider_mode: ProviderMode::default(),
            prefer_uncensored: false,
            write_to_specific_folder: false,
            write_to_specific_folder_library_id: None,
            write_to_specific_folder_template: DEFAULT_FOLDER_TEMPLATE.to_string(),
            strip_section_labels: false,
            instrumental_text: Some(DEFAULT_INSTRUMENTAL_TEXT.to_string()),
            duration_tolerance: DEFAULT_DURATION_TOLERANCE,
            require_album_match: false,
        }
    }
}

impl PluginConfig {
    pub fn load() -> Result<Self> {
        let per_type_cache_ttl = get_bool("perTypeCacheTtl", false)?;

        Ok(Self {
            lyrics_type_priority: resolve_lyrics_type_priority()?,
            write_lyrics: get_bool("writeLyrics", false)?,
            overwrite_lyrics: get_bool("overwriteLyrics", false)?,
            plain_extension: resolve_extension("plainExtension", DEFAULT_PLAIN_EXTENSION)?,
            instrumental_extension: resolve_extension(
                "instrumentalExtension",
                DEFAULT_INSTRUMENTAL_EXTENSION,
            )?,
            enable_cache: get_bool("enableCache", true)?,
            per_type_cache_ttl,
            cache_ttl_hours: ttl::resolve_global()?,
            type_cache_ttl_hours: if per_type_cache_ttl {
                // This costs several host round-trips, so only call it
                // when per-type mode is actually enabled.
                ttl::resolve_per_type()?
            } else {
                TypeCacheTtls::default()
            },
            negative_cache: get_bool("negativeCache", true)?,
            negative_cache_ttl_hours: ttl::resolve_negative()?,
            providers: providers::resolve_list()?,
            provider_mode: providers::resolve_mode()?,
            prefer_uncensored: get_bool("preferUncensored", false)?,
            write_to_specific_folder: get_bool("writeToSpecificFolder", false)?,
            write_to_specific_folder_library_id: get_optional_i32(
                "writeToSpecificFolderLibraryId",
            )?,
            write_to_specific_folder_template: get_string("writeToSpecificFolderTemplate")?
                .unwrap_or_else(|| DEFAULT_FOLDER_TEMPLATE.to_string()),
            strip_section_labels: get_bool("stripSectionLabels", false)?,
            instrumental_text: resolve_instrumental_text()?,
            duration_tolerance: resolve_duration_tolerance()?,
            require_album_match: get_bool("requireAlbumMatch", false)?,
        })
    }

    pub fn wants(&self, kind: LyricsKind) -> bool {
        self.lyrics_type_priority.contains(&kind)
    }

    pub fn cache_ttl_hours_for(&self, kind: LyricsKind) -> i64 {
        if self.per_type_cache_ttl {
            self.type_cache_ttl_hours.get(kind)
        } else {
            self.cache_ttl_hours
        }
    }

    pub fn skips_instrumental(&self) -> bool {
        self.instrumental_text.is_none()
    }

    pub fn extension_for(&self, kind: LyricsKind) -> &str {
        match kind {
            LyricsKind::Plain => self.plain_extension.as_str(),
            LyricsKind::Instrumental => self.instrumental_extension.as_str(),
            LyricsKind::Lrc => "lrc",
            LyricsKind::Elrc => "elrc",
            LyricsKind::Ttml => "ttml",
            LyricsKind::Srt => "srt",
            LyricsKind::Lyricsfile => "yml",
        }
    }
}

fn resolve_instrumental_text() -> Result<Option<String>> {
    Ok(match get_raw_string("instrumentalText")? {
        Some(text) if text.trim().is_empty() => None,
        Some(text) => Some(text),
        None => Some(DEFAULT_INSTRUMENTAL_TEXT.to_string()),
    })
}

fn resolve_lyrics_type_priority() -> Result<Vec<LyricsKind>> {
    let order = match get_string("lyricsFormats")? {
        Some(raw) => parse_lyrics_formats(&raw),
        None => Vec::new(),
    };

    if order.is_empty() {
        let fallback = DEFAULT_LYRICS_FORMATS.map(|kind| kind.slug()).join(" + ");
        warn!("no lyrics formats enabled, defaulting to {fallback}");
        return Ok(DEFAULT_LYRICS_FORMATS.to_vec());
    }

    Ok(order)
}

fn parse_lyrics_formats(raw: &str) -> Vec<LyricsKind> {
    let mut order: Vec<LyricsKind> = Vec::new();
    for slug in raw.split(',') {
        if let Some(kind) = LyricsKind::from_slug(slug)
            && kind != LyricsKind::Instrumental
            && !order.contains(&kind)
        {
            order.push(kind);
        }
    }

    order
}

fn resolve_extension(key: &str, default_value: &str) -> Result<String> {
    let extension = get_string(key)?
        .map(|s| normalize_extension(&s))
        .unwrap_or_else(|| default_value.to_string());

    if extension.is_empty() {
        warn!("{key} resolved to empty string, using '{default_value}'");
        return Ok(default_value.to_string());
    }

    Ok(extension)
}

fn normalize_extension(ext: &str) -> String {
    ext.trim()
        .trim_start_matches('.')
        .chars()
        .filter(|c| {
            !c.is_control() && !matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
        })
        .collect()
}

fn resolve_duration_tolerance() -> Result<Duration> {
    let raw = get_f64(
        "durationToleranceSeconds",
        DEFAULT_DURATION_TOLERANCE.as_secs_f64(),
    )?;

    let duration =
        Duration::from_secs_f64(raw).clamp(MIN_DURATION_TOLERANCE, MAX_DURATION_TOLERANCE);

    if duration.as_secs_f64() != raw {
        warn!("durationToleranceSeconds {raw} is out of range, clamping to {duration:?}");
    }

    Ok(duration)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[track_caller]
    fn check_formats(raw: &str, expected: &[LyricsKind]) {
        assert_eq!(parse_lyrics_formats(raw), expected, "formats from {raw:?}");
    }

    #[track_caller]
    fn check_extension(raw: &str, expected: &str) {
        assert_eq!(normalize_extension(raw), expected, "extension from {raw:?}");
    }

    #[track_caller]
    fn check_cache_ttl(config: &PluginConfig, kind: LyricsKind, expected: i64) {
        assert_eq!(
            config.cache_ttl_hours_for(kind),
            expected,
            "ttl for {kind:?}"
        );
    }

    #[test]
    fn formats_are_tried_in_the_order_they_were_listed() {
        check_formats(
            "ttml,lyricsfile,elrc,lrc,srt,plain",
            &[
                LyricsKind::Ttml,
                LyricsKind::Lyricsfile,
                LyricsKind::Elrc,
                LyricsKind::Lrc,
                LyricsKind::Srt,
                LyricsKind::Plain,
            ],
        );
        check_formats("plain,lrc", &[LyricsKind::Plain, LyricsKind::Lrc]);
    }

    #[test]
    fn format_names_ignore_case_and_padding() {
        check_formats(" LRC , Plain ", &[LyricsKind::Lrc, LyricsKind::Plain]);
    }

    #[test]
    fn a_format_listed_twice_is_wanted_once() {
        check_formats("lrc,lrc", &[LyricsKind::Lrc]);
    }

    #[test]
    fn formats_nobody_recognises_are_dropped() {
        check_formats("lrc,bogus,,lrc", &[LyricsKind::Lrc]);
        check_formats("", &[]);
        check_formats("bogus", &[]);
    }

    #[test]
    fn instrumental_is_never_a_requested_format() {
        check_formats("instrumental,plain", &[LyricsKind::Plain]);
        check_formats("instrumental", &[]);
    }

    #[test]
    fn instrumental_tracks_are_skipped_only_when_their_text_is_blank() {
        let config = PluginConfig {
            instrumental_text: None,
            ..PluginConfig::default()
        };

        assert!(config.skips_instrumental());
    }

    #[test]
    fn one_ttl_covers_every_format_until_per_type_is_turned_on() {
        let config = PluginConfig {
            per_type_cache_ttl: false,
            cache_ttl_hours: 168,
            type_cache_ttl_hours: TypeCacheTtls {
                plain: 1,
                ..TypeCacheTtls::default()
            },
            ..PluginConfig::default()
        };

        check_cache_ttl(&config, LyricsKind::Ttml, 168);
        check_cache_ttl(&config, LyricsKind::Plain, 168);
    }

    #[test]
    fn per_type_ttls_give_each_format_its_own() {
        let config = PluginConfig {
            per_type_cache_ttl: true,
            cache_ttl_hours: 168,
            type_cache_ttl_hours: TypeCacheTtls {
                plain: 1,
                lrc: 2,
                elrc: 3,
                ttml: 4,
                srt: 5,
                lyricsfile: 6,
                instrumental: 7,
            },
            ..PluginConfig::default()
        };

        for (kind, expected) in [
            (LyricsKind::Plain, 1),
            (LyricsKind::Lrc, 2),
            (LyricsKind::Elrc, 3),
            (LyricsKind::Ttml, 4),
            (LyricsKind::Srt, 5),
            (LyricsKind::Lyricsfile, 6),
            (LyricsKind::Instrumental, 7),
        ] {
            check_cache_ttl(&config, kind, expected);
        }
    }

    #[test]
    fn synced_formats_are_written_under_their_own_names() {
        let config = PluginConfig::default();

        for (kind, expected) in [
            (LyricsKind::Lrc, "lrc"),
            (LyricsKind::Elrc, "elrc"),
            (LyricsKind::Ttml, "ttml"),
            (LyricsKind::Srt, "srt"),
            (LyricsKind::Lyricsfile, "yml"),
        ] {
            assert_eq!(
                config.extension_for(kind),
                expected,
                "extension for {kind:?}"
            );
        }
    }

    #[test]
    fn plain_and_instrumental_are_written_under_the_configured_extensions() {
        let config = PluginConfig {
            plain_extension: "text".to_string(),
            instrumental_extension: "inst".to_string(),
            ..PluginConfig::default()
        };

        assert_eq!(config.extension_for(LyricsKind::Plain), "text");
        assert_eq!(config.extension_for(LyricsKind::Instrumental), "inst");
    }

    #[test]
    fn extensions_lose_their_leading_dots_and_padding() {
        for (raw, expected) in [
            ("lrc", "lrc"),
            (".lrc", "lrc"),
            ("...lrc", "lrc"),
            ("  .txt  ", "txt"),
            (".", ""),
        ] {
            check_extension(raw, expected);
        }
    }

    #[test]
    fn an_extension_can_never_carry_a_path() {
        for raw in ["txt/../../evil", "../../etc/passwd", r"txt\..\evil", "a:b"] {
            let extension = normalize_extension(raw);

            assert!(
                !extension.contains(['/', '\\', ':']),
                "{raw:?} left separators in {extension:?}"
            );
        }

        check_extension("///", "");
    }
}
