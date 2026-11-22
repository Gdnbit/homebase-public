import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import test from 'node:test';
import assert from 'node:assert/strict';
import {validateFrontendDist,configuredFrontend} from './check-native-frontend.mjs';

function fixture(t){
  const root=fs.mkdtempSync(path.join(os.tmpdir(),'hb-embedded-frontend-'));
  fs.mkdirSync(path.join(root,'frontend'));fs.writeFileSync(path.join(root,'frontend/index.html'),'<!doctype html><div id="root"></div>');
  fs.writeFileSync(path.join(root,'tauri.conf.json'),JSON.stringify({build:{frontendDist:'./frontend'}}));
  t.diagnostic(root);return root;
}
test('production refuses file URLs, remote URLs and ambiguous Windows drive strings',t=>{
  const root=fixture(t);
  for(const value of ['file:///F:/frontend/','https://example.org/','http://localhost:1420/','F:/frontend','F:\\frontend','\\\\?\\F:\\frontend']){
    assert.throws(()=>validateFrontendDist(value,root),/must embed a directory/);
  }
});
test('cross-drive directory arrays and normal relative frontend paths retain the whole directory',t=>{
  const root=fixture(t),directory=path.join(root,'frontend');
  assert.equal(validateFrontendDist([directory],root),directory);
  assert.equal(validateFrontendDist('./frontend',root),directory);
  assert.equal(validateFrontendDist('frontend',root),directory);
});
test('missing index, a single HTML file and URL-shaped file entries cannot pass',t=>{
  const root=fixture(t);fs.mkdirSync(path.join(root,'empty'));
  assert.throws(()=>validateFrontendDist('./empty',root),/ENOENT/);
  assert.throws(()=>validateFrontendDist(['frontend/index.html'],root),/directory is missing/);
  assert.throws(()=>validateFrontendDist(['file:///F:/frontend'],root),/not a URL/);
  for(const value of [null,[],{},['frontend','empty']])assert.throws(()=>validateFrontendDist(value,root));
});
test('platform and inline frontend overrides are checked, including null and unsafe partial overrides',t=>{
  const root=fixture(t),environment={CARGO_CFG_TARGET_OS:'windows'};
  assert.equal(configuredFrontend(root,environment),path.join(root,'frontend'));
  fs.writeFileSync(path.join(root,'tauri.windows.conf.json'),JSON.stringify({build:{frontendDist:'file:///F:/frontend/'}}));
  assert.throws(()=>configuredFrontend(root,environment),/must embed a directory/);
  assert.equal(configuredFrontend(root,{...environment,TAURI_CONFIG:JSON.stringify({build:{frontendDist:[path.join(root,'frontend')]}})}),path.join(root,'frontend'));
  assert.throws(()=>configuredFrontend(root,{...environment,TAURI_CONFIG:'{"build":null}'}));
  assert.throws(()=>configuredFrontend(root,{...environment,TAURI_CONFIG:'{"build":{"frontendDist":null}}'}));
});
