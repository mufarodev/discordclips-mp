"use strict";

const Module = require("module");
const path = require("path");
const electron = require("electron");

function beforeCoreLoad(clipsParentDir) {
  if (!Module.globalPaths.includes(clipsParentDir)) {
    Module.globalPaths.push(clipsParentDir);
  }

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
