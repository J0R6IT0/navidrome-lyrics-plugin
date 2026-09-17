//! QQ Music provider, modelled on the QQ Music Lite Android client as
//! implemented by https://github.com/chenmozhijin/LDDC (LDDC/core/api/lyrics/qm.py).

use crate::{
    config::{PluginConfig, ProviderParams},
    ext::TrackInfoExt,
    format::lrc,
    providers::{LyricsProvider, ProviderResult, error::ProviderError, http::Http},
    types::{Lyrics, LyricsKind},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use extism_pdk::{info, warn};
use nd_pdk::lyrics::TrackInfo;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::time::Duration;

mod qrc;

const ENDPOINT: &str = "https://u.y.qq.com/cgi-bin/musicu.fcg";
const CLIENT_USER_AGENT: &str = "okhttp/3.14.9";

const SEARCH_MODULE: &str = "music.search.SearchCgiService";
const SEARCH_METHOD: &str = "DoSearchForQQMusicLite";
const LYRIC_MODULE: &str = "music.musichallSong.PlayLyricInfo";
const LYRIC_METHOD: &str = "GetPlayLyricInfo";

/// Client identity block sent with every request. `ct` 11 plus the
/// `qqmusiclight` app id is what marks this as the Android Lite client.
#[derive(Debug, Serialize)]
struct Common {
    ct: u32,
    cv: &'static str,
    v: &'static str,
    os_ver: &'static str,
    phonetype: &'static str,
    rom: &'static str,
    #[serde(rename = "tmeAppID")]
    tme_app_id: &'static str,
    nettype: &'static str,
    udid: &'static str,
}

const COMMON: Common = Common {
    ct: 11,
    cv: "1003006",
    v: "1003006",
    os_ver: "15",
    phonetype: "24122RKC7C",
    rom: "Redmi/miro/miro:15/AE3A.240806.005/OS2.0.105.0.VOMCNXM:user/release-keys",
    tme_app_id: "qqmusiclight",
    nettype: "NETWORK_WIFI",
    udid: "0",
};

#[derive(Debug, Serialize)]
struct SearchParam<'a> {
    search_id: String,
    remoteplace: &'static str,
    query: &'a str,
    search_type: u32,
    num_per_page: u32,
    page_num: u32,
    highlight: u32,
    nqc_flag: u32,
    page_id: u32,
    grp: u32,
}

impl<'a> SearchParam<'a> {
    fn new(query: &'a str) -> Self {
        Self {
            search_id: random_search_id().to_string(),
            remoteplace: "search.android.keyboard",
            query,
            search_type: 0,
            num_per_page: 5,
            page_num: 1,
            highlight: 0,
            nqc_flag: 0,
            page_id: 1,
            grp: 1,
        }
    }
}

/// The lyric endpoint identifies tracks by numeric song ID. The
/// base64-encoded title, artist, album, and duration mirror the values sent by
/// the official client, but the server does not appear to validate them.
#[derive(Debug, Serialize)]
struct LyricParam {
    #[serde(rename = "albumName")]
    album_name: String,
    crypt: u32,
    ct: u32,
    cv: u32,
    interval: u64,
    lrc_t: u32,
    qrc: u32,
    qrc_t: u32,
    roma: u32,
    roma_t: u32,
    #[serde(rename = "singerName")]
    singer_name: String,
    #[serde(rename = "songID")]
    song_id: u64,
    #[serde(rename = "songName")]
    song_name: String,
    trans: u32,
    trans_t: u32,
    #[serde(rename = "type")]
    type_: u32,
}

impl LyricParam {
    fn new(song: &Song) -> Self {
        Self {
            album_name: STANDARD.encode(&song.album.name),
            crypt: 1,
            ct: 19,
            cv: 2111,
            interval: song.interval,
            lrc_t: 0,
            qrc: 1,
            qrc_t: 0,
            roma: 0,
            roma_t: 0,
            singer_name: STANDARD.encode(song.singers()),
            song_id: song.id,
            song_name: STANDARD.encode(&song.title),
            trans: 0,
            trans_t: 0,
            type_: 0,
        }
    }
}

#[derive(Debug, Deserialize)]
struct ApiResponse<T> {
    code: Option<i64>,
    request: Option<ModuleResponse<T>>,
}

#[derive(Debug, Deserialize)]
struct ModuleResponse<T> {
    code: Option<i64>,
    data: Option<T>,
}

#[derive(Debug, Deserialize)]
struct SearchData {
    #[serde(default)]
    body: SearchBody,
}

#[derive(Debug, Default, Deserialize)]
struct SearchBody {
    #[serde(default)]
    item_song: Vec<Song>,
}

#[derive(Debug, Deserialize)]
struct Song {
    id: u64,
    #[serde(default)]
    title: String,
    #[serde(default)]
    singer: Vec<Singer>,
    #[serde(default)]
    album: Album,
    /// Track length in seconds.
    #[serde(default)]
    interval: u64,
}

impl Song {
    /// The lyric endpoint expects every singer joined by a slash.
    fn singers(&self) -> String {
        self.singer
            .iter()
            .map(|singer| singer.name.as_str())
            .filter(|name| !name.is_empty())
            .collect::<Vec<_>>()
            .join("/")
    }
}

#[derive(Debug, Deserialize)]
struct Singer {
    #[serde(default)]
    name: String,
}

#[derive(Debug, Default, Deserialize)]
struct Album {
    #[serde(default)]
    name: String,
}

#[derive(Debug, Deserialize)]
struct LyricData {
    #[serde(default)]
    lyric: String,
}

pub struct QQMusic;

impl QQMusic {
    pub fn create(_params: &ProviderParams) -> Box<dyn LyricsProvider> {
        Box::new(Self)
    }

    fn search(&self, track: &TrackInfo) -> ProviderResult<SearchData> {
        let query = format!(
            "{} {}",
            track.title,
            track.first_artist().unwrap_or_default()
        );

        self.request(SEARCH_MODULE, SEARCH_METHOD, &SearchParam::new(&query))
    }

    fn get(&self, song: &Song) -> ProviderResult<Option<String>> {
        let data: LyricData = self.request(LYRIC_MODULE, LYRIC_METHOD, &LyricParam::new(song))?;

        if data.lyric.is_empty() {
            info!("qqmusic: song {} has no lyrics attached", song.id);
            return Ok(None);
        }

        let xml = match qrc::decrypt(&data.lyric) {
            Ok(xml) => xml,
            Err(e) => {
                warn!(
                    "qqmusic: failed to decrypt the lyrics for song {} ({} hex chars): {e}",
                    song.id,
                    data.lyric.len()
                );
                return Ok(None);
            }
        };

        let content = qrc::extract_lyric_content(&xml);

        if content.is_none() {
            warn!(
                "qqmusic: the decrypted payload for song {} carries no LyricContent",
                song.id
            );
        }

        Ok(content)
    }

    fn request<P: Serialize, T: DeserializeOwned>(
        &self,
        module: &str,
        method: &str,
        param: &P,
    ) -> ProviderResult<T> {
        let body = serde_json::to_vec(&serde_json::json!({
            "comm": COMMON,
            "request": {
                "module": module,
                "method": method,
                "param": param,
            },
        }))
        .map_err(|e| ProviderError::other(format!("failed to encode the request: {e}")))?;

        let response = Http::post(ENDPOINT)
            .header("Content-Type", "application/json")
            .header("Cookie", "tmeLoginType=-1;")
            .header("User-Agent", CLIENT_USER_AGENT)
            .body(body)
            .send()?;

        if response.status != 200 {
            return Err(response.unexpected_status("the API"));
        }

        let parsed: ApiResponse<T> = response.json("API")?;
        check_code(parsed.code, module, method, "gateway")?;

        let result = parsed
            .request
            .ok_or_else(|| missing(module, method, "'request' object"))?;
        check_code(result.code, module, method, "module")?;

        result
            .data
            .ok_or_else(|| missing(module, method, "'data' object"))
    }
}

impl LyricsProvider for QQMusic {
    fn supported_kinds(&self) -> &'static [LyricsKind] {
        &[LyricsKind::Lrc, LyricsKind::Elrc]
    }

    fn fetch_lyrics(
        &self,
        track: &TrackInfo,
        cfg: &PluginConfig,
    ) -> ProviderResult<Option<Lyrics>> {
        if !track.has_artist() {
            return Err(ProviderError::other("track has no artist"));
        }

        let song = match self
            .search(track)?
            .body
            .item_song
            .into_iter()
            .filter(|song| {
                track.matches_duration(Duration::from_secs(song.interval), cfg.duration_tolerance)
            })
            .find(|song| !cfg.require_album_match || track.matches_album(&song.album.name))
        {
            Some(song) => song,
            None => return Ok(None),
        };

        let content = match self.get(&song)? {
            Some(content) => content,
            None => return Ok(None),
        };

        let is_qrc = qrc::is_qrc(&content);

        if !is_qrc && lrc::is_instrumental(&content) {
            info!("qqmusic: track is instrumental");
            return Ok(Some(Lyrics::Instrumental));
        }

        for &kind in &cfg.lyrics_type_priority {
            match kind {
                LyricsKind::Elrc if is_qrc => {
                    let elrc = qrc::to_enhanced_lrc(&content);

                    if !elrc.trim().is_empty() {
                        return Ok(Some(Lyrics::Elrc(elrc)));
                    }
                }

                LyricsKind::Lrc => {
                    let lrc = if is_qrc {
                        qrc::to_lrc(&content)
                    } else {
                        content.clone()
                    };

                    if !lrc.trim().is_empty() {
                        return Ok(Some(Lyrics::Lrc(lrc)));
                    }
                }

                _ => {}
            }
        }

        Ok(None)
    }
}

fn check_code(code: Option<i64>, module: &str, method: &str, level: &str) -> ProviderResult<()> {
    match code {
        Some(0) => Ok(()),
        // 2001 means the endpoint rejected our client identity, either the `comm` block
        // or the headers no longer match the client it expects.
        Some(2001) => Err(ProviderError::other(format!(
            "{module}.{method} rejected the client identity ({level} error code 2001), the 'comm' block or request headers are stale"
        ))),
        Some(code) => Err(ProviderError::other(format!(
            "{module}.{method} returned {level} error code {code}"
        ))),
        None => Err(missing(module, method, &format!("{level} 'code' field"))),
    }
}

fn missing(module: &str, method: &str, what: &str) -> ProviderError {
    ProviderError::other(format!("{module}.{method} response is missing the {what}"))
}

fn random_search_id() -> u64 {
    let mut bytes = [0u8; 8];
    let rnd = if getrandom::getrandom(&mut bytes).is_ok() {
        u64::from_le_bytes(bytes)
    } else {
        0x1234_5678_9abc_def0
    };

    let prefix = (rnd % 20) + 1;
    let middle = (rnd >> 8) % 4_194_304;
    let low = (rnd >> 32) % 86_400_000;

    prefix * 18_014_398_509_481_984 + middle * 4_294_967_296 + low
}

#[cfg(test)]
mod tests {
    use super::*;

    fn song() -> Song {
        serde_json::from_value(serde_json::json!({
            "id": 7239095,
            "mid": "004D8vna0lUZbr",
            "title": "Bohemian Rhapsody",
            "singer": [{"name": "Queen"}, {"name": "David Bowie"}, {"name": ""}],
            "album": {"name": "A Night At The Opera"},
            "interval": 354,
        }))
        .unwrap()
    }

    #[track_caller]
    fn check_code_error(code: Option<i64>, expected: &str) {
        let error = check_code(code, SEARCH_MODULE, SEARCH_METHOD, "gateway").unwrap_err();

        assert!(
            error.to_string().contains(expected),
            "{error} does not contain {expected:?}"
        );
    }

    #[test]
    fn singers_are_joined_and_blank_names_skipped() {
        assert_eq!(song().singers(), "Queen/David Bowie");
    }

    #[test]
    fn a_song_deserializes_without_its_optional_fields() {
        let song: Song = serde_json::from_value(serde_json::json!({ "id": 7239095 })).unwrap();

        assert_eq!(song.title, "");
        assert_eq!(song.singers(), "");
        assert_eq!(song.album.name, "");
        assert_eq!(song.interval, 0);
    }

    #[test]
    fn lyric_param_base64_encodes_the_names() {
        let json = serde_json::to_string(&LyricParam::new(&song())).unwrap();

        assert!(json.contains(r#""songName":"Qm9oZW1pYW4gUmhhcHNvZHk=""#));
        assert!(json.contains(r#""singerName":"UXVlZW4vRGF2aWQgQm93aWU=""#));
        assert!(json.contains(r#""albumName":"QSBOaWdodCBBdCBUaGUgT3BlcmE=""#));
        assert!(json.contains(r#""songID":7239095"#));
        assert!(json.contains(r#""interval":354"#));
        assert!(json.contains(r#""type":0"#));
    }

    #[test]
    fn an_api_response_carries_the_module_data() {
        let response: ApiResponse<LyricData> = serde_json::from_value(serde_json::json!({
            "code": 0,
            "request": { "code": 0, "data": { "lyric": "cafe" } },
        }))
        .unwrap();

        let result = response.request.unwrap();

        assert_eq!(response.code, Some(0));
        assert_eq!(result.code, Some(0));
        assert_eq!(result.data.unwrap().lyric, "cafe");
    }

    #[test]
    fn a_zero_code_is_accepted() {
        assert!(check_code(Some(0), SEARCH_MODULE, SEARCH_METHOD, "gateway").is_ok());
    }

    #[test]
    fn a_non_zero_code_is_rejected() {
        check_code_error(Some(2001), "rejected the client identity");
        check_code_error(Some(1000), "returned gateway error code 1000");
        check_code_error(None, "missing the gateway 'code' field");
    }
}
