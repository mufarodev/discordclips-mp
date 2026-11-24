# Reference: Windows clips module (interface notes only)

Observed from the shipped Canary build by static inspection of strings/imports. Used for *vocabulary and design inspiration*; no code is copied.

- `discord_clips.node`: N-API shim, exports one JS function `getModulePath`. JS spawns the worker.
- `discord_clips.exe`: separate worker process, pipe `\\.\pipe\discord-clips-<id>`, framed serde-style tagged enums.
- Commands: StartRecording(fps, duration_ms, bitrate_kbps, startup_delay_ms, …), StopRecording, SetDuration, SetBitrate, SaveClip(filepath, metadata, trim_start_ms, trim_end_ms, thumbnail_ms, saved_at, …), ExportClip, ExportClipToFile, TakeScreenshot(quality), VideoFrame(handle, adapter_luid, timestamp_ms, content_width/height, slot_idx, generation), AudioStreamCreated, SetAudioStreamPermissions(allow_recording), VideoFrameControlCreated(slot_count), ExportVideoComposition(recipe_json, output_path), Shutdown, PerfMonitorEnable, SetMlEnabled, GetStats.
- Events: ClipSaved(filepath, duration_ms, stats_json, thumbnail, metadata), ClipSaveFailed, InitFailure, RecordingEvent, ExportComplete, ExportToFileComplete, ScreenshotComplete, MlDetection(label, confidence, user_id), CompositionExported, Error, PerfMonitorResult, Stats.
- Worker flags: --render --file --log-dir --data-dir --enable-ml --audio-model --sentry-*.
- Pipeline: rolling buffer → HW H.264/HEVC/AV1 → custom MP4 mux, AAC-LC.
