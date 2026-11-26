use crate::{
    config::PluginConfig,
    types::{Lyrics, LyricsKind},
};
use nd_pdk::{
    host::library,
    lyrics::{Error as LyricsError, TrackInfo},
};
use std::{fs, path::Path, path::PathBuf};

pub fn write(track: &TrackInfo, lyrics: &Lyrics, cfg: &PluginConfig) -> Result<(), LyricsError> {
    let path = if cfg.write_to_specific_folder {
        resolve_custom_path(track, lyrics.kind(), cfg)?
    } else {
        resolve_sidecar_path(track, lyrics.kind(), cfg)?
    };

    if cfg.upgrade_lyrics {
        if is_upgrade(track, &path, &lyrics.kind(), cfg)? {
            delete_existing(track, cfg)?;
        } else {
            return Ok(());
        }
    } else if path.exists() {
        return Ok(());
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| LyricsError::new(format!("failed to create lyrics directory: {e}")))?;
    }

    fs::write(&path, lyrics.text(cfg).as_bytes())
        .map_err(|e| LyricsError::new(format!("failed to write lyrics file: {e}")))?;

    Ok(())
}

fn is_upgrade(
    track: &TrackInfo,
    path: &Path,
    kind: &LyricsKind,
    cfg: &PluginConfig,
) -> Result<bool, LyricsError> {
    if path.exists() {
        return Ok(false);
    }
    let paths: Vec<PathBuf> = cfg
        .lyrics_type_priority
        .iter()
        .take_while(|k| *k != kind)
        .map(|k| {
            if cfg.write_to_specific_folder {
                resolve_custom_path(track, *k, cfg)
            } else {
                resolve_sidecar_path(track, *k, cfg)
            }
        })
        .collect::<Result<Vec<PathBuf>, LyricsError>>()?;
    let has_better = paths.iter().any(|p| p.exists());
    Ok(!has_better)
}

fn delete_existing(track: &TrackInfo, cfg: &PluginConfig) -> Result<(), LyricsError> {
    for k in &cfg.lyrics_type_priority {
        let path = if cfg.write_to_specific_folder {
            resolve_custom_path(track, *k, cfg)?
        } else {
            resolve_sidecar_path(track, *k, cfg)?
        };

        if path.exists() {
            fs::remove_file(&path).map_err(|e| {
                LyricsError::new(format!("failed to delete existing lyrics file: {e}"))
            })?;
        }
    }

    Ok(())
}

fn resolve_custom_path(
    track: &TrackInfo,
    kind: LyricsKind,
    cfg: &PluginConfig,
) -> Result<PathBuf, LyricsError> {
    let library_id = cfg.write_to_specific_folder_library_id.ok_or_else(|| {
        LyricsError::new("a library ID is required when write to custom path is enabled")
    })?;

    let lib = library::get_library(library_id)
        .map_err(|e| LyricsError::new(format!("failed to query library {library_id}: {e}")))?
        .ok_or_else(|| LyricsError::new(format!("library with ID {library_id} not found")))?;

    let ext = cfg.extension_for(kind);

    let relative_path = process_template(&cfg.write_to_specific_folder_template, track, kind, ext);

    Ok(PathBuf::from(lib.mount_point).join(relative_path))
}

fn resolve_sidecar_path(
    track: &TrackInfo,
    kind: LyricsKind,
    cfg: &PluginConfig,
) -> Result<PathBuf, LyricsError> {
    if track.path.is_empty() {
        return Err(LyricsError::new("track path is empty"));
    }

    let mut path = resolve_track_path(track)?
        .ok_or_else(|| LyricsError::new("could not resolve track path to a valid local file"))?;

    path.set_extension(cfg.extension_for(kind));
    Ok(path)
}

fn resolve_track_path(track: &TrackInfo) -> Result<Option<PathBuf>, LyricsError> {
    let library = library::get_library(track.library_id).map_err(|e| {
        LyricsError::new(format!("failed to get library {}: {e}", track.library_id))
    })?;

    if let Some(library) = library {
        let path = PathBuf::from(library.mount_point).join(&track.path);
        if path.exists() {
            return Ok(Some(path));
        }
    } else {
        return Err(LyricsError::new(format!(
            "library {} not found",
            track.library_id
        )));
    }

    Ok(None)
}

fn process_template(template: &str, track: &TrackInfo, kind: LyricsKind, ext: &str) -> PathBuf {
    let type_str = kind.slug();

    let mut path = PathBuf::new();

    for component in template.split(&['/', '\\']) {
        if component.is_empty() {
            continue;
        }

        let mut substituted = component
            .replace("{type}", type_str)
            .replace("{track:id}", &track.id)
            .replace("{track:title}", &track.title)
            .replace("{track:album}", &track.album)
            .replace("{track:artist}", &track.artist)
            .replace("{track:album_artist}", &track.album_artist);

        substituted = replace_padded_variable(
            &substituted,
            "track:track_number",
            &track.track_number.to_string(),
        );
        substituted = replace_padded_variable(
            &substituted,
            "track:disc_number",
            &track.disc_number.to_string(),
        );

        let sanitized = sanitize_path_segment(&substituted);

        if !sanitized.is_empty() {
            path.push(sanitized);
        }
    }

    if template.ends_with('/') || template.ends_with('\\') {
        path.push("unknown");
    }

    if path.as_os_str().is_empty() {
        path.push("unknown");
    }

    let path_str = path.as_os_str().to_string_lossy().to_string();

    if path_str.ends_with(&format!(".{ext}")) {
        return PathBuf::from(path_str);
    }

    let clean_path = path_str.trim_end_matches('.');

    if clean_path.is_empty() {
        PathBuf::from(format!("unknown.{ext}"))
    } else {
        PathBuf::from(format!("{clean_path}.{ext}"))
    }
}

fn replace_padded_variable(template: &str, var_name: &str, value: &str) -> String {
    let mut result = template.to_string();
    let search_prefix = format!("{{{var_name}");

    let mut start = 0;
    while let Some(pos) = result[start..].find(&search_prefix) {
        let abs_pos = start + pos;
        let remaining = &result[abs_pos..];

        if let Some(end_offset) = remaining.find('}') {
            let var_end = abs_pos + end_offset + 1;
            let full_var = &result[abs_pos..var_end];

            let inner = &full_var[1..full_var.len() - 1];
            let width = if let Some(padding_str) = inner.strip_prefix(&format!("{var_name}:")) {
                padding_str.parse::<usize>().unwrap_or(0)
            } else {
                0
            };

            let formatted_value = if width > 0 {
                format!("{:0>width$}", value, width = width)
            } else {
                value.to_string()
            };

            result.replace_range(abs_pos..var_end, &formatted_value);
            start = abs_pos + formatted_value.len();
        } else {
            break;
        }
    }
    result
}

fn sanitize_path_segment(name: &str) -> String {
    let mut sanitized = String::with_capacity(name.len());
    let mut last_underscore = false;

    for c in name.chars() {
        let is_invalid =
            c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|');

        if is_invalid {
            if !last_underscore {
                sanitized.push('_');
                last_underscore = true;
            }
        } else {
            sanitized.push(c);
            last_underscore = c == '_';
        }
    }

    let trimmed = sanitized
        .trim_matches(|c: char| c.is_whitespace() || c == '.')
        .to_string();

    if trimmed.is_empty() || trimmed.chars().all(|c| c == '_') {
        "unknown".to_string()
    } else {
        trimmed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        track: TrackInfo,
        kind: LyricsKind,
        ext: &'static str,
    }

    fn track() -> Fixture {
        Fixture {
            track: TrackInfo {
                id: "123".to_string(),
                title: "Test Song".to_string(),
                album: "Test Album".to_string(),
                artist: "Test Artist".to_string(),
                album_artist: "Test Album Artist".to_string(),
                track_number: 1,
                disc_number: 1,
                ..Default::default()
            },
            kind: LyricsKind::Lrc,
            ext: "lrc",
        }
    }

    impl Fixture {
        fn title(mut self, title: &str) -> Fixture {
            self.track.title = title.to_string();
            self
        }

        fn artist(mut self, artist: &str) -> Fixture {
            self.track.artist = artist.to_string();
            self
        }

        fn album(mut self, album: &str) -> Fixture {
            self.track.album = album.to_string();
            self
        }

        fn track_number(mut self, number: i32) -> Fixture {
            self.track.track_number = number;
            self
        }

        fn disc_number(mut self, number: i32) -> Fixture {
            self.track.disc_number = number;
            self
        }

        fn plain_text(mut self) -> Fixture {
            self.kind = LyricsKind::Plain;
            self.ext = "txt";
            self
        }

        #[track_caller]
        fn check(&self, template: &str, expected: &str) {
            let path = process_template(template, &self.track, self.kind, self.ext);
            assert_eq!(path, PathBuf::from(expected), "{template}");
        }
    }

    mod templates {
        use super::*;

        #[test]
        fn variables_are_filled_in_and_the_extension_appended() {
            track().check(
                "lyrics/{type}/{track:artist}/{track:title}",
                "lyrics/lrc/Test Artist/Test Song.lrc",
            );
        }

        #[test]
        fn extensions_are_not_repeated() {
            track().check("lyrics/{track:title}.lrc", "lyrics/Test Song.lrc");
            track().check("lyrics/{track:title}.backup", "lyrics/Test Song.backup.lrc");
            track().check(
                "lyrics/{track:title}.backup.txt",
                "lyrics/Test Song.backup.txt.lrc",
            );
            track().check("lyrics/{track:title}.txt", "lyrics/Test Song.txt.lrc");
        }

        #[test]
        fn both_slashes_separate_directories() {
            track().check(
                r"lyrics\{type}/{track:artist}\{track:title}",
                "lyrics/lrc/Test Artist/Test Song.lrc",
            );
            track().check(
                "lyrics///{type}//{track:artist}/{track:title}",
                "lyrics/lrc/Test Artist/Test Song.lrc",
            );
        }

        #[test]
        fn a_name_that_cannot_be_built_falls_back_to_unknown() {
            track().check("///", "unknown.lrc");
            track().check(
                "lyrics///{type}//{track:artist}/",
                "lyrics/lrc/Test Artist/unknown.lrc",
            );
            track()
                .plain_text()
                .title("")
                .artist("")
                .check("{track:artist}/{track:title}", "unknown/unknown.txt");
            track()
                .plain_text()
                .title("???")
                .artist("***")
                .check("{track:artist}/{track:title}", "unknown/unknown.txt");
        }

        #[test]
        fn dots_inside_a_name_are_kept() {
            track().artist("U2").album("Achtung.Baby").check(
                "{track:artist}/{track:album}/{track:title}",
                "U2/Achtung.Baby/Test Song.lrc",
            );
        }

        #[test]
        fn dots_around_a_name_are_not_kept() {
            track()
                .plain_text()
                .artist(".The Artist.")
                .title("  ..Hidden Song..  ")
                .check("{track:artist}/{track:title}", "The Artist/Hidden Song.txt");
        }

        #[test]
        fn it_is_not_possible_to_exit_a_directory() {
            track()
                .plain_text()
                .title("../../../etc/passwd")
                .check("lyrics/{track:title}", "lyrics/_.._.._etc_passwd.txt");

            track()
                .plain_text()
                .check("../../{track:title}", "unknown/unknown/Test Song.txt");
        }

        #[test]
        fn unicode_and_emoji_are_left_intact() {
            track().artist("Björk").title("大丈夫 🎵").check(
                "lyrics/{track:artist}/{track:title}",
                "lyrics/Björk/大丈夫 🎵.lrc",
            );
        }

        #[test]
        fn a_variable_that_is_not_recognized_stays_as_is() {
            track()
                .plain_text()
                .check("lyrics/{track:titel}", "lyrics/{track_titel}.txt");
            track()
                .plain_text()
                .check("lyrics/{ track:title }", "lyrics/{ track_title }.txt");
        }

        #[test]
        fn numbers_can_be_padded_by_the_template() {
            let track = track().plain_text().disc_number(1).track_number(5);

            track.check(
                "lyrics/Disc {track:disc_number}/{track:track_number} - {track:title}",
                "lyrics/Disc 1/5 - Test Song.txt",
            );
            track.check(
                "lyrics/Disc {track:disc_number:2}/{track:track_number:2} - {track:title}",
                "lyrics/Disc 01/05 - Test Song.txt",
            );
        }
    }

    mod padded_variables {
        use super::*;

        #[track_caller]
        fn check(template: &str, value: &str, expected: &str) {
            let replaced = replace_padded_variable(template, "track:track_number", value);
            assert_eq!(replaced, expected, "{template}");
        }

        #[test]
        fn a_bare_variable_takes_the_value_as_is() {
            check("{track:track_number}", "5", "5");
        }

        #[test]
        fn a_width_pads_the_value_with_zeros() {
            check("{track:track_number:2}", "5", "05");
            check("{track:track_number:3}", "12", "012");
        }

        #[test]
        fn a_width_that_is_not_a_number_is_ignored() {
            check("{track:track_number:abc}", "5", "5");
        }
    }

    mod path_segments {
        use super::*;

        #[track_caller]
        fn check(name: &str, expected: &str) {
            assert_eq!(sanitize_path_segment(name), expected, "{name}");
        }

        #[test]
        fn regular_names_are_kept() {
            check("Hello World", "Hello World");
        }

        #[test]
        fn invalid_path_characters_become_underscores() {
            check("AC/DC", "AC_DC");
            check("A:B|C?D*E", "A_B_C_D_E");
            check("<Track>", "_Track_");
        }

        #[test]
        fn a_run_of_invalid_characters_becomes_a_single_underscore() {
            check("Song ??? Title", "Song _ Title");
            check("Song//Title", "Song_Title");
        }

        #[test]
        fn surrounding_spaces_and_dots_are_trimmed() {
            check("  *test*  ", "_test_");
            check("...file...", "file");
            check(" . _ mixed _ . ", "_ mixed _");
        }

        #[test]
        fn underscores_the_name_came_with_are_kept() {
            check("_song_", "_song_");
            check("_intro_", "_intro_");
            check("__init__", "__init__");
            check(" _spaces_and_underscores_ ", "_spaces_and_underscores_");
        }

        #[test]
        fn a_name_with_nothing_left_becomes_unknown() {
            check("", "unknown");
            check("???", "unknown");
            check("...", "unknown");
            check("   ", "unknown");
            check("___", "unknown");
            check(". _ .", "unknown");
        }
    }
}
