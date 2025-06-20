#!/usr/bin/env node
import path from "path";
import { fileURLToPath, pathToFileURL } from "url";

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);

const cliPath = path.resolve(__dirname, "../dist/cli.js");
const cliUrl = pathToFileURL(cliPath).href;

import(cliUrl).catch(err => {
  console.error("Failed to load Codex CLI:", err);
  process.exit(1);
});
