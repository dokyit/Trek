#!/usr/bin/env node
// Every *.test.mjs in this folder, through Node's test runner: `node script/site/test/run.mjs`.
// Named one by one, as Node 20 takes a folder but not a pattern and Node 22 the other way round.

import { spawnSync } from 'node:child_process';
import { readdirSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const files = readdirSync(here)
  .filter((f) => f.endsWith('.test.mjs'))
  .sort()
  .map((f) => join(here, f));
const run = spawnSync(process.execPath, ['--test', ...files], { stdio: 'inherit' });
process.exit(run.status ?? 1);
