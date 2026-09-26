import js from "@eslint/js";
import prettier from "eslint-config-prettier";
import reactHooks from "eslint-plugin-react-hooks";
import reactRefresh from "eslint-plugin-react-refresh";
import globals from "globals";
import tseslint from "typescript-eslint";

// The directory barrels this codebase no longer has (`ARCHITECTURE.md`,
// "Frontend architecture", Barrels and the first-paint path). A barrel over a
// whole feature directory is a static edge to every module in it, so one name
// imported through it drags the rest into the importer's chunk; these five
// served a handful of imports each against many times as many deep ones, and
// were deleted. The rule keeps them deleted — the spellings a file under `src/`
// could reach them by, which are unambiguous because no directory here holds a
// sibling file of the same name.
const GONE = ["components", "utils", "services", "pages"];
const goneBarrels = [
  ...GONE.flatMap((dir) => [`./${dir}`, `../${dir}`, `../../${dir}`]),
  "../tasks",
  "../../tasks",
].map((name) => ({
  name,
  message:
    "This directory barrel was deleted: import the module itself " +
    '(ARCHITECTURE.md, "Frontend architecture").',
}));

// The files a first paint evaluates: the entry, the shell, the guards and the
// eagerly routed pages of `App.tsx`. The barrels that survive — `session/`,
// `launch/`, `pages/project/` — are reached only from lazy route chunks, and
// stay that way because nothing here may import one.
const ENTRY_PATH = [
  "src/main.tsx",
  "src/App.tsx",
  "src/AuthBootstrap.tsx",
  "src/queryClient.ts",
  "src/components/AdminRoute.tsx",
  "src/components/AuthLayout.tsx",
  "src/components/PageLayout.tsx",
  "src/components/ProtectedRoute.tsx",
  "src/pages/AcceptInvitePage.tsx",
  "src/pages/ChangePasswordPage.tsx",
  "src/pages/DashboardPage.tsx",
  "src/pages/ForgotPasswordPage.tsx",
  "src/pages/LoginPage.tsx",
  "src/pages/NotFoundPage.tsx",
  "src/pages/ResetPasswordPage.tsx",
];
const lazyBarrels = ["session", "launch", "project", "secrets", "tasks"]
  .flatMap((dir) => [`./${dir}`, `../${dir}`, `../../${dir}`])
  .map((name) => ({
    name,
    message:
      "A first paint evaluates this file, and this barrel reaches feature UI: " +
      'import the module itself, or lazily (ARCHITECTURE.md, "Frontend ' +
      'architecture"; scripts/check-entry-chunk.mjs enforces the result).',
  }));

// Icons come from the one map of `src/components/icons.ts` (ADR 0047), which is
// the only file that names the library.
const iconLibrary = {
  name: "lucide-react",
  message:
    "Import `Icon` from components/icons.ts, the console's one icon map " +
    "(ADR 0047).",
};

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
    files: ["src/**/*.{ts,tsx}"],
    rules: {
      "no-restricted-imports": [
        "error",
        { paths: [...goneBarrels, iconLibrary] },
      ],
    },
  },
  {
    files: ENTRY_PATH,
    rules: {
      "no-restricted-imports": [
        "error",
        { paths: [...goneBarrels, ...lazyBarrels, iconLibrary] },
      ],
    },
  },
  {
    files: ["src/components/icons.ts"],
    rules: { "no-restricted-imports": ["error", { paths: goneBarrels }] },
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
  // Last, so it switches off every stylistic rule above: layout is Prettier's
  // (`npm run format`), never a lint finding.
  prettier,
);
