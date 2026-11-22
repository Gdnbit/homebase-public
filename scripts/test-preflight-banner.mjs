import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";
import vm from "node:vm";
import { build } from "esbuild";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";

const require = createRequire(import.meta.url);
const result = await build({
  entryPoints: [
    fileURLToPath(
      new URL("../src/components/preflight-banner.tsx", import.meta.url),
    ),
  ],
  bundle: true,
  write: false,
  platform: "node",
  format: "cjs",
  jsx: "automatic",
  external: ["react", "react/jsx-runtime"],
  plugins: [
    {
      name: "isolated-preflight-dependencies",
      setup(build) {
        build.onResolve(
          { filter: /^(react$|@tanstack\/react-query|lucide-react|@\/lib\/)/ },
          (args) => ({ path: args.path, namespace: "fixture" }),
        );
        build.onLoad({ filter: /.*/, namespace: "fixture" }, ({ path }) => ({
          contents:
            path === "react"
              ? "export function useEffect() {} export function useState(initial) { return [initial, () => {}]; }"
              : path === "@tanstack/react-query"
                ? "export function useQuery(options) { globalThis.options = options; return globalThis.query; }"
                : path === "lucide-react"
                  ? "export const AlertTriangle = () => null, ShieldAlert = () => null, CheckCircle2 = () => null, RefreshCw = () => null;"
                  : path.endsWith("utils")
                    ? "export const cn = (...parts) => parts.filter(Boolean).join(' ');"
                    : "export const systemApi = { preflight: () => [] };",
        }));
      },
    },
  ],
});
const module = { exports: {} };
const context = { require, module, exports: module.exports };
vm.runInNewContext(result.outputFiles[0].text, context);
const { PreflightBanner } = module.exports;
let refreshes = 0;
const check = (id, severity) => ({
  id,
  severity,
  label: id,
  ok: false,
  hint: "检查失败",
  detail: null,
});
const render = (state) => {
  context.query = {
    isLoading: false,
    isError: false,
    isFetching: false,
    refetch: () => {
      refreshes++;
    },
    ...state,
  };
  return renderToStaticMarkup(createElement(PreflightBanner));
};
assert.match(render({ isLoading: true }), /正在检查本机密钥工具/);
assert.equal(render({ data: [] }), "");
assert.equal(
  render({ data: [{ ...check("sops_exe", "critical"), ok: true }] }),
  "",
);
let html = render({ isError: true });
assert.match(html, /启动自检未完成/);
assert.match(html, /重新检查/);
html = render({
  data: [check("sops_exe", "critical"), check("agent_env", "warning")],
});
assert.match(html, /2 项检查未通过/);
assert.doesNotMatch(html, /2 项 critical|密钥管理会失效/);
assert.match(html, /检查失败/);
html = render({ data: [check("agent_env", "warning")] });
assert.match(html, /部分可选组件检查未通过/);
html = render({ data: [check("sops_exe", "critical")], isFetching: true });
assert.match(html, /disabled=""/);
assert.match(html, /正在检查/);
assert.match(html, /aria-busy="true"/);
// A failed refresh must supersede cached results, even if the old check passed.
html = render({ data: [], isError: true });
assert.match(html, /启动自检未完成/);
render({ data: [check("sops_exe", "critical")] });
const tree = PreflightBanner();
function findButton(element) {
  if (!element || typeof element !== "object") return;
  if (element.type === "button") return element;
  for (const child of [element.props?.children].flat(Infinity)) {
    const found = findButton(child);
    if (found) return found;
  }
}
findButton(tree).props.onClick();
assert.equal(refreshes, 1);
assert.equal(render({ data: [] }), "", "successful recheck clears the warning");
console.log(
  "preflight-banner tests passed (loading, success, failure, retry and recovery)",
);
