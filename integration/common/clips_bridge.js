"use strict";

// Clean-room macOS & Linux Clips bridge for Discord's VoiceEngine JS wrapper.
// Speaks the length-prefixed JSON protocol in PROTOCOL.md to our `clipsd` worker binary
// over a private Unix domain socket, while leaving all voice/video calls on `discord_voice.node` untouched.

const { spawn } = require("child_process");
const fs = require("fs");
const net = require("net");
const os = require("os");
const path = require("path");

const MAX_FRAME = 16 * 1024 * 1024;

const RecordingEvent = {
  Started: 0,
  Ended: 1,
  Error: 2,
  StoppedByGoLive: 5,
  GoLiveEnded: 7,
  IdleShutdown: 8,
  RecordingHealthy: 9,
  RecordingActive: 10,
  RecordingInactive: 11,
};

function parseJsonSafe(raw) {
  if (raw == null) return null;
  if (typeof raw === "object") return raw;
  if (typeof raw !== "string" || raw.trim() === "") return null;
  try {
    return JSON.parse(raw);
  } catch {
    return null;
  }
}

function parseWindowId(sourceId) {
  if (sourceId == null) return null;
  const s = String(sourceId).trim();
  if (!s) return null;
  const stripped = s.includes(":") ? s.split(":").pop() : s;
  const n = Number(stripped);
  return Number.isInteger(n) && n > 0 && n <= 0xffffffff ? n : null;
}

function defaultSocketDir() {
  if (process.platform === "linux" && process.env.XDG_RUNTIME_DIR && fs.existsSync(process.env.XDG_RUNTIME_DIR)) {
    return process.env.XDG_RUNTIME_DIR;
  }
  return os.tmpdir();
}

class ClipsdClient {
  constructor(resolveBinaryPath) {
    this.resolveBinaryPath = resolveBinaryPath;
    this.sockPath = path.join(defaultSocketDir(), `clipsd-discord-${process.pid}.sock`);
    this.child = null;
    this.conn = null;
    this.starting = null;
    this.rxBuf = Buffer.alloc(0);
    this.queue = [];
    this.closed = false;

    const cleanup = () => this.shutdownSync();
    process.once("exit", cleanup);
    if (typeof globalThis.window !== "undefined" && globalThis.window?.addEventListener) {
      globalThis.window.addEventListener("beforeunload", cleanup);
    }
  }

  async ensureRunning() {
    if (this.conn && !this.conn.destroyed && this.child && this.child.exitCode === null) {
      return;
    }
    if (this.starting) {
      return this.starting;
    }
    this.starting = this._startAndConnect().finally(() => {
      this.starting = null;
    });
    return this.starting;
  }

  async _startAndConnect() {
    const bin = this.resolveBinaryPath();
    if (!bin || !fs.existsSync(bin)) {
      throw new Error(`clipsd binary not found (looked at ${bin || "<unset>"})`);
    }
    try {
      fs.unlinkSync(this.sockPath);
    } catch {}

    const child = spawn(bin, ["start", "--idle", "--socket", this.sockPath], {
      stdio: ["ignore", "pipe", "pipe"],
    });
    this.child = child;
    let stderrTail = "";
    child.stderr?.on("data", (chunk) => {
      const text = chunk.toString("utf8");
      stderrTail = (stderrTail + text).slice(-2000);
      for (const line of text.split(/\r?\n/)) {
        if (line.trim()) console.info("[clipsd]", line);
      }
    });
    child.on("exit", (code, sig) => {
      if (this.child === child) this.child = null;
      this._failPending(new Error(`clipsd exited (code=${code}, sig=${sig}): ${stderrTail.trim()}`));
    });

    const deadline = Date.now() + 8000;
    while (Date.now() < deadline) {
      if (child.exitCode !== null) {
        throw new Error(`clipsd exited during startup (code=${child.exitCode}): ${stderrTail.trim()}`);
      }
      if (fs.existsSync(this.sockPath)) {
        try {
          await this._connect();
          return;
        } catch {
          // Socket exists but listener may not have bound yet; retry briefly.
        }
      }
      await new Promise((r) => setTimeout(r, 40));
    }
    throw new Error(`timed out waiting for clipsd socket ${this.sockPath}`);
  }

  _connect() {
    return new Promise((resolve, reject) => {
      const sock = net.createConnection(this.sockPath);
      const onError = (err) => {
        sock.destroy();
        reject(err);
      };
      sock.once("error", onError);
      sock.once("connect", () => {
        sock.removeListener("error", onError);
        this.conn = sock;
        this.rxBuf = Buffer.alloc(0);
        sock.on("data", (chunk) => this._onData(chunk));
        sock.on("error", (err) => {
          if (this.conn === sock) this.conn = null;
          this._failPending(err);
        });
        sock.on("close", () => {
          if (this.conn === sock) this.conn = null;
          this._failPending(new Error("clipsd socket closed"));
        });
        resolve();
      });
    });
  }

  _onData(chunk) {
    this.rxBuf = Buffer.concat([this.rxBuf, chunk]);
    while (this.rxBuf.length >= 4) {
      const len = this.rxBuf.readUInt32BE(0);
      if (len > MAX_FRAME) {
        this.conn?.destroy();
        this._failPending(new Error(`clipsd frame too large (${len})`));
        return;
      }
      if (this.rxBuf.length < 4 + len) return;
      const body = this.rxBuf.subarray(4, 4 + len).toString("utf8");
      this.rxBuf = this.rxBuf.subarray(4 + len);
      const next = this.queue.shift();
      if (!next) continue;
      try {
        next.resolve(JSON.parse(body));
      } catch (e) {
        next.reject(e);
      }
    }
  }

  _failPending(err) {
    while (this.queue.length > 0) {
      this.queue.shift().reject(err);
    }
  }

  async call(cmd) {
    await this.ensureRunning();
    const body = Buffer.from(JSON.stringify(cmd), "utf8");
    const hdr = Buffer.allocUnsafe(4);
    hdr.writeUInt32BE(body.length, 0);
    const frame = Buffer.concat([hdr, body]);
    return new Promise((resolve, reject) => {
      if (!this.conn || this.conn.destroyed) {
        reject(new Error("clipsd socket not connected"));
        return;
      }
      this.queue.push({ resolve, reject });
      this.conn.write(frame, (err) => {
        if (err) {
          const idx = this.queue.findIndex((item) => item.resolve === resolve);
          if (idx !== -1) this.queue.splice(idx, 1);
          reject(err);
        }
      });
    });
  }

  shutdownSync() {
    if (this.closed) return;
    this.closed = true;
    try {
      this.conn?.destroy();
    } catch {}
    try {
      if (this.child && this.child.exitCode === null) {
        this.child.kill("SIGTERM");
      }
    } catch {}
    try {
      fs.unlinkSync(this.sockPath);
    } catch {}
  }
}

function attachClipsBridge(VoiceEngine, features) {
  if (process.platform !== "darwin" && process.platform !== "linux") {
    return;
  }

  let clipsModulePath = path.resolve(__dirname, "..", "discord_clips");
  let clipsDataPath = null;
  let recordingEventCb = null;
  let recordingEnabled = false;
  let recordingActive = false;
  let activeSourceId = "";
  let activeWindowId = null;
  let activePid = null;

  const settings = {
    fps: 30,
    width: 1920,
    height: 1080,
    bitrateKbps: 8000,
    durationMs: 30000,
  };

  function resolveBinaryPath() {
    const candidates = [
      path.join(clipsModulePath, "clipsd"),
      path.join(clipsModulePath, "discord_clips"),
      path.resolve(__dirname, "..", "discord_clips", "clipsd"),
      path.resolve(__dirname, "..", "discord_clips", "discord_clips"),
    ];
    for (const c of candidates) {
      if (fs.existsSync(c)) return c;
    }
    return candidates[0];
  }

  const client = new ClipsdClient(resolveBinaryPath);
  let opChain = Promise.resolve();
  const serialize = (fn) => {
    const p = opChain.then(fn, fn);
    opChain = p.catch(() => {});
    return p;
  };

  function emitEvent(code, msg) {
    if (typeof recordingEventCb !== "function") return;
    try {
      recordingEventCb(code, msg ?? "");
    } catch (e) {
      console.error("[clipsd-bridge] recording event callback threw:", e);
    }
  }

  async function startCapture(windowId, pid) {
    if (recordingActive) {
      await client.call({ type: "StopRecording" }).catch(() => {});
      recordingActive = false;
    }
    const ev = await client.call({
      type: "StartRecording",
      fps: Math.max(1, Math.min(120, Math.round(settings.fps || 30))),
      duration_ms: Math.max(1000, Math.round(settings.durationMs || 30000)),
      bitrate_kbps: Math.max(500, Math.round(settings.bitrateKbps || 8000)),
      startup_delay_ms: 0,
      width: settings.width || 1920,
      window_id: windowId ?? null,
      pid: pid ?? null,
    });
    if (ev.type === "RecordingEvent" && ev.state === "started") {
      recordingActive = true;
      activeWindowId = windowId ?? null;
      activePid = pid ?? null;
      emitEvent(RecordingEvent.Started);
      emitEvent(RecordingEvent.RecordingActive);
      emitEvent(RecordingEvent.RecordingHealthy);
      return ev;
    }
    const msg = ev.message || `unexpected reply: ${JSON.stringify(ev)}`;
    recordingActive = false;
    emitEvent(RecordingEvent.Error, msg);
    emitEvent(RecordingEvent.RecordingInactive);
    throw new Error(msg);
  }

  async function stopCapture() {
    activeSourceId = "";
    activeWindowId = null;
    activePid = null;
    if (!recordingActive) return;
    recordingActive = false;
    await client.call({ type: "StopRecording" }).catch(() => {});
    emitEvent(RecordingEvent.Ended);
    emitEvent(RecordingEvent.RecordingInactive);
  }

  VoiceEngine.setClipsModulePath = function (dir) {
    if (typeof dir === "string" && dir.length > 0) {
      clipsModulePath = dir;
    }
    console.info("[clipsd-bridge] setClipsModulePath:", clipsModulePath);
  };

  VoiceEngine.setClipsDataPath = function (dir) {
    clipsDataPath = dir;
  };

  VoiceEngine.setClipsSentryConfig = function () {};
  VoiceEngine.setClipsUIActive = function () {};
  VoiceEngine.setClipsV3MLEnabled = function () {};
  VoiceEngine.setClipsAudioModelOverride = function () {};
  VoiceEngine.setIgnoreGlobalClipDisplay = function () {};
  VoiceEngine.setIgnoreGlobalClipSingleWindow = function () {};

  VoiceEngine.setOnClipsRecordingEvent = function (cb) {
    recordingEventCb = typeof cb === "function" ? cb : null;
  };

  VoiceEngine.setClipsRecordingEnabled = function (enabled) {
    recordingEnabled = !!enabled;
    console.info("[clipsd-bridge] setClipsRecordingEnabled:", recordingEnabled);
    if (!recordingEnabled) {
      serialize(() => stopCapture());
    } else {
      client.ensureRunning().catch((e) => console.warn("[clipsd-bridge] worker warmup failed:", e.message));
    }
  };

  VoiceEngine.setClipBufferLength = function (seconds) {
    const secs = Number(seconds);
    if (!Number.isFinite(secs) || secs <= 0) {
      settings.durationMs = 0;
      serialize(() => stopCapture());
      return;
    }
    settings.durationMs = Math.max(1000, Math.round(secs * 1000));
    if (recordingActive) {
      serialize(() => client.call({ type: "SetDuration", duration_ms: settings.durationMs }).catch(() => {}));
    }
  };

  VoiceEngine.applyClipsSettings = function (opts) {
    if (!opts || typeof opts !== "object") return;
    if (Number.isFinite(opts.frameRate) && opts.frameRate > 0) settings.fps = Math.round(opts.frameRate);
    if (Number.isFinite(opts.width) && opts.width > 0) settings.width = Math.round(opts.width);
    if (Number.isFinite(opts.height) && opts.height > 0) settings.height = Math.round(opts.height);
    if (Number.isFinite(opts.bitrateKbps) && opts.bitrateKbps > 0) {
      settings.bitrateKbps = Math.round(opts.bitrateKbps);
      if (recordingActive) {
        serialize(() => client.call({ type: "SetBitrate", bitrate_kbps: settings.bitrateKbps }).catch(() => {}));
      }
    }
  };

  VoiceEngine.applyClipsQualitySettings = function (width, height, frameRate) {
    if (Number.isFinite(width) && width > 0) settings.width = Math.round(width);
    if (Number.isFinite(height) && height > 0) settings.height = Math.round(height);
    if (Number.isFinite(frameRate) && frameRate > 0) settings.fps = Math.round(frameRate);
  };

  VoiceEngine.setClipsSource = function (source) {
    const id = source && typeof source.id === "string" ? source.id : "";
    console.info("[clipsd-bridge] setClipsSource:", JSON.stringify(source));
    if (!id) {
      serialize(() => stopCapture());
      return;
    }
    activeSourceId = id;
    const windowId = parseWindowId(id);
    const pid =
      Number.isInteger(source?.soundshareId) && source.soundshareId > 0 ? source.soundshareId : null;
    serialize(async () => {
      // On Linux, if we are already recording this exact game PID, keep the ring buffer intact
      // and re-emit readiness events. If a new game PID arrived (e.g. upgrading from the initial
      // portal fallback to a hooked Vulkan game), restart with `pid` so clipsd attaches directly
      // to the game's Vulkan DMA-BUF layer when available.
      if (process.platform === "linux" && recordingActive && (pid == null || pid === activePid)) {
        activeWindowId = windowId;
        emitEvent(RecordingEvent.Started);
        emitEvent(RecordingEvent.RecordingActive);
        emitEvent(RecordingEvent.RecordingHealthy);
        return;
      }
      try {
        await startCapture(windowId, pid);
      } catch (err) {
        console.error("[clipsd-bridge] startCapture failed:", err);
      }
    });
  };

  VoiceEngine.saveClipEx = function (opts, onSuccess, onError) {
    const filepath = opts?.filepath;
    console.info("[clipsd-bridge] saveClipEx:", filepath, "active=", recordingActive);
    if (!filepath) {
      onError?.(JSON.stringify({ errorMessage: "missing filepath", errorAt: "bridge" }));
      return;
    }
    const parsedMeta = parseJsonSafe(opts?.metadata);
    // Discord passes steady-clock timestamps (from getSystemSteadyClockNowMs) in
    // startMs/endMs/trimStartMs/trimEndMs/thumbnailMs, not relative offsets from 0.
    const nowMs = Math.floor(performance.now());
    const startMs = Number(opts?.startMs);
    const endMs = Number(opts?.endMs);
    const trimStartMs = Number(opts?.trimStartMs);
    const trimEndMs = Number(opts?.trimEndMs);

    let lastMs = Math.max(1000, Math.round(settings.durationMs || 30000));
    let trimEndRel = 0;
    if (Number.isFinite(trimStartMs) && Number.isFinite(trimEndMs) && trimEndMs > trimStartMs) {
      lastMs = Math.max(1000, Math.round(trimEndMs - trimStartMs));
      const tailGap = Math.round(nowMs - trimEndMs);
      if (tailGap > 500 && tailGap < 600000) {
        trimEndRel = tailGap;
      }
    } else if (Number.isFinite(startMs) && Number.isFinite(endMs) && endMs > startMs) {
      lastMs = Math.max(1000, Math.round(endMs - startMs));
    }

    let thumbOffsetMs = 0;
    if (opts?.thumbnailMs != null && Number.isFinite(Number(opts.thumbnailMs))) {
      const rawThumb = Number(opts.thumbnailMs);
      const base =
        Number.isFinite(trimStartMs) && trimStartMs > 0
          ? trimStartMs
          : Number.isFinite(startMs) && startMs > 0
            ? startMs
            : 0;
      thumbOffsetMs =
        base > 0 && rawThumb >= base ? Math.max(0, Math.round(rawThumb - base)) : Math.max(0, Math.round(rawThumb));
    }

    const cmd = {
      type: "SaveClip",
      filepath,
      metadata: parsedMeta,
      trim_start_ms: 0,
      trim_end_ms: trimEndRel,
      last_ms: lastMs,
      thumbnail_ms: thumbOffsetMs,
      embed_thumbnail: true,
    };
    serialize(async () => {
      try {
        if (!recordingActive) {
          throw { errorMessage: "buffer warming up (not recording)", errorAt: "buffer_warming_up" };
        }
        const ev = await client.call(cmd);
        console.info("[clipsd-bridge] saveClipEx reply:", ev.type, ev.error || ev.message || "");
        if (ev.type === "ClipSaved") {
          const metaStr = ev.metadata ? JSON.stringify(ev.metadata) : "";
          onSuccess?.(ev.duration_ms ?? 0, ev.stats_json ?? "{}", ev.thumbnail ?? "", metaStr);
          return;
        }
        const errMsg = ev.error || ev.message || `clip save failed (${ev.type})`;
        onError?.(JSON.stringify({ errorMessage: errMsg, errorAt: "clipsd" }));
      } catch (err) {
        console.warn("[clipsd-bridge] saveClipEx failed:", err?.errorMessage || err?.message || err);
        if (err && typeof err === "object" && "errorMessage" in err) {
          onError?.(JSON.stringify(err));
        } else {
          onError?.(JSON.stringify({ errorMessage: String(err?.message || err), errorAt: "bridge" }));
        }
      }
    });
  };

  VoiceEngine.saveScreenshot = function (filepath, metadataJson, _watermarkPath, quality, delayMs, onSuccess, onError) {
    const delay = Math.max(0, Number(delayMs) || 0);
    const q = Math.max(1, Math.min(100, Math.round(Number(quality) || 90)));
    const parsedMeta = parseJsonSafe(metadataJson);
    serialize(async () => {
      try {
        if (delay > 0) {
          await new Promise((r) => setTimeout(r, delay));
        }
        const ev = await client.call({
          type: "TakeScreenshot",
          filepath,
          quality: q,
          metadata: parsedMeta,
          window_id: activeWindowId,
        });
        if (ev.type !== "ScreenshotComplete") {
          throw new Error(ev.message || `screenshot failed (${ev.type})`);
        }
        const bytes = fs.readFileSync(ev.filepath);
        onSuccess?.(bytes);
      } catch (err) {
        onError?.(String(err?.message || err));
      }
    });
  };

  VoiceEngine.updateClipMetadata = function (filepath, metadataJson, onSuccess, onError) {
    const parsedMeta = parseJsonSafe(metadataJson);
    if (!parsedMeta) {
      onError?.("invalid metadata JSON");
      return;
    }
    serialize(async () => {
      try {
        const ev = await client.call({
          type: "UpdateClipMetadata",
          filepath,
          metadata: parsedMeta,
        });
        if (ev.type === "Ack") {
          onSuccess?.();
        } else {
          onError?.(ev.message || `updateClipMetadata failed (${ev.type})`);
        }
      } catch (err) {
        onError?.(String(err?.message || err));
      }
    });
  };

  VoiceEngine.exportClipToFile = function (inputPath, outputPath, _options, onSuccess, onError) {
    try {
      fs.mkdirSync(path.dirname(outputPath), { recursive: true });
      fs.copyFileSync(inputPath, outputPath);
      onSuccess?.(outputPath, false);
    } catch (err) {
      onError?.(String(err?.message || err));
    }
  };

  VoiceEngine.setClipsPerfMonitoring = function () {
    return Promise.resolve();
  };

  VoiceEngine.getSystemSteadyClockNowMs = function () {
    return Math.floor(performance.now());
  };

  if (features && typeof features.declareSupported === "function") {
    features.declareSupported("clips");
    features.declareSupported("clips_thumbnail");
    features.declareSupported("clips_recording_ready_events");
  }

  if (process.env.CLIPSD_TRACE) {
    const names = Object.keys(VoiceEngine).filter((k) => /clip|screenshot/i.test(k) && typeof VoiceEngine[k] === "function");
    console.info("[clipsd-bridge] clip-related VoiceEngine methods:", names.join(","));
    for (const k of names) {
      const orig = VoiceEngine[k];
      if (/^(getSystemSteadyClockNowMs|setClipsPerfMonitoring)$/.test(k)) continue;
      VoiceEngine[k] = function (...args) {
        let a;
        try { a = JSON.stringify(args.map((x) => (typeof x === "function" ? "[fn]" : x))).slice(0, 300); } catch { a = "?"; }
        console.info("[clipsd-bridge] call", k, a);
        return orig.apply(this, args);
      };
    }
  }

  if (typeof globalThis.window !== "undefined") {
    globalThis.window.__clipsdBridge = {
      stats: () => client.call({ type: "GetStats" }),
      startDisplay: () => serialize(() => startCapture(null)),
      stop: () => serialize(() => stopCapture()),
      getState: () => ({
        platform: process.platform,
        clipsModulePath,
        clipsDataPath,
        recordingEnabled,
        recordingActive,
        activeSourceId,
        activeWindowId,
        settings: { ...settings },
        sockPath: client.sockPath,
      }),
    };
  }

  console.info(`[clipsd-bridge] ${process.platform} Clips bridge attached to VoiceEngine`);
}

module.exports = attachClipsBridge;
