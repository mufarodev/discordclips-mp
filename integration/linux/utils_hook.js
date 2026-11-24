"use strict";

// Linux Clips helper for Discord's discord_utils module.
// On Linux, discord_utils.node leaves `windowHandle` unset on detected games and returns ""
// from `getWindowHandleFromPid`. Discord's ClipsManager (`fireClipsInitEvent`) and saveClip
// action both require a non-null `windowHandle` on the visible game to dispatch `CLIPS_INIT`,
// set the clips source, and allow saving clips/screenshots.
//
// Synthesize a deterministic, non-colliding positive 32-bit handle for any detected game PID
// whose native `windowHandle` is missing/zero, and map it back in `getPidFromWindowHandle`
// and `getWindowHandleFromPid`.

const SYNTH_BASE = 0x70000000;

function attachUtilsHook(nativeUtils) {
  if (process.platform !== "linux" || !nativeUtils || nativeUtils.__clipsdUtilsHooked) {
    return;
  }
  nativeUtils.__clipsdUtilsHooked = true;

  const handleToPid = new Map();
  const pidToHandle = new Map();

  const origGetWindowHandleFromPid =
    typeof nativeUtils.getWindowHandleFromPid === "function"
      ? nativeUtils.getWindowHandleFromPid.bind(nativeUtils)
      : null;
  const origGetPidFromWindowHandle =
    typeof nativeUtils.getPidFromWindowHandle === "function"
      ? nativeUtils.getPidFromWindowHandle.bind(nativeUtils)
      : null;

  function synthHandle(pid) {
    const p = Number(pid);
    if (!Number.isInteger(p) || p <= 0) return null;
    const handle = String(SYNTH_BASE + (p & 0x0fffffff));
    handleToPid.set(handle, p);
    pidToHandle.set(p, handle);
    return handle;
  }

  function ensureGameHandles(games) {
    if (!Array.isArray(games)) return games;
    let patched = 0;
    const out = games.map((g) => {
      if (!g || typeof g !== "object") return g;
      if (g.windowHandle != null && g.windowHandle !== "" && g.windowHandle !== "0") {
        return g;
      }
      const pid = g.pid;
      let handle = null;
      if (origGetWindowHandleFromPid && pid) {
        try {
          const real = origGetWindowHandleFromPid(pid);
          if (real && real !== "0") handle = String(real);
        } catch {}
      }
      if (!handle) {
        handle = synthHandle(pid);
      }
      if (!handle) return g;
      patched += 1;
      try {
        g.windowHandle = handle;
        if (g.windowHandle === handle) return g;
      } catch {}
      return { ...g, windowHandle: handle };
    });
    if (patched > 0) {
      console.info(`[clipsd-utils-hook] synthesized windowHandle for ${patched} detected game(s)`);
    }
    return out;
  }

  if (origGetWindowHandleFromPid) {
    nativeUtils.getWindowHandleFromPid = function (pid) {
      const real = origGetWindowHandleFromPid(pid);
      if (real && real !== "0") return real;
      const p = Number(pid);
      return pidToHandle.get(p) ?? "";
    };
  }

  if (origGetPidFromWindowHandle) {
    nativeUtils.getPidFromWindowHandle = function (handle) {
      const key = String(handle ?? "");
      if (handleToPid.has(key)) return handleToPid.get(key);
      return origGetPidFromWindowHandle(handle);
    };
  }

  for (const fnName of ["setObservedGamesCallback2", "setObservedGamesCallback"]) {
    const orig = nativeUtils[fnName];
    if (typeof orig !== "function") continue;
    nativeUtils[fnName] = function (games, cb, ...rest) {
      if (typeof cb !== "function") {
        return orig.call(this, games, cb, ...rest);
      }
      return orig.call(
        this,
        games,
        function (detected, ...cbRest) {
          return cb.call(this, ensureGameHandles(detected), ...cbRest);
        },
        ...rest
      );
    };
  }

  console.info("[clipsd-utils-hook] Linux windowHandle shim attached to discord_utils");
}

module.exports = attachUtilsHook;
