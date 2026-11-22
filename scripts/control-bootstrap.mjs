import fs from "node:fs";
import path from "node:path";
export function bootstrapPrompt(projectRoot) {
  const root = path.resolve(projectRoot);
  const template = fs.readFileSync(
    path.join(root, "control/bootstrap.template.txt"),
    "utf8",
  );
  return template
    .replaceAll("{{PROJECT_ROOT}}", () => root)
    .replaceAll("{{START_FILE}}", () => path.join(root, "control/START.md"))
    .replaceAll("{{CLI_FILE}}", () => path.join(root, "scripts/homebase.mjs"));
}
