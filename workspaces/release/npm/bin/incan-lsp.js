#!/usr/bin/env node
"use strict";

const { runCommand } = require("../lib/sdk");

runCommand("incan-lsp", process.argv.slice(2));
