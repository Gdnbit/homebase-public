// Prints a scoped apply_patch patch; does not rewrite source encoding or line endings.
import fs from 'node:fs';
import path from 'node:path';
let patch = '*** Begin Patch\n';
for (const module of ['gdnbit', 'wiki_agent', 'studio']) {
  const file = `src-tauri/src/commands/${module}.rs`; const source = fs.readFileSync(file, 'utf8');
  patch += `*** Update File: ${path.resolve(file).replaceAll('\\','/')}\n@@\n-use tauri::{AppHandle, Emitter};\n+use tauri::AppHandle;\n+use crate::task_events::TrackedEmitter;\n`;
  for (const line of source.split(/\r?\n/).filter(line => /\bapp(?:2|_clone)?\.emit\(/.test(line))) {
    patch += `@@\n-${line}\n+${line.replace('.emit(', '.emit_tracked(')}\n`;
  }
}
patch += '*** End Patch';
process.stdout.write(JSON.stringify(patch));
