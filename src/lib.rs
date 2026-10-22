use nd_pdk::lifecycle::{Error as LifecycleError, InitProvider};
use nd_pdk::lyrics::{
    Error as LyricsError, GetLyricsRequest, GetLyricsResponse, Lyrics as LyricsPlugin,
};
use nd_pdk::scrobbler::{
    Error as ScrobblerError, IsAuthorizedRequest, NowPlayingRequest, PlaybackReportRequest,
    ScrobbleRequest, Scrobbler,
};
use nd_pdk::taskworker::{Error as TaskError, TaskExecuteProvider, TaskExecuteRequest};

mod cache;
mod config;
mod engine;
mod ext;
mod fetch;
mod format;
mod prefetch;
mod providers;
mod types;
mod writing;

#[derive(Default)]
struct Plugin;

nd_pdk::register_lyrics!(Plugin);
nd_pdk::register_lifecycle_init!(Plugin);
nd_pdk::register_scrobbler!(Plugin);
nd_pdk::register_taskworker_task_execute!(Plugin);

impl LyricsPlugin for Plugin {
    fn get_lyrics(&self, req: GetLyricsRequest) -> Result<GetLyricsResponse, LyricsError> {
        engine::get_lyrics(req.track)
    }
}

impl InitProvider for Plugin {
    fn on_init(&self) -> Result<(), LifecycleError> {
        prefetch::create_queue().map_err(|e| LifecycleError::new(e.to_string()))
    }
}

impl Scrobbler for Plugin {
    fn is_authorized(&self, _req: IsAuthorizedRequest) -> Result<bool, ScrobblerError> {
        config::fetch_on_play().map_err(|e| ScrobblerError::new(e.to_string()))
    }

    fn now_playing(&self, req: NowPlayingRequest) -> Result<(), ScrobblerError> {
        let track = req.track;
        let label = ext::track_label(&track.id, &track.artist, &track.title);

        prefetch::enqueue(&req.username, &track.id, label)
            .map_err(|e| ScrobblerError::new(e.to_string()))
    }

    fn scrobble(&self, _req: ScrobbleRequest) -> Result<(), ScrobblerError> {
        Ok(())
    }

    fn playback_report(&self, _req: PlaybackReportRequest) -> Result<(), ScrobblerError> {
        Ok(())
    }
}

impl TaskExecuteProvider for Plugin {
    fn on_task_execute(&self, req: TaskExecuteRequest) -> Result<String, TaskError> {
        prefetch::run(&req.payload).map_err(|e| TaskError::new(e.to_string()))
    }
}
