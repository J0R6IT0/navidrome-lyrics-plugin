use crate::{
    config::{PluginConfig, ProviderParams},
    ext::TrackInfoExt,
    format::lrc,
    providers::{LyricsProvider, ProviderResult, error::ProviderError, http::Http},
    types::{Lyrics, LyricsKind},
};
use nd_pdk::lyrics::TrackInfo;
use regex::Regex;
use std::{collections::HashMap, time::Duration};

const BASE_URL: &str = "https://www.genie.co.kr";
const LYRICS_URL: &str = "https://dn.genie.co.kr/app/purchase/get_msl.asp";

const MAX_PROBE: usize = 5;

struct Candidate {
    id: String,
    album: String,
}

pub struct Genie;

impl Genie {
    pub fn create(_params: &ProviderParams) -> Box<dyn LyricsProvider> {
        Box::new(Self)
    }

    fn search(&self, query: &str) -> ProviderResult<Vec<Candidate>> {
        let response = Http::get(format!("{BASE_URL}/search/searchMain"))
            .browser()
            .param("query", query)
            .send()?;

        match response.status {
            200 => parse_candidates(&response.text()),
            429 => Err(response.rate_limited()),
            _ => Err(response.unexpected_status("the search")),
        }
    }

    fn duration(&self, id: &str) -> ProviderResult<Option<Duration>> {
        let response = Http::get(format!("{BASE_URL}/detail/songInfo"))
            .browser()
            .param("xgnm", id)
            .send()?;

        match response.status {
            200 => Ok(parse_duration(&response.text())),
            429 => Err(response.rate_limited()),
            _ => Err(response.unexpected_status("the song detail page")),
        }
    }

    fn lyrics(&self, id: &str) -> ProviderResult<Option<String>> {
        let response = Http::get(LYRICS_URL)
            .browser()
            .param("path", "a")
            .param("songid", id)
            .send()?;

        match response.status {
            200 => Ok(parse_lyrics(&response.text())),
            429 => Err(response.rate_limited()),
            500 => Ok(None),
            _ => Err(response.unexpected_status("the lyrics endpoint")),
        }
    }
}

impl LyricsProvider for Genie {
    fn supported_kinds(&self) -> &'static [LyricsKind] {
        &[LyricsKind::Lrc]
    }

    fn fetch_lyrics(
        &self,
        track: &TrackInfo,
        cfg: &PluginConfig,
    ) -> ProviderResult<Option<Lyrics>> {
        if !track.has_artist() {
            return Err(ProviderError::other("track has no artist"));
        }

        let title = track.clean_title();
        if title.is_empty() {
            return Ok(None);
        }

        let artist = track.first_artist().unwrap_or_default();
        let query = format!("{title} {artist}");

        for candidate in self.search(&query)?.into_iter().take(MAX_PROBE) {
            if cfg.require_album_match && !track.matches_album(&candidate.album) {
                continue;
            }

            let Some(duration) = self.duration(&candidate.id)? else {
                continue;
            };
            if !track.matches_duration(duration, cfg.duration_tolerance) {
                continue;
            }

            let Some(lrc) = self.lyrics(&candidate.id)? else {
                continue;
            };
            if !lrc.trim().is_empty() {
                return Ok(Some(Lyrics::Lrc(lrc)));
            }
        }

        Ok(None)
    }
}

fn parse_candidates(body: &str) -> ProviderResult<Vec<Candidate>> {
    let row_re = Regex::new(r#"(?s)<tr class="list"\s*songid="(\d+)">(.*?)</tr>"#)
        .map_err(|e| ProviderError::other(format!("invalid row regex: {e}")))?;
    let album_re = Regex::new(r#"class="albumtitle ellipsis"[^>]*>\s*([^<]+?)\s*<"#)
        .map_err(|e| ProviderError::other(format!("invalid album regex: {e}")))?;

    Ok(row_re
        .captures_iter(body)
        .filter_map(|cap| {
            let id = cap.get(1)?.as_str().to_string();
            let album = album_re
                .captures(cap.get(2)?.as_str())
                .and_then(|c| c.get(1))
                .map(|m| m.as_str().to_string())
                .unwrap_or_default();

            Some(Candidate { id, album })
        })
        .collect())
}

fn parse_duration(body: &str) -> Option<Duration> {
    let re = Regex::new(r#"alt="재생시간"\s*/></span>\s*<span class="value">(\d{1,2}):(\d{2})<"#)
        .ok()?;
    let caps = re.captures(body)?;

    let minutes: u64 = caps.get(1)?.as_str().parse().ok()?;
    let seconds: u64 = caps.get(2)?.as_str().parse().ok()?;

    Some(Duration::from_secs(minutes * 60 + seconds))
}

fn parse_lyrics(body: &str) -> Option<String> {
    let start = body.find('{')?;
    let end = body.rfind('}')?;

    let lines: HashMap<String, String> = serde_json::from_str(&body[start..=end]).ok()?;
    if lines.is_empty() {
        return None;
    }

    let mut entries: Vec<(i64, String)> = lines
        .into_iter()
        .filter_map(|(ms, text)| ms.parse::<i64>().ok().map(|ms| (ms, text)))
        .collect();
    entries.sort_by_key(|(ms, _)| *ms);

    Some(
        entries
            .into_iter()
            .map(|(ms, text)| format!("[{}] {}", lrc::format_timestamp(ms), text))
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[track_caller]
    fn check_candidates(body: &str, expected: &[&str]) {
        let ids: Vec<String> = parse_candidates(body)
            .unwrap()
            .into_iter()
            .map(|c| c.id)
            .collect();
        assert_eq!(ids, expected);
    }

    #[track_caller]
    fn check_albums(body: &str, expected: &[&str]) {
        let albums: Vec<String> = parse_candidates(body)
            .unwrap()
            .into_iter()
            .map(|c| c.album)
            .collect();
        assert_eq!(albums, expected);
    }

    #[track_caller]
    fn check_duration(body: &str, expected: Option<Duration>) {
        assert_eq!(parse_duration(body), expected);
    }

    #[track_caller]
    fn check_lyrics(body: &str, expected: Option<&str>) {
        assert_eq!(parse_lyrics(body), expected.map(str::to_string));
    }

    #[test]
    fn candidates_are_parsed_from_search_rows() {
        check_candidates(
            r#"
                <tr class="list"  songid="99570005">...</tr>
                <tr class="list"  songid="12345678">...</tr>
            "#,
            &["99570005", "12345678"],
        );
        check_candidates("<html></html>", &[]);
    }

    #[test]
    fn the_album_title_is_parsed_from_each_row() {
        check_albums(
            r##"
                <tr class="list"  songid="16480934">
                    <a href="#" class="artist ellipsis" onclick="fnViewArtist('14947246');return false;">
                        Bruce Springsteen
                    </a>
                    <i class="bar">|</i>
                    <a href="#" class="albumtitle ellipsis" onclick="fnViewAlbumLayer('15050884');return false;">


                        Born To Run

                    </a>
                </tr>
            "##,
            &["Born To Run"],
        );
    }

    #[test]
    fn a_row_without_an_album_title_gets_an_empty_string() {
        check_albums(r#"<tr class="list"  songid="99570005">...</tr>"#, &[""]);
    }

    #[test]
    fn duration_is_parsed_from_the_detail_page() {
        check_duration(
            r#"<li><span class="attr"><img src="//x/txt_8.png" alt="재생시간" /></span> <span class="value">03:06</span></li>"#,
            Some(Duration::from_secs(186)),
        );
        check_duration("<li>no duration here</li>", None);
    }

    #[test]
    fn lyrics_are_parsed_from_the_jsonp_wrapper() {
        check_lyrics(
            r#"null({"9020":"second line","1040":"first line"});"#,
            Some("[00:01.04] first line\n[00:09.02] second line"),
        );
        check_lyrics("null({});", None);
        check_lyrics("<html><body>An error occurred.</body></html>", None);
        // Instrumental/karaoke tracks answer HTTP 200 with this.
        check_lyrics("NOT FOUND LYRICS", None);
    }
}
