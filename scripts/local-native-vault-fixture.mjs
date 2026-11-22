// Only synthetic credentials in a new, explicitly selected F: lab. Never use user identities.
import fs from 'node:fs';
import path from 'node:path';
import assert from 'node:assert/strict';
import {spawnSync} from 'node:child_process';
export function createSyntheticVault(lab) {
  assert(path.resolve(lab).replaceAll('\\','/').startsWith('F:/HomeBase-Labs/'));
  const vault=path.join(lab,'secrets.yaml'), meta=path.join(lab,'secrets-meta.json');
  assert(!fs.existsSync(vault));
  const identity=path.join(lab,'.age-keys/primary.txt');fs.mkdirSync(path.dirname(identity),{recursive:true});
  const env={...process.env,HOME:lab,USERPROFILE:lab,XDG_CONFIG_HOME:path.join(lab,'.config'),SOPS_AGE_KEY_FILE:identity,HOMEBASE_SOPS_AGE_KEY_FILE:identity};
  for(const key of Object.keys(env)) if(/(?:SOPS|HOMEBASE_SOPS)_AGE_(?:KEY|KEY_CMD|SSH_PRIVATE_KEY_FILE|SSH_PRIVATE_KEY_CMD)$/.test(key))delete env[key];
  const age='C:/HomeBase/Tools/age/age-keygen.exe',sops='C:/HomeBase/Tools/sops/bin/sops.exe';
  function run(exe,args){const r=spawnSync(exe,args,{env,encoding:'utf8',windowsHide:true,timeout:60000});assert.equal(r.status,0,`Synthetic ${path.basename(exe)} fixture failed: ${r.error?.code||r.signal||r.status}`);return r.stdout;}
  run(age,['-o',identity]);const recipient=run(age,['-y',identity]).trim();assert.match(recipient,/^age1[a-z0-9]+$/);
  const config=path.join(lab,'.sops.yaml');fs.writeFileSync(config,`creation_rules:\n  - path_regex: .*\n    age: ${recipient}\n`);
  const values={fixture_token:'SYNTHETIC-SOPS-FIRST-ONLY',openrouter_key:'SYNTHETIC-OTHER-SOPS-SOURCE-ONLY'};
  const plain=path.join(lab,'synthetic-vault-input.json');fs.writeFileSync(plain,JSON.stringify(values));
  fs.writeFileSync(vault,run(sops,['--config',config,'--encrypt','--age',recipient,'--input-type','json','--output-type','yaml',plain]));
  fs.writeFileSync(meta,JSON.stringify({fixture_token:{revision:0,expiry_policy:'block'},openrouter_key:{revision:0,expiry_policy:'block',expires_at:'2020-01-01T00:00:00Z'}}));
  fs.writeFileSync(path.join(lab,'synthetic-key-manager.json'),JSON.stringify({value:'SYNTHETIC-API-MANAGER-ONLY'}));
  function rotateToken(value){
    assert(value.startsWith('SYNTHETIC-'));
    fs.writeFileSync(plain,JSON.stringify({...values,fixture_token:value}));
    fs.writeFileSync(vault,run(sops,['--config',config,'--encrypt','--age',recipient,'--input-type','json','--output-type','yaml',plain]));
  }
  return {env:{...env,HOMEBASE_SECRETS_FILE:vault,HOMEBASE_SECRETS_META:meta,HOMEBASE_SOPS_CONFIG:config,HOMEBASE_SOPS_EXE:sops},values,rotateToken};
}
