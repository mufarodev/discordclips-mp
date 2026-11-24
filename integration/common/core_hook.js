"use strict";

// Clean-room main-process hook for in-place macOS/Linux injection (Vencord-style).
// Loaded by discord_desktop_core/index.js without modifying the host app binary or signature.
// Makes `ensureModule("discord_clips")`, `requireModule("discord_clips")`, and
// `getModulePath("discord_clips")` resolve to our local clipsd module directory.

const Module = require("module");
const path = require("path");
const electron = require("electron");

function beforeCoreLoad(clipsParentDir) {
  // 1. Add clipsParentDir to Node's global module search paths before core.asar loads
  //    so preload's `nativeModuleLoader` allows and resolves `requireModule("discord_clips")`.
  if (!Module.globalPaths.includes(clipsParentDir)) {
    Module.globalPaths.push(clipsParentDir);
  }

  // 2. Wrap ipcMain.handle before core.asar registers DISCORD_NATIVE_MODULES_INSTALL.
  try {
    const ipc = electron?.ipcMain;
    if (ipc && typeof ipc.handle === "function" && !ipc.__clipsdHandleHooked) {
      const origHandle = ipc.handle.bind(ipc);
      ipc.handle = function (channel, listener) {
        if (channel === "DISCORD_NATIVE_MODULES_INSTALL") {
          return origHandle(channel, (event, moduleName, ...rest) => {
            if (moduleName === "discord_clips") return Promise.resolve();
            return listener(event, moduleName, ...rest);
          });
        }
        return origHandle(channel, listener);
      };
      ipc.__clipsdHandleHooked = true;
    }
  } catch (e) {
    console.warn("[clipsd-core-hook] ipcMain patch failed:", e);
  }
}

function afterCoreLoad(core, clipsParentDir) {
  const clipsDir = path.join(clipsParentDir, "discord_clips");

  // 3. Patch `updater` inside `core.startup(bootstrapModules)` so `discord_clips`
  //    is treated as already installed and committed (for `getModulePath("discord_clips")`
  //    and `renderMontageRecipe`), never hitting Discord's CDN.
  if (core && typeof core.startup === "function" && !core.__clipsdHooked) {
    const origStartup = core.startup.bind(core);
    core.startup = function (bootstrapModules) {
      try {
        const u = bootstrapModules?.updater?.getUpdater?.();
        if (u) {
          u.committedModules?.add?.("discord_clips");
          u.committedModulePaths?.set?.("discord_clips", clipsDir);
          if (typeof u.installModule === "function" && !u.__clipsdInstallHooked) {
            const origInstall = u.installModule.bind(u);
            u.installModule = function (name, progressCb) {
              if (name === "discord_clips") {
                u.committedModules?.add?.("discord_clips");
                u.committedModulePaths?.set?.("discord_clips", clipsDir);
                return Promise.resolve();
              }
              return origInstall(name, progressCb);
            };
            u.__clipsdInstallHooked = true;
          }
          if (typeof u.installModuleWithOptions === "function" && !u.__clipsdInstallOptsHooked) {
            const origInstallOpts = u.installModuleWithOptions.bind(u);
            u.installModuleWithOptions = function (name, opts, progressCb) {
              if (name === "discord_clips") {
                u.committedModules?.add?.("discord_clips");
                u.committedModulePaths?.set?.("discord_clips", clipsDir);
                return Promise.resolve();
              }
              return origInstallOpts(name, opts, progressCb);
            };
            u.__clipsdInstallOptsHooked = true;
          }
        }
      } catch (e) {
        console.warn("[clipsd-core-hook] updater patch failed:", e);
      }
      console.info("[clipsd-core-hook] registered local discord_clips at", clipsDir);
      return origStartup(bootstrapModules);
    };
    core.__clipsdHooked = true;
  }

  return core;
}

module.exports = { beforeCoreLoad, afterCoreLoad };
