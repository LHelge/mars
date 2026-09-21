import js from "@eslint/js";
import reactHooks from "eslint-plugin-react-hooks";
import reactRefresh from "eslint-plugin-react-refresh";
import globals from "globals";
import tseslint from "typescript-eslint";

export default tseslint.config(
  { ignores: ["dist", "node_modules", "coverage"] },
  {
    files: ["**/*.{ts,tsx}"],
    extends: [
      js.configs.recommended,
      ...tseslint.configs.recommendedTypeChecked,
      reactHooks.configs.flat["recommended-latest"],
      reactRefresh.configs.vite,
    ],
    languageOptions: {
      ecmaVersion: 2023,
      globals: globals.browser,
      parserOptions: {
        projectService: true,
        tsconfigRootDir: import.meta.dirname,
      },
    },
    rules: {
      "react-hooks/rules-of-hooks": "error",
      "react-hooks/exhaustive-deps": "error",
      // A `switch` over a union names every member of it. The frontend's
      // unions mirror server enums that are only ever added to (`SPEC.md`,
      // "AgentEvent", "TaskEvent"), and TypeScript calls such a switch
      // exhaustive without saying whether it covers the cases by name or by a
      // `default:` that guesses at them. `considerDefaultExhaustiveForUnions`
      // keeps a deliberate `default:` legal — the forward-compatible arm of a
      // boundary reducer — while a switch that simply forgot a member is an
      // error.
      "@typescript-eslint/switch-exhaustiveness-check": [
        "error",
        { considerDefaultExhaustiveForUnions: true },
      ],
    },
  },
  {
    // The Playwright suite is Node, not React. A fixture is
    // `async ({ deps }, use) => { await use(value) }`: `use` there is
    // Playwright's own hand-over, which the React Hooks rule mistakes for
    // `React.use`, and a fixture that depends on nothing still has to
    // destructure an empty object for Playwright to read its dependencies off.
    files: ["tests/**/*.ts"],
    rules: {
      "react-hooks/rules-of-hooks": "off",
      "no-empty-pattern": "off",
    },
  },
);
