#!/usr/bin/env node
"use strict";

const { runInstaller } = require("../lib/sdk");

try {
  process.exit(runInstaller(process.argv.slice(2)));
} catch (error) {
  console.error(`install-incan-sdk: ${error.message}`);
  process.exit(1);
}
