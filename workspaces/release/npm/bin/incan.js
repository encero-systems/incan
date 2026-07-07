#!/usr/bin/env node
"use strict";

const { runCommand } = require("../lib/toolchain");

runCommand("incan", process.argv.slice(2));
