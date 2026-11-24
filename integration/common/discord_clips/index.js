"use strict";

// Clean-room macOS/Linux discord_clips module shim.
// Exports `getModulePath()` returning the directory containing the `clipsd` / `discord_clips` worker binary.
module.exports = {
  getModulePath() {
    return __dirname;
  },
};
