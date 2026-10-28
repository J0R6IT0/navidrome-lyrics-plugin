use crate::cache::LyricsCache;
use crate::config::{PluginConfig, ProviderEntry};
use crate::format::ttml;
use crate::providers::error::ProviderError;
use crate::providers::{LyricsProvider, ProviderLyrics, ProviderRegistry, register_providers};
use crate::types::{Lyrics, LyricsKind};
use extism_pdk::{info, warn};
use nd_pdk::lyrics::TrackInfo;
use ranking::{Rank, Ranker};

mod ranking;
mod ratelimit;
mod selection;

pub enum Outcome {
    Found(Lyrics),
    NotFound,
    ProviderError,
}

pub fn run(track: &TrackInfo, cfg: &PluginConfig, cache: Option<&LyricsCache>) -> Outcome {
    let mut registry = ProviderRegistry::new();
    register_providers(&mut registry);

    Orchestrator {
        registry: &registry,
        track,
        cfg,
        negative_cache: cfg.negative_cache.then_some(cache).flatten(),
    }
    .run()
}

struct PreparedProvider {
    provider: Box<dyn LyricsProvider>,
    label: String,
    provider_id: String,
}

enum ProviderFetch {
    Lyrics(ProviderLyrics),
    Empty,
    Error,
}

struct Candidate {
    fetched: ProviderLyrics,
    rank: Rank,
}

struct Orchestrator<'a> {
    registry: &'a ProviderRegistry,
    track: &'a TrackInfo,
    cfg: &'a PluginConfig,
    negative_cache: Option<&'a LyricsCache>,
}

impl Orchestrator<'_> {
    fn run(&self) -> Outcome {
        match Ranker::for_mode(self.cfg) {
            Some(ranker) => self.best_of(ranker),
            None => self.sequential(),
        }
    }

    fn prepared_providers(&self) -> impl Iterator<Item = PreparedProvider> + '_ {
        selection::order_providers(self.cfg)
            .into_iter()
            .filter_map(|entry| self.prepare(entry))
    }

    fn sequential(&self) -> Outcome {
        let mut had_error = false;

        for prepared in self.prepared_providers() {
            match self.fetch_one(&prepared) {
                ProviderFetch::Lyrics(fetched) => return self.found(fetched),
                ProviderFetch::Error => had_error = true,
                ProviderFetch::Empty => {}
            }
        }

        outcome_without_lyrics(had_error)
    }

    fn best_of(&self, ranker: Ranker) -> Outcome {
        let mut best: Option<Candidate> = None;
        let mut had_error = false;

        for prepared in self.prepared_providers() {
            if let Some(current) = &best
                && let Some(ceiling) = ranker.ceiling(prepared.provider.supported_kinds())
                && ceiling >= current.rank
            {
                info!(
                    "skipping provider '{}': best possible ({}) cannot improve on current {} lyrics",
                    prepared.label,
                    ranker.describe(ceiling),
                    ranker.describe(current.rank),
                );
                continue;
            }

            match self.fetch_one(&prepared) {
                ProviderFetch::Lyrics(fetched) => {
                    if matches!(fetched.lyrics, Lyrics::Instrumental) {
                        return Outcome::Found(fetched.lyrics);
                    }

                    let rank = ranker.rank(&fetched.lyrics);
                    if rank.is_censored() {
                        info!("provider '{}' returned censored lyrics", prepared.label);
                    }

                    if best.as_ref().is_none_or(|current| rank < current.rank) {
                        best = Some(Candidate { fetched, rank });
                    }

                    if rank == Rank::BEST {
                        break;
                    }
                }
                ProviderFetch::Error => had_error = true,
                ProviderFetch::Empty => {}
            }
        }

        match best {
            Some(candidate) => self.found(candidate.fetched),
            None => outcome_without_lyrics(had_error),
        }
    }

    fn found(&self, ProviderLyrics { lyrics, tracks }: ProviderLyrics) -> Outcome {
        if tracks.is_empty() {
            return Outcome::Found(lyrics);
        }

        if !self.cfg.wants(LyricsKind::Ttml) {
            info!(
                "leaving out {} translation tracks: ttml is not an enabled lyrics type",
                tracks.len()
            );
            return Outcome::Found(lyrics);
        }

        match ttml::with_tracks(&lyrics, &tracks) {
            Some(ttml) => {
                info!("merged translation tracks into ttml lyrics");
                Outcome::Found(Lyrics::Ttml(ttml))
            }
            None => {
                info!(
                    "no translation track matched the {} lyrics",
                    lyrics.kind().slug()
                );
                Outcome::Found(lyrics)
            }
        }
    }

    fn prepare(&self, entry: &ProviderEntry) -> Option<PreparedProvider> {
        let Some(provider) = self.registry.create(entry) else {
            warn!("unknown provider '{}', skipping", entry.name);
            return None;
        };

        if !provider
            .supported_kinds()
            .iter()
            .any(|&kind| self.cfg.wants(kind))
        {
            return None;
        }

        Some(PreparedProvider {
            label: provider_label(&entry.name, &provider.log_params()),
            provider_id: entry.id(),
            provider,
        })
    }

    fn fetch_one(&self, prepared: &PreparedProvider) -> ProviderFetch {
        let PreparedProvider {
            provider,
            label,
            provider_id,
        } = prepared;

        if let Some(time_left) = ratelimit::remaining(provider_id) {
            info!("skipping rate limited provider '{label}', time left: {time_left}s");
            return ProviderFetch::Empty;
        }

        if self.is_negative(provider_id) {
            info!(
                "provider '{}' negative-cached for this track, skipping",
                label
            );
            return ProviderFetch::Empty;
        }

        info!("trying provider '{}'", label);
        match provider.fetch_lyrics(self.track, self.cfg) {
            Ok(Some(mut fetched)) => {
                fetched.lyrics.sanitize(self.cfg);
                if fetched.lyrics.is_empty() {
                    warn!(
                        "provider '{}' returned empty lyrics after sanitization, skipping",
                        label
                    );
                    self.mark_negative(provider_id);
                    ProviderFetch::Empty
                } else {
                    info!(
                        "provider '{}' returned {} lyrics",
                        label,
                        fetched.lyrics.kind().slug()
                    );
                    ProviderFetch::Lyrics(fetched)
                }
            }
            Ok(None) => {
                info!("provider '{}' returned no lyrics", label);
                self.mark_negative(provider_id);
                ProviderFetch::Empty
            }
            Err(ProviderError::RateLimited { retry_after_secs }) => {
                info!(
                    "provider '{label}' has been rate limited, backing off for {retry_after_secs}s"
                );
                ratelimit::record(provider_id, retry_after_secs);
                ProviderFetch::Empty
            }
            Err(e) => {
                warn!("provider '{}' failed: {}", label, e);
                ProviderFetch::Error
            }
        }
    }

    fn is_negative(&self, provider_id: &str) -> bool {
        self.negative_cache
            .is_some_and(|cache| cache.is_negative(&self.track.id, provider_id))
    }

    fn mark_negative(&self, provider_id: &str) {
        let Some(cache) = self.negative_cache else {
            return;
        };

        match cache.write_negative(&self.track.id, provider_id) {
            Ok(()) => info!(
                "cached negative result for track '{}' (provider {provider_id}, ttl {}h)",
                self.track.id, self.cfg.negative_cache_ttl_hours
            ),
            Err(err) => warn!("failed to persist negative cache entry: {err}"),
        }
    }
}

fn outcome_without_lyrics(had_error: bool) -> Outcome {
    if had_error {
        Outcome::ProviderError
    } else {
        Outcome::NotFound
    }
}

fn provider_label(name: &str, params: &[(&'static str, String)]) -> String {
    if params.is_empty() {
        return name.to_string();
    }

    let joined = params
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join(", ");

    format!("{name}({joined})")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_without_params_label_has_only_name() {
        assert_eq!(provider_label("kugou", &[]), "kugou");
    }

    #[test]
    fn provider_label_includes_params() {
        let params = [("baseUrl", "http://localhost:7592".to_string())];
        assert_eq!(
            provider_label("lrclib", &params),
            "lrclib(baseUrl=http://localhost:7592)"
        );
    }
}
