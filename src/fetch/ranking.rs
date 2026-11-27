use crate::config::{PluginConfig, ProviderMode};
use crate::types::{Lyrics, LyricsKind, SyncLevel};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct Rank {
    primary: usize,
    censored: bool,
}

impl Rank {
    pub(super) const BEST: Rank = Rank {
        primary: 0,
        censored: false,
    };

    pub(super) fn is_censored(&self) -> bool {
        self.censored
    }
}

pub(super) struct Ranker<'a> {
    basis: Basis<'a>,
    prefer_uncensored: bool,
}

enum Basis<'a> {
    TypePriority(&'a [LyricsKind]),
    SyncLevel(&'a [LyricsKind]),
}

impl<'a> Ranker<'a> {
    pub(super) fn for_mode(cfg: &'a PluginConfig) -> Option<Self> {
        let priority = &cfg.lyrics_type_priority;

        let basis = match cfg.provider_mode {
            ProviderMode::TypePriority => Basis::TypePriority(priority),
            ProviderMode::BestSyncLevel => Basis::SyncLevel(priority),
            ProviderMode::Priority | ProviderMode::Rotation => return None,
        };

        Some(Self {
            basis,
            prefer_uncensored: cfg.prefer_uncensored,
        })
    }

    pub(super) fn rank(&self, lyrics: &Lyrics) -> Rank {
        Rank {
            primary: match &self.basis {
                Basis::TypePriority(priority) => kind_rank(lyrics.kind(), priority),
                Basis::SyncLevel(_) => lyrics.sync_level().rank(),
            },
            censored: self.prefer_uncensored && lyrics.is_censored(),
        }
    }

    pub(super) fn ceiling(&self, kinds: &[LyricsKind]) -> Option<Rank> {
        let primary = match &self.basis {
            Basis::TypePriority(priority) => best_rank_for(kinds, priority),
            Basis::SyncLevel(wanted) => kinds
                .iter()
                .filter(|kind| wanted.contains(kind))
                .map(|kind| kind.max_sync_level().rank())
                .min(),
        }?;

        Some(Rank {
            primary,
            censored: false,
        })
    }

    pub(super) fn describe(&self, rank: Rank) -> String {
        let primary = match &self.basis {
            Basis::TypePriority(priority) => priority.get(rank.primary).map(|kind| kind.slug()),
            Basis::SyncLevel(_) => SyncLevel::from_rank(rank.primary).map(|level| level.slug()),
        };

        match (primary, rank.censored) {
            (None, _) => "none".to_string(),
            (Some(name), true) => format!("censored {name}"),
            (Some(name), false) => name.to_string(),
        }
    }
}

fn kind_rank(kind: LyricsKind, priority: &[LyricsKind]) -> usize {
    priority
        .iter()
        .position(|&k| k == kind)
        .unwrap_or(usize::MAX)
}

fn best_rank_for(kinds: &[LyricsKind], priority: &[LyricsKind]) -> Option<usize> {
    kinds
        .iter()
        .map(|&kind| kind_rank(kind, priority))
        .filter(|&rank| rank != usize::MAX)
        .min()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct ConfigFixture {
        cfg: PluginConfig,
    }

    fn sync(enabled: &str) -> ConfigFixture {
        fixture(ProviderMode::BestSyncLevel, enabled)
    }

    fn type_priority(enabled: &str) -> ConfigFixture {
        fixture(ProviderMode::TypePriority, enabled)
    }

    fn fixture(mode: ProviderMode, enabled: &str) -> ConfigFixture {
        ConfigFixture {
            cfg: PluginConfig {
                provider_mode: mode,
                lyrics_type_priority: kinds(enabled),
                ..PluginConfig::default()
            },
        }
    }

    fn kinds(slugs: &str) -> Vec<LyricsKind> {
        slugs
            .split(",")
            .map(|slug| {
                LyricsKind::from_slug(slug).unwrap_or_else(|| panic!("unknown lyrics type {slug}"))
            })
            .collect()
    }

    fn clean_lrc() -> Lyrics {
        Lyrics::Lrc("[00:01.00]Be humble hol' up".to_string())
    }

    fn censored_lrc() -> Lyrics {
        Lyrics::Lrc("[00:01.00]B**ch be humble hol' up".to_string())
    }

    impl ConfigFixture {
        fn prefer_uncensored(mut self) -> ConfigFixture {
            self.cfg.prefer_uncensored = true;
            self
        }

        fn ranker(&self) -> Ranker<'_> {
            Ranker::for_mode(&self.cfg).expect("mode should be ranked")
        }

        #[track_caller]
        fn check_rank(&self, lyrics: Lyrics, expected: &str) {
            let ranker = self.ranker();
            assert_eq!(ranker.describe(ranker.rank(&lyrics)), expected);
        }

        #[track_caller]
        fn check_ceiling(&self, offered: &str, expected: &str) {
            let ranker = self.ranker();
            let ceiling = ranker.ceiling(&kinds(offered));
            let described = ceiling.map_or_else(|| "nothing".to_string(), |it| ranker.describe(it));
            assert_eq!(described, expected, "ceiling for {offered}");
        }

        #[track_caller]
        fn check_best(&self, candidates: &[Lyrics], expected: &str) {
            let ranker = self.ranker();
            let best = candidates
                .iter()
                .min_by_key(|lyrics| ranker.rank(lyrics))
                .expect("no candidates");
            assert_eq!(best.text(&self.cfg), expected);
        }

        #[track_caller]
        fn check_worth_trying(&self, best: Lyrics, offered: &str, expected: bool) {
            let ranker = self.ranker();
            let current = ranker.rank(&best);
            let worth = ranker
                .ceiling(&kinds(offered))
                .is_none_or(|ceiling| ceiling < current);
            assert_eq!(worth, expected, "{offered} against {best:?}");
        }

        #[track_caller]
        fn check_search_ends(&self, lyrics: Lyrics, expected: bool) {
            assert_eq!(self.ranker().rank(&lyrics) == Rank::BEST, expected);
        }

        #[track_caller]
        fn check_reports_censored(&self, lyrics: Lyrics, expected: bool) {
            assert_eq!(self.ranker().rank(&lyrics).is_censored(), expected);
        }
    }

    #[test]
    fn type_mode_follows_the_configured_order() {
        let cfg = type_priority("ttml,elrc,lrc,plain");

        cfg.check_rank(Lyrics::Ttml(String::new()), "ttml");
        cfg.check_rank(Lyrics::Lrc(String::new()), "lrc");
        cfg.check_rank(Lyrics::Plain(String::new()), "plain");
        cfg.check_best(
            &[
                Lyrics::Plain("plain".to_string()),
                Lyrics::Elrc("elrc".to_string()),
            ],
            "elrc",
        );
    }

    #[test]
    fn type_mode_ranks_disabled_types_below_everything() {
        let cfg = type_priority("lrc,plain");

        cfg.check_rank(Lyrics::Srt(String::new()), "none");
        cfg.check_best(
            &[
                Lyrics::Srt("srt".to_string()),
                Lyrics::Plain("plain".to_string()),
            ],
            "plain",
        );
    }

    #[test]
    fn sync_mode_prioritizes_sync_level() {
        let cfg = sync("lrc,elrc,plain");

        cfg.check_rank(Lyrics::Elrc(String::new()), "word-by-word");
        cfg.check_rank(Lyrics::Lrc(String::new()), "line-by-line");
        cfg.check_rank(Lyrics::Plain(String::new()), "plain");
        cfg.check_best(
            &[
                Lyrics::Lrc("lrc".to_string()),
                Lyrics::Elrc("elrc".to_string()),
            ],
            "elrc",
        );
    }

    #[test]
    fn sync_mode_reads_the_sync_level_of_lyrics() {
        let cfg = sync("ttml");

        cfg.check_rank(
            Lyrics::Ttml(r#"<tt itunes:timing="Word"></tt>"#.to_string()),
            "word-by-word",
        );
        cfg.check_rank(
            Lyrics::Ttml(r#"<tt itunes:timing="None"></tt>"#.to_string()),
            "plain",
        );
    }

    #[test]
    fn sync_mode_ceiling_is_the_finest_type_a_provider_serves() {
        let cfg = sync("ttml,elrc,lrc,plain");

        cfg.check_ceiling("ttml", "word-by-word");
        cfg.check_ceiling("lrc,plain", "line-by-line");
        cfg.check_ceiling("plain", "plain");
    }

    #[test]
    fn type_mode_ceiling_is_the_best_type_a_provider_serves() {
        let cfg = type_priority("ttml,elrc,lrc,plain");

        cfg.check_ceiling("lrc,elrc", "elrc");
        cfg.check_ceiling("plain", "plain");
    }

    #[test]
    fn a_ceiling_only_counts_enabled_types() {
        sync("lrc,plain").check_ceiling("elrc,lrc", "line-by-line");
        sync("lrc,plain").check_ceiling("srt", "nothing");
        type_priority("lrc,plain").check_ceiling("srt,lrc", "lrc");
        type_priority("lrc,plain").check_ceiling("srt", "nothing");
    }

    #[test]
    fn censoring_is_ignored_unless_it_is_asked_for() {
        sync("lrc").check_rank(censored_lrc(), "line-by-line");
        sync("lrc").check_reports_censored(censored_lrc(), false);

        sync("lrc")
            .prefer_uncensored()
            .check_rank(censored_lrc(), "censored line-by-line");
        sync("lrc")
            .prefer_uncensored()
            .check_reports_censored(censored_lrc(), true);
        sync("lrc")
            .prefer_uncensored()
            .check_reports_censored(clean_lrc(), false);
    }

    #[test]
    fn censoring_only_breaks_ties_in_the_same_level() {
        let cfg = sync("elrc,lrc").prefer_uncensored();

        cfg.check_best(
            &[
                Lyrics::Lrc("clean,lrc".to_string()),
                Lyrics::Elrc("f**k".to_string()),
            ],
            "f**k",
        );
        cfg.check_best(
            &[
                Lyrics::Lrc("b**ch".to_string()),
                Lyrics::Lrc("clean".to_string()),
            ],
            "clean",
        );
    }

    #[test]
    fn a_censored_hit_allows_checking_another_provider() {
        let cfg = sync("lrc").prefer_uncensored();

        cfg.check_worth_trying(censored_lrc(), "lrc", true);
        cfg.check_worth_trying(clean_lrc(), "lrc", false);
    }

    #[test]
    fn a_provider_that_cannot_improve_is_not_worth_trying() {
        let cfg = sync("elrc,lrc,plain");

        cfg.check_worth_trying(clean_lrc(), "plain", false);
        cfg.check_worth_trying(clean_lrc(), "elrc", true);
    }

    #[test]
    fn a_clean_word_synced_hit_ends_the_search() {
        let cfg = sync("elrc,lrc").prefer_uncensored();

        cfg.check_search_ends(Lyrics::Elrc("clean".to_string()), true);
        cfg.check_search_ends(Lyrics::Elrc("f**k".to_string()), false);
        cfg.check_search_ends(clean_lrc(), false);
    }

    #[test]
    fn the_top_of_the_priority_list_ends_the_search() {
        let cfg = type_priority("ttml,lrc");

        cfg.check_search_ends(Lyrics::Ttml("clean".to_string()), true);
        cfg.check_search_ends(Lyrics::Lrc("clean".to_string()), false);
    }
}
