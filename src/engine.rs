use crate::cache::LyricsCache;
use crate::config::PluginConfig;
use crate::ext::TrackInfoExt;
use crate::fetch::{self, Outcome};
use crate::types::Lyrics;
use crate::writing;
use extism_pdk::{info, warn};
use nd_pdk::lyrics::{Error as LyricsError, GetLyricsResponse, TrackInfo};

pub fn get_lyrics(track: TrackInfo) -> Result<GetLyricsResponse, LyricsError> {
    let cfg = PluginConfig::load()?;
    let cache = cfg.enable_cache.then(|| LyricsCache::new(&cfg));

    let lyrics = match lookup_cached(cache.as_ref(), &track) {
        Some(lyrics) => lyrics,
        None => {
            let lyrics = fetch_from_providers(&track, &cfg, cache.as_ref())?;
            store_in_cache(cache.as_ref(), &track, &lyrics, &cfg);
            lyrics
        }
    };

    if cfg.skips_instrumental() && matches!(lyrics, Lyrics::Instrumental) {
        let message = format!("'{}' is an instrumental track", track.label());
        info!("{message}");
        return Err(LyricsError::new(message));
    }

    if cfg.write_lyrics
        && let Err(err) = writing::write(&track, &lyrics, &cfg)
    {
        warn!("failed to write lyrics file to disk: {err}");
    }

    Ok(lyrics.to_response(&cfg))
}

fn lookup_cached(cache: Option<&LyricsCache>, track: &TrackInfo) -> Option<Lyrics> {
    let lyrics = cache?.lookup(&track.id);
    match &lyrics {
        Some(lyrics) => info!(
            "cache hit ({}) for '{}'",
            lyrics.kind().slug(),
            track.label()
        ),
        None => info!("cache miss for '{}'", track.label()),
    }
    lyrics
}

fn fetch_from_providers(
    track: &TrackInfo,
    cfg: &PluginConfig,
    cache: Option<&LyricsCache>,
) -> Result<Lyrics, LyricsError> {
    let reason = match fetch::run(track, cfg, cache) {
        Outcome::Found(lyrics) => return Ok(lyrics),
        Outcome::NotFound => "no lyrics found from any provider",
        Outcome::ProviderError => "all providers errored while fetching lyrics",
    };
    Err(LyricsError::new(format!(
        "{reason} for '{}'",
        track.label()
    )))
}

fn store_in_cache(
    cache: Option<&LyricsCache>,
    track: &TrackInfo,
    lyrics: &Lyrics,
    cfg: &PluginConfig,
) {
    let Some(cache) = cache else {
        return;
    };

    match cache.write(&track.id, lyrics) {
        Ok(()) => info!(
            "cached {} lyrics for '{}' (ttl {}h)",
            lyrics.kind().slug(),
            track.label(),
            cfg.cache_ttl_hours_for(lyrics.kind())
        ),
        Err(err) => warn!("failed to persist lyrics to cache: {err}"),
    }
}
