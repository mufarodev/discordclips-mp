"use strict";

// End-to-end integration test of the macOS discord_clips module + clips_bridge.js
// driving the real clipsd binary over its Unix socket, validating the exact
// Discord MediaEngineNative callback contracts and file formats.

const assert = require("assert");
const fs = require("fs");
const os = require("os");
const path = require("path");

const attachClipsBridge = require("../common/clips_bridge");
const discordClipsMod = require("../common/discord_clips");

const DISCORD_UUID = Buffer.from("a1c8529933464db888f083f57a75a5ef", "hex");

function readDiscordUuidBox(filepath) {
  const buf = fs.readFileSync(filepath);
  assert.strictEqual(buf.subarray(4, 12).toString("ascii"), "ftypisom", "MP4 must start with ftypisom");
  let i = buf.readUInt32BE(0);
  while (i < buf.length) {
    assert(i + 24 <= buf.length, "each top-level box must have >= 24 readable bytes");
    const size = buf.readUInt32BE(i);
    assert(size >= 8, "box size must be >= 8");
    if (buf.subarray(i + 4, i + 8).toString("ascii") === "uuid" && buf.subarray(i + 8, i + 24).equals(DISCORD_UUID)) {
      return JSON.parse(buf.subarray(i + 24, i + size).toString("utf8"));
    }
    i += size;
  }
  throw new Error("no Discord uuid box found");
}

function readJpegComment(filepath) {
  const buf = fs.readFileSync(filepath);
  assert(buf[0] === 0xff && buf[1] === 0xd8, "not a JPEG");
  let pos = 2;
  while (pos + 4 <= buf.length) {
    assert.strictEqual(buf[pos], 0xff);
    const marker = buf[pos + 1];
    const len = buf.readUInt16BE(pos + 2);
    if (marker === 0xfe) {
      return JSON.parse(buf.subarray(pos + 4, pos + 2 + len).toString("utf8"));
    }
    if (marker === 0xda) break;
    pos += 2 + len;
  }
  throw new Error("no JPEG COM comment found");
}

async function main() {
  const binDir = process.argv[2];
  assert(binDir && fs.existsSync(path.join(binDir, "clipsd")), "usage: node test_bridge.js <dir-with-clipsd>");

  // Mock VoiceEngine & features (identical shape to discord_voice/index.js)
  const supported = new Set();
  const features = { declareSupported: (f) => supported.add(f) };
  const VoiceEngine = {};

  assert.strictEqual(typeof discordClipsMod.getModulePath(), "string");
  attachClipsBridge(VoiceEngine, features);

  // 1. Verify renderer eligibility prerequisites (features + hasClipsV3Support)
  assert(supported.has("clips"), "must declare 'clips'");
  assert(supported.has("clips_thumbnail"), "must declare 'clips_thumbnail'");
  assert(supported.has("clips_recording_ready_events"), "must declare 'clips_recording_ready_events'");
  assert.strictEqual(typeof VoiceEngine.setClipsModulePath, "function");
  assert.strictEqual(typeof VoiceEngine.setClipsRecordingEnabled, "function");
  assert.strictEqual(typeof VoiceEngine.exportClipToFile, "function");

  const events = [];
  VoiceEngine.setOnClipsRecordingEvent((code, msg) => events.push({ code, msg }));
  VoiceEngine.setClipsModulePath(binDir);
  VoiceEngine.setClipsRecordingEnabled(true);
  VoiceEngine.setClipBufferLength(10);
  VoiceEngine.applyClipsSettings({ frameRate: 30, width: 1280, height: 720, bitrateKbps: 4000 });

  // 2. Start capture (non-existent window id exercises fallback to main display)
  VoiceEngine.setClipsSource({ id: "screen:0", soundshareId: 0 });
  const deadline = Date.now() + 10000;
  while (!events.some((e) => e.code === 10 || e.code === 2) && Date.now() < deadline) {
    await new Promise((r) => setTimeout(r, 100));
  }
  const errEv = events.find((e) => e.code === 2);
  assert(!errEv, `capture init failed: ${errEv?.msg}`);
  assert(events.some((e) => e.code === 10), "expected RecordingActive (10) event");
  assert(events.some((e) => e.code === 9), "expected RecordingHealthy (9) event");

  // Buffer ~2.5s so we have at least one full GOP
  await new Promise((r) => setTimeout(r, 2500));

  const tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), "clipsd-bridge-test-"));
  try {
    // 3. saveClipEx (using steady-clock timestamps, matching Discord's real call shape)
    const clipPath = path.join(tmpDir, "test-clip.mp4");
    const initMeta = { id: "clip-test-1", applicationName: "TestApp", version: 1, type: "clip" };
    const nowMs = VoiceEngine.getSystemSteadyClockNowMs();
    const saveRes = await new Promise((resolve, reject) => {
      VoiceEngine.saveClipEx(
        {
          filepath: clipPath,
          metadata: JSON.stringify(initMeta),
          startMs: nowMs - 30000,
          endMs: nowMs,
          trimStartMs: nowMs - 30000,
          trimEndMs: nowMs,
          thumbnailMs: nowMs - 30000,
        },
        (duration, statsJson, thumbnail, metadataJson) => resolve({ duration, statsJson, thumbnail, metadataJson }),
        (errJson) => reject(new Error(`saveClipEx failed: ${errJson}`))
      );
    });
    assert(saveRes.duration > 500, `expected duration > 500ms, got ${saveRes.duration}`);
    const stats = JSON.parse(saveRes.statsJson);
    assert(stats.clipSizeBytes > 1000, `expected clipSizeBytes > 1000, got ${stats.clipSizeBytes}`);
    assert(saveRes.thumbnail.startsWith("data:image/jpeg;base64,"), "expected data URL thumbnail");
    const boxMeta = readDiscordUuidBox(clipPath);
    assert.strictEqual(boxMeta.id, "clip-test-1");
    assert.strictEqual(boxMeta.length, saveRes.duration);
    assert(boxMeta.thumbnail.startsWith("data:image/jpeg;base64,"));
    assert(!fs.existsSync(clipPath.replace(/\.mp4$/, ".jpg")), "must not leave stray .jpg sidecar");

    // 4. updateClipMetadata
    const updatedMeta = { ...boxMeta, name: "Renamed Clip", isFavorite: true };
    await new Promise((resolve, reject) => {
      VoiceEngine.updateClipMetadata(clipPath, JSON.stringify(updatedMeta), resolve, (e) => reject(new Error(e)));
    });
    const boxMeta2 = readDiscordUuidBox(clipPath);
    assert.strictEqual(boxMeta2.name, "Renamed Clip");
    assert.strictEqual(boxMeta2.isFavorite, true);

    // 5. exportClipToFile
    const exportPath = path.join(tmpDir, "export", "out.mp4");
    const exp = await new Promise((resolve, reject) => {
      VoiceEngine.exportClipToFile(clipPath, exportPath, {}, (fp, fmt) => resolve({ fp, fmt }), (e) => reject(new Error(e)));
    });
    assert.strictEqual(exp.fp, exportPath);
    assert.strictEqual(exp.fmt, false);
    assert.strictEqual(readDiscordUuidBox(exportPath).name, "Renamed Clip");

    // 6. saveScreenshot
    const shotPath = path.join(tmpDir, "shot.jpeg");
    const shotMeta = { id: "shot-test-1", applicationName: "TestApp", version: 1, type: "screenshot" };
    const shotBuf = await new Promise((resolve, reject) => {
      VoiceEngine.saveScreenshot(shotPath, JSON.stringify(shotMeta), "", 85, 0, resolve, (e) => reject(new Error(e)));
    });
    assert(Buffer.isBuffer(shotBuf) && shotBuf[0] === 0xff && shotBuf[1] === 0xd8, "expected JPEG Buffer");
    const comment = readJpegComment(shotPath);
    assert.strictEqual(comment.id, "shot-test-1");

    // 7. Stop recording
    VoiceEngine.setClipsSource({ id: "", soundshareId: 0 });
    await new Promise((r) => setTimeout(r, 300));
    assert(events.some((e) => e.code === 11), "expected RecordingInactive (11) event");

    console.log("PASS: all 7 bridge integration checks succeeded", {
      durationMs: saveRes.duration,
      clipSizeBytes: stats.clipSizeBytes,
      thumbLen: saveRes.thumbnail.length,
      shotBytes: shotBuf.length,
    });
  } finally {
    fs.rmSync(tmpDir, { recursive: true, force: true });
    process.exit(0);
  }
}

main().catch((err) => {
  console.error("FAIL:", err);
  process.exit(1);
});
