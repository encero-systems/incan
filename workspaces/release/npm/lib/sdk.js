"use strict";

const fs = require("fs");
const path = require("path");
const childProcess = require("child_process");

function packageRoot() {
  return path.resolve(__dirname, "..");
}

function sdkHome() {
  return process.env.INCAN_NPM_SDK_HOME || path.join(packageRoot(), ".incan", "home");
}

function binDir() {
  return process.env.INCAN_NPM_BIN_DIR || path.join(packageRoot(), ".incan", "bin");
}

function installerScript() {
  const candidates = [
    path.join(packageRoot(), "vendor", "install-incan-sdk.sh"),
    path.resolve(packageRoot(), "..", "install-incan-sdk.sh"),
  ];
  for (const candidate of candidates) {
    if (fs.existsSync(candidate)) {
      return candidate;
    }
  }
  throw new Error("could not find bundled install-incan-sdk.sh");
}

function hasValueOption(args, name) {
  return args.includes(name) || args.some((arg) => arg.startsWith(`${name}=`));
}

function installerArgs(args) {
  const next = args.filter((arg) => arg !== "--package-install");
  if (!hasValueOption(next, "--incan-home")) {
    next.push("--incan-home", sdkHome());
  }
  if (!hasValueOption(next, "--bin-dir")) {
    next.push("--bin-dir", binDir());
  }
  return next;
}

function runInstaller(args, options = {}) {
  if (args.includes("--package-install") && process.env.INCAN_SKIP_NPM_INSTALL === "1") {
    return 0;
  }
  const result = childProcess.spawnSync("bash", [installerScript(), ...installerArgs(args)], {
    stdio: options.stdio || "inherit",
    env: process.env,
  });
  if (result.error) {
    throw result.error;
  }
  return result.status === null ? 1 : result.status;
}

function commandPath(command) {
  return path.join(binDir(), command);
}

function ensureCommand(command) {
  if (!fs.existsSync(commandPath(command))) {
    const status = runInstaller([]);
    if (status !== 0) {
      process.exit(status);
    }
  }
}

function runCommand(command, args) {
  ensureCommand(command);
  const child = childProcess.spawn(commandPath(command), args, {
    stdio: "inherit",
    env: process.env,
  });
  child.on("error", (error) => {
    console.error(error.message);
    process.exit(1);
  });
  child.on("exit", (code, signal) => {
    if (signal) {
      process.kill(process.pid, signal);
    }
    process.exit(code === null ? 1 : code);
  });
}

module.exports = {
  runCommand,
  runInstaller,
};
