# Reference: Windows clips module (interface notes only)

Observed from the shipped Canary build by static inspection of strings/imports. Used for *vocabulary and design inspiration*; no code is copied.

- `discord_clips.node`: N-API shim, exports one JS function `getModulePath`. JS spawns the worker.
- `discord_clips.exe`: separate worker process, pipe `\\.\pipe\discord-clips-<id>`, framed serde-style tagged enums.
- Commands: StartRecording(fps, duration_ms, bitrate_kbps, startup_delay_ms, …), StopRecording, SetDuration, SetBitrate, SaveClip(filepath, metadata, trim_start_ms, trim_end_ms, thumbnail_ms, saved_at, …), ExportClip, ExportClipToFile, TakeScreenshot(quality), VideoFrame(handle, adapter_luid, timestamp_ms, content_width/height, slot_idx, generation), AudioStreamCreated, SetAudioStreamPermissions(allow_recording), VideoFrameControlCreated(slot_count), ExportVideoComposition(recipe_json, output_path), Shutdown, PerfMonitorEnable, SetMlEnabled, GetStats.
- Events: ClipSaved(filepath, duration_ms, stats_json, thumbnail, metadata), ClipSaveFailed, InitFailure, RecordingEvent, ExportComplete, ExportToFileComplete, ScreenshotComplete, MlDetection(label, confidence, user_id), CompositionExported, Error, PerfMonitorResult, Stats.
- Worker flags: --render --file --log-dir --data-dir --enable-ml --audio-model --sentry-*.
- Pipeline: rolling buffer → HW H.264/HEVC/AV1 → custom MP4 mux, AAC-LC.

## macOS client (Canary 0.0.1591) — what's already there (interface notes only)
Observed by static inspection of the installed app's JS and native-module strings.

- **Clip files (main process, in `discord_desktop_core`)**: the macOS client already has the file side of clips: a custom protocol that serves clips with range requests, `loadClip`/`loadClipsDirectory`/`deleteClip`/`moveClip`/`updateClipMetadata`/`getClipMetadata`, a clips data dir, and a main-process logger named `discord_clips_main_proc`.
- **Clip validity**: `.mp4` extension; bytes 4..12 are `ftypisom`; walking top-level boxes (32-bit sizes, each position needs 24 readable bytes) must find a `uuid` box with UUID `a1c85299-3346-4db8-88f0-83f57a75a5ef`; box body after the 16-byte UUID is UTF-8 JSON metadata (schema owned by the host/renderer). `delete`/`move` re-verify this box before acting. Screenshots are `.jpg` whose JPEG COM comment is JSON with a string `id` and optional numeric `version`.
- **Worker location**: `getModulePath("discord_clips")` (falling back to `<module_data>/discord_clips`) + `discord_clips` (`.exe` on win32). Used by the main process for `renderMontageRecipe`, which spawns the worker as a one-shot process with a recipe.
- **Native engine (`discord_voice.node`)** already exports the clips control surface: `setClipBufferLength`, `setClipRecordUser`, `setClipsSource`, `saveClipEx`, `exportClipToFile`, `applyClipsSettings`, `applyClipsQualitySettings`, `setIgnoreGlobalClipDisplay`, `setIgnoreGlobalClipSingleWindow`, `SaveClipForUser`, `UpdateClipMetadata` — **and a string `clips not supported in this build`**. No `discord_clips` worker launch, no pipe client, and no clips-related ScreenCaptureKit code were found in the macOS binary.
- **JS module wrapper (`discord_voice/index.js`)** wraps `discord_voice.node` and gates `features.declareSupported('clips')`, `'clips_thumbnail'`, `'clips_recording_ready_events'` under `process.platform === 'win32'`. The renderer calls Clips V3 methods (`setClipsModulePath`, `setClipsDataPath`, `setClipsRecordingEnabled`, `setClipsSource`, `applyClipsSettings`, `applyClipsQualitySettings`, `setClipBufferLength`, `setOnClipsRecordingEvent`, `saveClipEx`, `saveScreenshot`, `updateClipMetadata`, `exportClipToFile`) directly on the `VoiceEngine` object exported by `discord_voice/index.js`.
- **Standalone module loading**: `Contents/Resources/build_info.json` supports `"disableUpdater": true` and `"standaloneModules": true`, causing `app.asar` to skip `updater.node` and load modules from `Contents/Resources/standalone_modules/`.
- No `discord_clips` module is installed on macOS (modules present: desktop_core, voice, utils, game_utils, krisp, …).
