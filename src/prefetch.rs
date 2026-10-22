use extism_pdk::{Error, debug, info, warn};
use nd_pdk::host::{cache, subsonicapi, task};
use serde::{Deserialize, Serialize, de::IgnoredAny};

const QUEUE_NAME: &str = "prefetch";
const QUEUE_RETENTION_MS: i64 = 3_600_000;

const DEDUPE_KEY_PREFIX: &str = "prefetch:";
const DEDUPE_TTL_SECS: i64 = 6 * 3600;

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Job {
    username: String,
    track_id: String,
    label: String,
}

pub fn create_queue() -> Result<(), Error> {
    task::create_queue(
        QUEUE_NAME,
        task::QueueConfig {
            concurrency: 1,
            max_retries: 0,
            retention_ms: QUEUE_RETENTION_MS,
            ..Default::default()
        },
    )
}

pub fn enqueue(username: &str, track_id: &str, label: String) -> Result<(), Error> {
    let key = dedupe_key(track_id);
    if cache::has(&key).unwrap_or(false) {
        debug!("'{label}' was prefetched recently, skipping");
        return Ok(());
    }

    let job = Job {
        username: username.to_string(),
        track_id: track_id.to_string(),
        label,
    };
    task::enqueue(QUEUE_NAME, serde_json::to_vec(&job)?)?;
    debug!("queued lyrics prefetch for '{}'", job.label);

    if let Err(err) = cache::set_int(&key, 1, DEDUPE_TTL_SECS) {
        warn!("failed to mark '{}' as prefetched: {err}", job.label);
    }

    Ok(())
}

pub fn run(payload: &[u8]) -> Result<String, Error> {
    let job: Job = serde_json::from_slice(payload)?;
    let query =
        serde_urlencoded::to_string([("u", job.username.as_str()), ("id", job.track_id.as_str())])?;

    let body = subsonicapi::call(&format!("getLyricsBySongId?{query}"))?;
    let message = match parse_lyrics_count(&body)? {
        0 => format!("no lyrics found for '{}'", job.label),
        count => format!("prefetched {count} lyrics entries for '{}'", job.label),
    };

    info!("{message}");
    Ok(message)
}

fn dedupe_key(track_id: &str) -> String {
    format!("{DEDUPE_KEY_PREFIX}{track_id}")
}

#[derive(Deserialize)]
struct Envelope {
    #[serde(rename = "subsonic-response")]
    response: SubsonicResponse,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SubsonicResponse {
    status: String,
    #[serde(default)]
    error: Option<SubsonicError>,
    #[serde(default)]
    lyrics_list: Option<LyricsList>,
}

#[derive(Deserialize)]
struct SubsonicError {
    #[serde(default)]
    message: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LyricsList {
    #[serde(default)]
    structured_lyrics: Vec<IgnoredAny>,
}

fn parse_lyrics_count(body: &str) -> Result<usize, Error> {
    let response = serde_json::from_str::<Envelope>(body)?.response;

    if response.status != "ok" {
        let message = response.error.map(|e| e.message).unwrap_or_default();
        return Err(Error::msg(format!(
            "getLyricsBySongId returned '{}': {message}",
            response.status
        )));
    }

    Ok(response
        .lyrics_list
        .map_or(0, |list| list.structured_lyrics.len()))
}
