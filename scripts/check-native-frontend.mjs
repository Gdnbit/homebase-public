// Production HomeBase must embed its Vite directory, never navigate to a URL.
import fs from 'node:fs';
import path from 'node:path';
import assert from 'node:assert/strict';
import {fileURLToPath} from 'node:url';

export function validateFrontendDist(value, root) {
  let directory;
  if (typeof value === 'string') {
    assert(!/^[a-z][a-z0-9+.-]*:/i.test(value) && !/^[\\/]{2}/.test(value),
      'HomeBase frontendDist must embed a directory, not a URL or ambiguous Windows path. Use [absoluteDirectory] for a cross-drive build.');
    directory=value;
  } else {
    assert(Array.isArray(value) && value.length === 1 && typeof value[0] === 'string',
      'HomeBase frontendDist must contain one complete frontend directory.');
    directory=value[0];
    assert(!/^(?:https?|file|tauri):/i.test(directory), 'The frontend file list must contain a filesystem directory, not a URL.');
  }
  assert(directory.length > 0, 'HomeBase frontend directory is empty.');
  const resolved=path.resolve(root,directory);
  assert(fs.statSync(resolved).isDirectory(), 'HomeBase frontend directory is missing.');
  assert(fs.statSync(path.join(resolved,'index.html')).isFile(), 'HomeBase frontend index.html is missing.');
  return resolved;
}

export function configuredFrontend(root, environment) {
  const platform={windows:'windows',linux:'linux',macos:'macos',android:'android',ios:'ios'}[environment.CARGO_CFG_TARGET_OS];
  const configs=[JSON.parse(fs.readFileSync(path.join(root,'tauri.conf.json'),'utf8'))];
  const platformFile=platform&&path.join(root,`tauri.${platform}.conf.json`);
  if(platformFile&&fs.existsSync(platformFile))configs.push(JSON.parse(fs.readFileSync(platformFile,'utf8')));
  if(environment.TAURI_CONFIG)configs.push(JSON.parse(environment.TAURI_CONFIG));
  let selected;
  for(const config of configs){
    if(Object.hasOwn(config,'build')&&config.build===null)selected=undefined;
    else if(config.build&&Object.hasOwn(config.build,'frontendDist'))selected=config.build.frontendDist;
  }
  return validateFrontendDist(selected,root);
}

if(process.argv[1]&&path.resolve(process.argv[1])===fileURLToPath(import.meta.url)){
  if(process.env.CARGO_FEATURE_CUSTOM_PROTOCOL){
    const root=fileURLToPath(new URL('../src-tauri/',import.meta.url));
    configuredFrontend(root,process.env);
    console.log('Native frontend directory verified for embedded HomeBase assets.');
  }else console.log('Development frontend: embedded-asset check does not apply.');
}
