# Video watcher and player

## What it does

Models can't watch video, so Trek gives every agent a `watch_video` tool and the user a player.

`watch_video` (Trek's `trek-video` MCP server, `trek-mcp video`) takes `source` (a local file, or an https URL; YouTube, X and Vimeo links go through yt-dlp when it's on the PATH, else only direct media links work), optional `start`/`end` seconds, `fps` (default 1, at most 10) and `transcript` (default true). It refuses http, `file://` and other schemes, credentials in a URL, and hosts on the machine or its network. It copies what it fetches into a scratch folder of its own under Trek's cache (`~/Library/Caches/dev.trek.Trek/videos/<id>/`, or `<data dir>/cache/videos` when Trek's data is moved, as with `trek-dev`; downloads stop at 500 MB, folders older than a week are cleared). It reads the video with ffprobe and takes frames with ffmpeg, fit to a long side of 1024 (at most 40 frames and 12 MB: past that it samples evenly and says so). It transcribes the audio with `whisper` when installed, else on a Mac with the Speech framework, in pieces under a minute, in a child process (`trek-mcp transcribe`); Windows without whisper skips it with a note. It answers with one text block (metadata, `Manifest: <path>`, the frames' timestamps, any notes, the transcript with timestamps) and then the frames as JPEG images, in order. Without ffmpeg/ffprobe it answers only "install them with `brew install ffmpeg`" (Windows: `winget install ffmpeg`); nothing is bundled.

The player is the card a note's video plays in (`artifact::video_card`: the engine's own `<video>`, with Open and Expand). It plays:

- a video an answer links on a line of its own: `[clip](demo.mp4)` (absolute, `~/` or relative to the thread's folder, and there on disk) or an https link whose path ends in `.mp4`, `.m4v`, `.mov` or `.webm`;
- a video attached to a message (the composer sends a non-image file as `@/path/clip.mov`), under the message;
- the video a finished `watch_video` call watched, under its row (the call gets a row of its own, out of the tool group), with the frames the model saw in a strip under the player. Clicking a frame takes the player there and plays; when there's no player (the download was cleared, or artifacts are off), the frame opens in the image viewer.

Local files are served by the card at `/media` (in pieces, for seeking) under the video policy (`media-src 'self'`, no scripts); a web video plays from its own address under `REMOTE_VIDEO_CSP` (`media-src https:`, nothing else). With Settings › General › Interactive artifacts off, an answer's video is a plain link line and no player shows.

Settings › Tools › Video: the "Video watcher" switch (on by default; new sessions) and whether ffmpeg is installed, with an Install ffmpeg button that runs the install in Trek's terminal.

## Reach it as a user

Ask an agent to watch a file or a link ("what happens in ~/Movies/bug.mov around 0:40?"). Drop a video into the composer to attach it. Ask for a link to a clip in an answer.

## Reach it with the CLI

```sh
./.agents/skills/verify-trek/scripts/trek-dev open draft
./.agents/skills/verify-trek/scripts/trek-dev send "mock:video"
./.agents/skills/verify-trek/scripts/trek-dev control artifacts           # the player cards, live or not, their frames
./.agents/skills/verify-trek/scripts/trek-dev screenshot
```

`mock:video` needs ffmpeg on the PATH. It makes a 6-second test pattern with a tone once, at `<isolated data dir>/mock-video/clip.mp4`, calls the session's real `trek-video` server on it (`fps` 1, no transcript: six frames), shows that as a `mcp__trek-video__watch_video` row, and answers with `[The test clip](<path>)`. So one turn shows both players: the one under the tool row with its six-frame strip, and the one in the answer. Without ffmpeg it says so. The isolated app hands sessions `trek-video` only when the bundled or built `trek-mcp` is next to the binary (`cargo build -p trek-mcp`).

The player is a native web view, so `screenshot` (the `shots` frame) shows the card's header and the strip but not the video; capture the window region with `screencapture -R` for the real picture, as for interactive artifacts. Seeking can't be checked through `control` either (a click on a frame reaches GPUI, which evaluates the seek in the page): click a frame with the real pointer posted to the dev Trek's process and capture again; the player's time moves to the frame's.

Manual path without the mock: generate a clip (`ffmpeg -f lavfi -i testsrc=duration=5:size=640x360:rate=24 -c:v mpeg4 /tmp/clip.mp4`), then `send "@/tmp/clip.mp4 what's in this?"` to see it under the message.

## Code

The tool: `crates/trek-mcp/src/video.rs` (sources, ffprobe, frame plan and budget, chunked transcripts, Speech child, yt-dlp and curl fetches), registered in `crates/trek-mcp/src/main.rs`. The manifest format, cache folder, link and mention rules: `crates/trek-core/src/media.rs`; the web video's page and policy and the seek script: `crates/trek-core/src/artifact.rs`. The players and the frame strip: `crates/trek-app/src/video.rs`; the card, `Doc::video`/`Doc::remote_video` and `seek`: `crates/trek-app/src/artifact/mod.rs`; the answer hook in `md.rs`; the row in `thread_view.rs` (`layout` gives a finished call its own row; `Row::Tool` and the user row draw the players). Wiring: `workspace.rs` (`video_server`: `TREK_VIDEO_DIR` and a 30-minute tool timeout); the switch and ffmpeg check in `settings_view.rs` and `integrations.rs`; the setting `tools.video` in `crates/trek-core/src/settings.rs`; the mock in `crates/trek-agents/src/mock.rs` (`watch_video`).

## Verify

`mock:video`: the tool row reads `mcp__trek-video__watch_video` with a check, and under it a card titled "clip.mp4" (Video · MP4 · its size) and "The 6 frames the agent saw" with six thumbnails labelled 0:00 to 0:05; the answer's link is a second card titled "The test clip". Both play when clicked (real capture). Clicking the fourth frame takes the player under the row to about 0:03. The tool's text (expand the row) starts with `Watched`, `Manifest:` and `Video: 0:06.0 long · 960x540 mpeg4 …`. Night and Paper both read. `logs --grep error` is empty. With the switch off, a new session's `mock:mcp` doesn't list `trek-video`, and `mock:video` says no watcher was given.
