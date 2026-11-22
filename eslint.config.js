import js from "@eslint/js";
import tseslint from "typescript-eslint";

export default tseslint.config(
  { ignores: ["dist/**", "src-tauri/target/**", "node_modules/**"] },
  {
    files: ["src/**/*.{ts,tsx}"],
    extends: [js.configs.recommended, ...tseslint.configs.recommended],
    rules: {
      // TypeScript already checks unused locals and parameters in this project.
      "@typescript-eslint/no-unused-vars": "off",
      // IPC payloads and third-party adapters still use explicitly untyped values.
      "@typescript-eslint/no-explicit-any": "off",
    },
  },
);
