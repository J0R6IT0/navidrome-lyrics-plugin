# Navidrome Lyrics Plugin

A Navidrome plugin for fetching lyrics from multiple sources. Formerly Navidrome LRCLIB Plugin.

> [!IMPORTANT]
> The Navidrome WebUI does not display lyrics from plugins at the moment, you need a third party client in order to see them.

## Features

- Multiple [lyrics providers](https://github.com/J0R6IT0/navidrome-lyrics-plugin#providers).
- Supports plain, line-synced and word-by-word lyrics in TTML, LRC, ELRC, Lyricsfile and other formats.
- Allows writing lyrics to sidecar files.
- Different provider modes to query by priority, preferred format or sync level.
- Configurable caching to reduce network calls.
- Cleans up lyrics by removing section labels, credits and other unwanted text.

## Installation

Make sure your Navidrome version is at least `v0.63.0`.

1. Download the latest `nd-lyrics.ndp` from the Releases page and place it in your plugins folder.

2. Add `nd-lyrics` to the `LyricsPriority` config option. See [here](https://www.navidrome.org/docs/usage/configuration/options/#:~:text=true-,LyricsPriority,-ND_LYRICSPRIORITY).

> [!IMPORTANT]
> The value added to `LyricsPriority` should match the name of the plugin without the extension. If you rename the plugin to `lyrics.ndp`, you should add `lyrics` instead of `nd-lyrics`.

> [!TIP]
> If you are using the "Write lyrics to files" option, you can do `".ttml,.yaml,.yml,.elrc,.lrc,.srt,.txt,embedded,nd-lyrics"` so Navidrome reads the files directly when available. This will only work if "Write to custom path" is disabled.

> [!TIP]
> If you're running the TrueNAS Community Edition Navidrome app, configuration is done via environment variables. Set `ND_LYRICSPRIORITY` to the same value you would use for `LyricsPriority`.

3. You may need to restart Navidrome for the plugin to be detected. Don't forget to enable the plugin and configure it to your liking.

4. The plugin will fetch lyrics only when a client requests them.

## Providers

At this time, the following providers are available. Please report any issues you encounter while using them.

Some of the providers don't host lyrics themselves, they pull them from other websites. Those are indicated in the `Sources` column.

| Provider    | Sources                                                               | Type                 | Notes                                                                |
| ----------- | --------------------------------------------------------------------- | -------------------- | -------------------------------------------------------------------- |
| LRCLIB      |                                                                       | plain,lrc,lyricsfile | Supports custom instances                                            |
| lyrics.ovh  | Genius, AZLyrics, Paroles.net, LyricsMania, Letras.mus.br, Lyrics.com | plain                | Supports custom instances                                            |
| lrcmux      | Genius, KuGou, Musixmatch, LRCLIB, NetEase, YouTube Music             | plain,lrc,elrc       | Supports custom instances                                            |
| KuGou       |                                                                       | lrc,elrc             |                                                                      |
| NetEase     |                                                                       | lrc,elrc             |                                                                      |
| QQ Music    |                                                                       | lrc,elrc             |                                                                      |
| Apple Music |                                                                       | ttml                 | Supports translations and romanization. Requires active subscription |
| stixoi.info |                                                                       | plain                | Greek lyrics                                                         |
| Genie Music |                                                                       | lrc                  | Korean lyrics                                                        |
| BiniLyrics  |                                                                       | ttml                 |                                                                      |

## Provider modes

The **provider mode** controls how the provider list is used on each lookup:

- **Priority**: tries providers top to bottom and the first one that returns lyrics wins.
- **Rotation**: each lookup starts with the next provider in the list, cycling on successive calls; the rest act as fallbacks. Useful to spread load and avoid rate limits.
- **Type priority**: queries providers until it has the highest-priority format they can collectively offer, then returns the best result found, instead of stopping at the first hit.

  For example, with format priority `ttml,elrc,lrc,plain` and providers `qqmusic,netease,kugou,lyrics.ovh`, the best achievable format is `elrc` (none of these serve `ttml`). Each provider is queried in turn until one yields `elrc`. A provider is skipped once it cannot beat what has already been fetched (e.g. `lyrics.ovh`, which only serves `plain`, is skipped when an `lrc` result is already in hand). On a tie, the higher provider in the list wins. This makes more requests per lookup in exchange for the best available format.

- **Best sync level**: works like type priority, but ranks results by their sync level rather than by the configured format order:

  `word-by-word` > `line-by-line` > `plain`

## Path variables

Custom paths to write lyrics files to can be composed using path variables.

Consider the following example:

```
_lyrics/{type}/{track:album}/{track:track_number:2} - {track:title}
```

This will be transformed into something like this:

```
<selected_library_root>/_lyrics/lrc/The Razors Edge/01 - Thunderstruck.lrc
```

Note that the extension is appended automatically based on the configuration and lyrics type.

| Variable             | Description                                                                                |
| -------------------- | ------------------------------------------------------------------------------------------ |
| {type}               | The type of lyrics ("plain", "lrc", "elrc", "ttml", "srt", "lyricsfile" or "instrumental") |
| {track:id}           | The ID of the track                                                                        |
| {track:title}        | The title of the track                                                                     |
| {track:album}        | The name of the album this track belongs to                                                |
| {track:artist}       | The artist of the track                                                                    |
| {track:album_artist} | The artist of the album this track belongs to                                              |
| {track:track_number} | The number of track in the album\*                                                         |
| {track:disc_number}  | The number of the disc in the album\*                                                      |

\* {track:track_number} and {track:disc_number} accept a padding argument to fill with 0s:

`{track:track_number:2}` will ensure that there are at least 2 digits. For example, track_number `1` will become `01`.
