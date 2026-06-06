#!/usr/bin/env node

const childProcess = require("child_process");
const fs = require("fs");
const path = require("path");

function fail(message) {
  console.error(`prepare npm package: ${message}`);
  process.exit(1);
}

function usage() {
  console.log(`Prepare the npm thin installer package.

Usage:
  prepare_package.js <dist-dir> [--skip-pack]
`);
}

const args = process.argv.slice(2);
if (args.includes("-h") || args.includes("--help")) {
  usage();
  process.exit(0);
}
const distArg = args.find((arg) => !arg.startsWith("--"));
if (!distArg) {
  fail("missing dist directory");
}
const skipPack = args.includes("--skip-pack");

const packageDir = __dirname;
const repoRoot = path.resolve(packageDir, "../../..");
const distDir = path.resolve(process.cwd(), distArg);
const versionPath = path.join(distDir, "sdk-version.txt");
const version = fs.readFileSync(versionPath, "utf8").split(/\r?\n/)[0].trim();
if (!version) {
  fail(`empty SDK version in ${versionPath}`);
}

const stageDir = path.join(distDir, "_npm-package");
fs.rmSync(stageDir, { recursive: true, force: true });
fs.cpSync(packageDir, stageDir, {
  recursive: true,
  filter: (source) => {
    const name = path.basename(source);
    return (
      name !== "node_modules" &&
      !source.includes(`${path.sep}node_modules${path.sep}`) &&
      name !== "prepare_package.js" &&
      name !== "publish_package.sh"
    );
  },
});

const vendorDir = path.join(stageDir, "vendor");
fs.mkdirSync(vendorDir, { recursive: true });
fs.copyFileSync(
  path.join(repoRoot, "workspaces/release/install-incan-sdk.sh"),
  path.join(vendorDir, "install-incan-sdk.sh"),
);

const packageJsonPath = path.join(stageDir, "package.json");
const packageJson = JSON.parse(fs.readFileSync(packageJsonPath, "utf8"));
packageJson.version = version;
fs.writeFileSync(packageJsonPath, `${JSON.stringify(packageJson, null, 2)}\n`);

if (!skipPack) {
  childProcess.execFileSync("npm", ["pack", stageDir, "--pack-destination", distDir], {
    stdio: "inherit",
  });
}
