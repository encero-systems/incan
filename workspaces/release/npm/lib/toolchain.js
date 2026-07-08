"use strict";

const fs = require("fs");
const path = require("path");
const childProcess = require("child_process");

const HOST_PACKAGES = {
  "x86_64-unknown-linux-gnu": {
    packageName: "@incan/toolchain-linux-x64",
    os: "linux",
    arch: "x64",
  },
  "x86_64-apple-darwin": {
    packageName: "@incan/toolchain-darwin-x64",
    os: "darwin",
    arch: "x64",
  },
  "aarch64-apple-darwin": {
    packageName: "@incan/toolchain-darwin-arm64",
    os: "darwin",
    arch: "arm64",
  },
};

function packageRoot() {
  return path.resolve(__dirname, "..");
}

function toolchainHome() {
  return process.env.INCAN_NPM_TOOLCHAIN_HOME || path.join(packageRoot(), ".incan", "home");
}

function binDir() {
  return process.env.INCAN_NPM_BIN_DIR || path.join(packageRoot(), ".incan", "bin");
}

function hostTarget() {
  if (process.env.INCAN_NPM_HOST_TARGET) {
    return process.env.INCAN_NPM_HOST_TARGET;
  }
  for (const [target, config] of Object.entries(HOST_PACKAGES)) {
    if (process.platform === config.os && process.arch === config.arch) {
      return target;
    }
  }
  return `${process.arch}-${process.platform}`;
}

function supportedTargets() {
  return Object.keys(HOST_PACKAGES).join(", ");
}

function platformPackageRoot(target) {
  const config = HOST_PACKAGES[target];
  if (!config) {
    throw new Error(`unsupported npm toolchain target: ${target}; supported targets: ${supportedTargets()}`);
  }
  try {
    return path.dirname(require.resolve(`${config.packageName}/package.json`, { paths: [packageRoot()] }));
  } catch (error) {
    if (error && error.code === "MODULE_NOT_FOUND") {
      throw new Error(
        `missing npm toolchain package ${config.packageName} for ${target}; reinstall @incan/toolchain or install ${config.packageName}`,
      );
    }
    throw error;
  }
}

function packageVersion() {
  const packageJson = JSON.parse(fs.readFileSync(path.join(packageRoot(), "package.json"), "utf8"));
  return packageJson.version;
}

function packageManifestUrl() {
  const release = `v${packageVersion()}`;
  return `https://github.com/encero-systems/incan/releases/download/${release}/manifest.json`;
}

function installerScript() {
  const candidates = [
    path.join(packageRoot(), "vendor", "install-incan.sh"),
    path.resolve(packageRoot(), "..", "install-incan.sh"),
  ];
  for (const candidate of candidates) {
    if (fs.existsSync(candidate)) {
      return candidate;
    }
  }
  throw new Error("could not find bundled install-incan.sh");
}

function hasValueOption(args, name) {
  return args.includes(name) || args.some((arg) => arg.startsWith(`${name}=`));
}

function installerArgs(args) {
  const next = args.filter((arg) => arg !== "--package-install");
  if (!hasValueOption(next, "--manifest") && !process.env.INCAN_TOOLCHAIN_MANIFEST) {
    next.push("--manifest", packageManifestUrl());
  }
  if (!hasValueOption(next, "--incan-home")) {
    next.push("--incan-home", toolchainHome());
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
  if (process.env.INCAN_NPM_TOOLCHAIN_DIR) {
    return path.join(process.env.INCAN_NPM_TOOLCHAIN_DIR, "bin", command);
  }
  return path.join(platformPackageRoot(hostTarget()), "toolchain", "bin", command);
}

function runCommand(command, args) {
  let executable;
  try {
    executable = commandPath(command);
  } catch (error) {
    console.error(error.message);
    process.exit(1);
  }
  if (!fs.existsSync(executable)) {
    console.error(`missing ${command} binary in npm toolchain package: ${executable}`);
    process.exit(1);
  }
  const child = childProcess.spawn(executable, args, {
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
