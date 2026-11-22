import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const read = (name) =>
  readFileSync(new URL(`../${name}`, import.meta.url), "utf8");
const manifest = JSON.parse(read("package.json"));
const lock = JSON.parse(read("package-lock.json"));
const crates = new Map();
for (const block of read("src-tauri/Cargo.lock").split("[[package]]")) {
  const name = block.match(/^name = "([^"]+)"/m)?.[1];
  const version = block.match(/^version = "([^"]+)"/m)?.[1];
  if (name && version) crates.set(name, version);
}
const minor = (version) => version.split(".").slice(0, 2).join(".");
let checked = 0;
for (const name of Object.keys(manifest.dependencies)) {
  if (!name.startsWith("@tauri-apps/")) continue;
  const crate =
    name === "@tauri-apps/api"
      ? "tauri"
      : name.replace("@tauri-apps/", "tauri-");
  const native = crates.get(crate);
  const frontend = lock.packages[`node_modules/${name}`]?.version;
  assert.ok(
    native && frontend,
    `Missing locked versions for ${name} / ${crate}`,
  );
  assert.equal(
    minor(frontend),
    minor(native),
    `${name} ${frontend} must match ${crate} ${native} at major/minor`,
  );
  checked++;
}
assert.ok(checked > 0);
console.log(`Tauri frontend/native versions aligned (${checked} packages)`);
