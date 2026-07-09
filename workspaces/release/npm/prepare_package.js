#!/usr/bin/env node

const childProcess = require("child_process");
const fs = require("fs");
const path = require("path");

function fail(message) {
  console.error(`prepare npm package: ${message}`);
  process.exit(1);
}

function usage() {
  console.log(`Prepare the npm toolchain packages.

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
const versionPath = path.join(distDir, "toolchain-version.txt");
const version = fs.readFileSync(versionPath, "utf8").split(/\r?\n/)[0].trim();
if (!version) {
  fail(`empty toolchain version in ${versionPath}`);
}
const releasePath = path.join(distDir, "toolchain-release.txt");
const release = fs.existsSync(releasePath)
  ? fs.readFileSync(releasePath, "utf8").split(/\r?\n/)[0].trim()
  : `v${version}`;
if (!release) {
  fail(`empty toolchain release in ${releasePath}`);
}

const platformPackages = [
  {
    target: "x86_64-unknown-linux-gnu",
    packageName: "@incan/toolchain-linux-x64",
    os: "linux",
    cpu: "x64",
  },
  {
    target: "x86_64-apple-darwin",
    packageName: "@incan/toolchain-darwin-x64",
    os: "darwin",
    cpu: "x64",
  },
  {
    target: "aarch64-apple-darwin",
    packageName: "@incan/toolchain-darwin-arm64",
    os: "darwin",
    cpu: "arm64",
  },
];

function packageTarball(target) {
  const archive = path.join(distDir, `incan-${release}-${target}.tar.gz`);
  if (!fs.existsSync(archive)) {
    fail(`missing toolchain archive for ${target}: ${archive}`);
  }
  return archive;
}

function packageReadme(platformPackage) {
  return `# ${platformPackage.packageName}

This package contains the prebuilt Incan toolchain payload for \`${platformPackage.target}\`.
It is installed as an optional dependency of \`@incan/toolchain\` and is not intended to be used directly.

See https://incan.io for documentation.
`;
}

const stageDir = path.join(distDir, "_npm-package");
const platformRoot = path.join(distDir, "_npm-platform-packages");
fs.rmSync(stageDir, { recursive: true, force: true });
fs.rmSync(platformRoot, { recursive: true, force: true });
fs.cpSync(packageDir, stageDir, {
  recursive: true,
  filter: (source) => {
    const name = path.basename(source);
    return (
      name !== "node_modules" &&
      !source.includes(`${path.sep}node_modules${path.sep}`) &&
      name !== "prepare_package.js"
    );
  },
});

const vendorDir = path.join(stageDir, "vendor");
fs.mkdirSync(vendorDir, { recursive: true });
fs.copyFileSync(
  path.join(repoRoot, "workspaces/release/install-incan.sh"),
  path.join(vendorDir, "install-incan.sh"),
);

const packageJsonPath = path.join(stageDir, "package.json");
const packageJson = JSON.parse(fs.readFileSync(packageJsonPath, "utf8"));
packageJson.version = version;
delete packageJson.scripts;
packageJson.optionalDependencies = Object.fromEntries(
  platformPackages.map((platformPackage) => [platformPackage.packageName, version]),
);
fs.writeFileSync(packageJsonPath, `${JSON.stringify(packageJson, null, 2)}\n`);

for (const platformPackage of platformPackages) {
  const platformDir = path.join(platformRoot, platformPackage.target);
  const toolchainDir = path.join(platformDir, "toolchain");
  fs.mkdirSync(toolchainDir, { recursive: true });
  childProcess.execFileSync("tar", ["-xzf", packageTarball(platformPackage.target), "-C", toolchainDir], {
    stdio: "inherit",
  });
  fs.writeFileSync(path.join(platformDir, "README.md"), packageReadme(platformPackage));
  fs.writeFileSync(
    path.join(platformDir, "package.json"),
    `${JSON.stringify(
      {
        name: platformPackage.packageName,
        version,
        description: `Prebuilt Incan toolchain payload for ${platformPackage.target}`,
        license: packageJson.license,
        homepage: packageJson.homepage,
        repository: packageJson.repository,
        os: [platformPackage.os],
        cpu: [platformPackage.cpu],
        files: ["README.md", "toolchain"],
        engines: packageJson.engines,
      },
      null,
      2,
    )}\n`,
  );
}

if (!skipPack) {
  for (const platformPackage of platformPackages) {
    childProcess.execFileSync(
      "npm",
      ["pack", path.join(platformRoot, platformPackage.target), "--pack-destination", distDir],
      {
        stdio: "inherit",
      },
    );
  }
  childProcess.execFileSync("npm", ["pack", stageDir, "--pack-destination", distDir], {
    stdio: "inherit",
  });
}
