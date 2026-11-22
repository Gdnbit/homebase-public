#!/usr/bin/env node
// External workflow adapter. Does not start/restart HomeBase or bypass its control contract.
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';

const home = process.env.AGENT_AUTOLOOP_HOME || path.join(os.homedir(), '.agent-autoloop');
try {
  const registration = JSON.parse(fs.readFileSync(path.join(home, 'workbench/entry.json'), 'utf8'));
  for (const file of [registration.node, registration.entry]) {
    if (!path.isAbsolute(file) || !fs.statSync(file).isFile()) throw new Error('Invalid local workflow registration');
  }
  const child = spawnSync(registration.node, [registration.entry, ...process.argv.slice(2)], {
    stdio: 'inherit', windowsHide: true,
  });
  if (child.error) throw child.error;
  process.exitCode = child.status ?? 1;
} catch (error) {
  console.error(JSON.stringify({ ok: false, code: 'AUTOLOOP_ADAPTER_UNAVAILABLE', message: error.message }));
  process.exitCode = 1;
}
