use super::host::get_string;
use crate::config::Result;
use extism_pdk::warn;
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};

#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct ProviderParams(BTreeMap<String, String>);

impl ProviderParams {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).map(String::as_str)
    }

    pub fn flag(&self, key: &str) -> bool {
        self.get(key).is_some_and(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "true" | "1" | "yes"
            )
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProviderMode {
    #[default]
    Priority,
    Rotation,
    TypePriority,
    BestSyncLevel,
}

impl ProviderMode {
    pub fn slug(&self) -> &'static str {
        match self {
            ProviderMode::Priority => "priority",
            ProviderMode::Rotation => "rotation",
            ProviderMode::TypePriority => "type",
            ProviderMode::BestSyncLevel => "sync",
        }
    }

    pub fn from_slug(slug: &str) -> Option<ProviderMode> {
        match slug.trim().to_ascii_lowercase().as_str() {
            "priority" => Some(ProviderMode::Priority),
            "rotation" => Some(ProviderMode::Rotation),
            "type" => Some(ProviderMode::TypePriority),
            "sync" => Some(ProviderMode::BestSyncLevel),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ProviderEntry {
    pub name: String,
    pub params: ProviderParams,
}

impl ProviderEntry {
    pub fn id(&self) -> String {
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        let mut mix = |bytes: &[u8]| {
            for byte in bytes {
                hash ^= *byte as u64;
                hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
            }
        };

        mix(self.name.as_bytes());
        for (k, v) in &self.params.0 {
            mix(b"\0");
            mix(k.as_bytes());
            mix(b"=");
            mix(v.as_bytes());
        }

        format!("{hash:016x}")
    }
}

pub(super) fn resolve_list() -> Result<Vec<ProviderEntry>> {
    let Some(raw) = get_string("providersList")? else {
        warn!("no providers configured, no lyrics will be fetched");
        return Ok(Vec::new());
    };

    match parse_providers(&raw) {
        Some(providers) if !providers.is_empty() => Ok(providers),
        Some(_) => {
            warn!("providersList has no usable entries, no lyrics will be fetched");
            Ok(Vec::new())
        }
        None => {
            warn!("providersList is not valid JSON, no lyrics will be fetched");
            Ok(Vec::new())
        }
    }
}

pub(super) fn resolve_mode() -> Result<ProviderMode> {
    let Some(raw) = get_string("providerMode")? else {
        return Ok(ProviderMode::default());
    };

    match ProviderMode::from_slug(&raw) {
        Some(mode) => Ok(mode),
        None => {
            let fallback = ProviderMode::default();
            warn!("unknown providerMode '{raw}', using '{}'", fallback.slug());
            Ok(fallback)
        }
    }
}

fn parse_providers(raw: &str) -> Option<Vec<ProviderEntry>> {
    let rows: Vec<BTreeMap<String, Value>> = serde_json::from_str(raw).ok()?;

    let mut seen = HashSet::new();
    Some(
        rows.into_iter()
            .filter_map(parse_provider_row)
            .filter(|entry| seen.insert(entry.clone()))
            .collect(),
    )
}

fn parse_provider_row(mut row: BTreeMap<String, Value>) -> Option<ProviderEntry> {
    let name = row
        .remove("provider")
        .as_ref()
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())?
        .to_string();

    let params = row
        .into_iter()
        .filter_map(|(key, value)| {
            let value = match value {
                Value::String(s) => s.trim().to_string(),
                Value::Bool(b) => b.to_string(),
                Value::Number(n) => n.to_string(),
                Value::Array(items) => items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
                    .join(","),
                _ => return None,
            };
            (!value.is_empty()).then_some((key, value))
        })
        .collect();

    Some(ProviderEntry {
        name,
        params: ProviderParams(params),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, params: &[(&str, &str)]) -> ProviderEntry {
        ProviderEntry {
            name: name.to_string(),
            params: ProviderParams(
                params
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
            ),
        }
    }

    #[track_caller]
    fn check_providers(raw: &str, expected: Option<Vec<ProviderEntry>>) {
        assert_eq!(parse_providers(raw), expected, "providers from {raw}");
    }

    #[test]
    fn providers_that_differ_have_different_ids() {
        let entries = [
            entry("lrclib", &[]),
            entry("kugou", &[]),
            entry("lrclib", &[("baseUrl", "http://a")]),
            entry("lrclib", &[("baseUrl", "http://b")]),
            entry("lrclib", &[("baseUrl", "http://a"), ("timeout", "30")]),
        ];
        let ids: Vec<String> = entries.iter().map(ProviderEntry::id).collect();
        let unique: HashSet<&String> = ids.iter().collect();

        assert_eq!(unique.len(), ids.len(), "cache ids collided: {ids:?}");
    }

    #[test]
    fn providers_are_parsed() {
        check_providers(
            r#"[{"provider":"lrclib"},{"provider":"lyrics.ovh"}]"#,
            Some(vec![entry("lrclib", &[]), entry("lyrics.ovh", &[])]),
        );
    }

    #[test]
    fn provider_parameters_are_parsed() {
        check_providers(
            r#"[{"provider":"applemusic","mediaUserToken":" abc ","storefront":"gb"}]"#,
            Some(vec![entry(
                "applemusic",
                &[("mediaUserToken", "abc"), ("storefront", "gb")],
            )]),
        );
    }

    #[test]
    fn array_params_are_joined_with_commas() {
        check_providers(
            r#"[{"provider":"lrcmux","sources":["lrclib"," kugou ",""]}]"#,
            Some(vec![entry("lrcmux", &[("sources", "lrclib,kugou")])]),
        );
    }

    #[test]
    fn blank_params_are_ignored() {
        check_providers(
            r#"[{"provider":"lrclib","baseUrl":"   ","storefront":"","sources":[]}]"#,
            Some(vec![entry("lrclib", &[])]),
        );
    }

    #[test]
    fn a_provider_without_a_name_is_skipped() {
        check_providers(
            r#"[{"provider":""},{"provider":"  "},{"baseUrl":"http://a"},{"provider":"kugou"}]"#,
            Some(vec![entry("kugou", &[])]),
        );
    }

    #[test]
    fn two_equal_providers_are_deduplicated() {
        check_providers(
            r#"[{"provider":"kugou"},{"provider":"kugou"},{"provider":"netease"}]"#,
            Some(vec![entry("kugou", &[]), entry("netease", &[])]),
        );
    }

    #[test]
    fn two_providers_with_different_params_are_kept() {
        check_providers(
            r#"[{"provider":"lrclib","baseUrl":"http://a"},{"provider":"lrclib","baseUrl":"http://b"}]"#,
            Some(vec![
                entry("lrclib", &[("baseUrl", "http://a")]),
                entry("lrclib", &[("baseUrl", "http://b")]),
            ]),
        );
    }

    #[test]
    fn invalid_provider_list_json_is_rejected() {
        for raw in ["not json", "", "{}", r#"{"provider":"lrclib"}"#, "[1,2]"] {
            check_providers(raw, None);
        }
    }

    #[test]
    fn an_empty_provider_list_is_valid() {
        check_providers("[]", Some(Vec::new()));
    }

    #[test]
    fn boolean_parameters_are_read_as_flags() {
        let params = entry(
            "netease",
            &[
                ("mergeTranslation", "true"),
                ("other", "1"),
                ("third", "YES"),
                ("off", "false"),
            ],
        )
        .params;

        assert!(params.flag("mergeTranslation"));
        assert!(params.flag("other"));
        assert!(params.flag("third"));
        assert!(!params.flag("off"));
        assert!(!params.flag("missing"));
    }
}
