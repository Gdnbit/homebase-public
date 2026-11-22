import fs from "node:fs";
import path from "node:path";
import crypto from "node:crypto";
import { fileURLToPath } from "node:url";
import { enrichCommands, referenceMarkdown } from "./control-interface-docs.mjs";

export const root = fileURLToPath(new URL("../", import.meta.url));
const camel = (value) =>
  value.replace(/_([a-z])/g, (_, letter) => letter.toUpperCase());
function splitArgs(text) {
  let depth = 0,
    start = 0;
  const result = [];
  for (let i = 0; i < text.length; i++) {
    if ("<([{".includes(text[i])) depth++;
    if (">)]}".includes(text[i])) depth--;
    if (text[i] === "," && depth === 0) {
      result.push(text.slice(start, i));
      start = i + 1;
    }
  }
  result.push(text.slice(start));
  if (depth !== 0) throw new Error("Unsupported unbalanced Rust arguments");
  return result.map((value) => value.trim()).filter(Boolean);
}

export function contract(projectRoot = root) {
  const lib = fs.readFileSync(
    path.join(projectRoot, "src-tauri/src/lib.rs"),
    "utf8",
  );
  const handler = /tauri::generate_handler!\[([\s\S]*?)\]\)/.exec(lib)?.[1];
  if (!handler) throw new Error("Missing Tauri command registration");
  const policy = JSON.parse(
    fs.readFileSync(path.join(projectRoot, "control/policy.json"), "utf8"),
  );
  const classified = new Map();
  for (const [group, entries] of Object.entries(policy)) {
    if (group === "schema_version") continue;
    for (const name of Array.isArray(entries)
      ? entries
      : Object.keys(entries)) {
      if (classified.has(name)) throw new Error(`Duplicate policy: ${name}`);
      classified.set(name, {
        status: ["blocked", "unavailable"].includes(group)
          ? group
          : "available",
        requires:
          group === "read" || ["blocked", "unavailable"].includes(group)
            ? []
            : group.split("_"),
        reason: Array.isArray(entries) ? null : entries[name],
      });
    }
  }
  const names = new Set();
  const commands = [];
  for (const match of handler.matchAll(/commands::(\w+)::(\w+)\s*,/g)) {
    const [, module, name] = match;
    if (names.has(name)) throw new Error(`Duplicate command: ${name}`);
    names.add(name);
    if (!classified.has(name))
      throw new Error(`Review control/policy.json for new command: ${name}`);
    const source = `src-tauri/src/commands/${module}.rs`;
    // Rust comments contribute to the public contract hash on every platform.
    const text = fs.readFileSync(path.join(projectRoot, source), "utf8").replace(/\r\n/g, "\n");
    const signature = new RegExp(
      `pub (async )?fn ${name}\\s*\\(([\\s\\S]*?)\\)\\s*->\\s*([^\\{]+)\\{`,
    ).exec(text);
    if (!signature) throw new Error(`Unsupported command signature: ${name}`);
    const args = splitArgs(signature[2]).map((arg) => {
      const parsed = /^(\w+)\s*:\s*(.+)$/.exec(arg.replace(/\s+/g, " "));
      if (!parsed || /[&]|State<|Window|Channel</.test(parsed[2]))
        throw new Error(`Unsupported command argument: ${name}`);
      return {
        rust_name: parsed[1],
        name: camel(parsed[1]),
        type: parsed[2],
        injected: /^(tauri::)?AppHandle$/.test(parsed[2]),
        optional: /^Option\s*</.test(parsed[2]),
      };
    });
    const result = /^Result\s*</.test(signature[3].trim());
    const prefix = text.slice(0, signature.index);
    const docs = /((?:\/\/\/[^\n]*\n)+)\s*#\[tauri::command\]\s*$/
      .exec(prefix)?.[1]
      ?.replace(/^\/\/\/[ \t]?/gm, "")
      .trim();
    commands.push({
      name,
      module,
      ...classified.get(name),
      parameters: args.filter((arg) => !arg.injected),
      source,
      line: prefix.split("\n").length,
      description: docs || name,
      output_type: signature[3].trim(),
      async: !!signature[1],
      fallible: result,
      internal_args: args,
    });
  }
  for (const name of classified.keys())
    if (!names.has(name))
      throw new Error(`Policy references unregistered command: ${name}`);
  commands.sort((a, b) => a.name.localeCompare(b.name, "en"));
  const basicCommands = commands.map(
    ({ async: isAsync, internal_args, fallible, ...entry }) => entry,
  );
  const {commands: publicCommands, definitions} = enrichCommands(projectRoot, basicCommands);
  const hash = crypto
    .createHash("sha256")
    .update(JSON.stringify({commands:publicCommands,definitions}))
    .digest("hex");
  return {
    schema_version: 1,
    protocol_version: 1,
    catalog_hash: hash,
    $defs: definitions,
    commands: publicCommands,
    internal: commands,
  };
}

export function generated(projectRoot = root) {
  const { internal, ...catalog } = contract(projectRoot);
  const arms = internal
    .filter((command) => command.status === "available")
    .map((command) => {
      const args = command.internal_args.map((arg) =>
        arg.injected
          ? "app.clone()"
          : `decode_arg(args, ${JSON.stringify(arg.name)}, ${JSON.stringify(arg.rust_name)}, ${arg.optional})?`,
      );
      const call = `crate::commands::${command.module}::${command.name}(${args.join(", ")})${command.async ? ".await" : ""}${command.fallible ? '.map_err(command_error)?' : ""}`;
      return `        ${JSON.stringify(command.name)} => encode_result(${call}),`;
    });
  const rust = `// Generated by scripts/generate-control-contract.mjs. Do not edit.\npub const CATALOG: &str = include_str!("../../control/commands.json");\npub async fn dispatch(app: &tauri::AppHandle, name: &str, args: &serde_json::Map<String, serde_json::Value>) -> Result<serde_json::Value, ControlError> {\n    match name {\n${arms.join("\n")}\n        _ => Err(ControlError::new("UNKNOWN_COMMAND", "命令未开放")),\n    }\n}\n`;
  return new Map([
    ["control/commands.json", `${JSON.stringify(catalog, null, 2)}\n`],
    ["control/reference.md", referenceMarkdown(catalog)],
    ["src-tauri/src/control_dispatch.generated.rs", rust],
  ]);
}

if (
  process.argv[1] &&
  path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)
) {
  const check = process.argv.includes("--check");
  for (const [relative, content] of generated()) {
    const destination = path.join(root, relative);
    if (check) {
      if (
        !fs.existsSync(destination) ||
        fs.readFileSync(destination, "utf8") !== content
      )
        throw new Error(
          `Stale control contract: run npm run control:generate (${relative})`,
        );
    } else fs.writeFileSync(destination, content, "utf8");
  }
  console.log(
    check
      ? "Control contract matches registered Tauri commands and reviewed policy."
      : "Generated control command catalog and native dispatch.",
  );
}
