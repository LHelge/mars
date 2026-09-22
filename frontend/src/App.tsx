// The route table of `SPEC.md`, "Frontend", Routes. Every route is one line
// with its own import line.
//
// `/login`, `/invite/:token`, `/forgot-password` and `/reset-password/:token`
// render outside `ProtectedRoute`; `/change-password` is inside it but exempt
// from the must-change redirect, so the user can actually clear the flag.
//
// Code splitting: the eagerly imported pages are the ones an unauthenticated
// visitor or a cold sign-in reaches without navigating — the auth forms, which
// all share `AuthLayout`, and the dashboard the signed-in user lands on. Every
// heavier page is a `React.lazy` of its own file — there is no `pages/` barrel
// to import through (`ARCHITECTURE.md`, "Frontend architecture", Barrels and
// the first-paint path) — so the session view's `react-markdown` and
// virtualizer, the project board and the admin and secrets forms are fetched
// the first time someone navigates to them. The named export is mapped to
// `default` because this codebase has no default exports.
//
// One `Suspense` wraps the whole table. Every lazy route sits inside
// `ProtectedRoute`, so the fallback can be the same `LoadingState` inside
// `PageLayout` that those pages show while their own first read is in flight,
// and the chrome does not flicker between the two.

import { lazy, Suspense } from "react";
import { Route, Routes } from "react-router";
import { AdminRoute } from "./components/AdminRoute";
import { LoadingState } from "./components/LoadingState";
import { PageLayout } from "./components/PageLayout";
import { ProtectedRoute } from "./components/ProtectedRoute";
import { AcceptInvitePage } from "./pages/AcceptInvitePage";
import { ChangePasswordPage } from "./pages/ChangePasswordPage";
import { DashboardPage } from "./pages/DashboardPage";
import { ForgotPasswordPage } from "./pages/ForgotPasswordPage";
import { LoginPage } from "./pages/LoginPage";
import { NotFoundPage } from "./pages/NotFoundPage";
import { ResetPasswordPage } from "./pages/ResetPasswordPage";

const AdminPage = lazy(() =>
  import("./pages/AdminPage").then((m) => ({ default: m.AdminPage })),
);
const HelpPage = lazy(() =>
  import("./pages/HelpPage").then((m) => ({ default: m.HelpPage })),
);
const ProjectPage = lazy(() =>
  import("./pages/ProjectPage").then((m) => ({ default: m.ProjectPage })),
);
const ProjectsPage = lazy(() =>
  import("./pages/ProjectsPage").then((m) => ({ default: m.ProjectsPage })),
);
const SecretsPage = lazy(() =>
  import("./pages/SecretsPage").then((m) => ({ default: m.SecretsPage })),
);
const SessionPage = lazy(() =>
  import("./pages/SessionPage").then((m) => ({ default: m.SessionPage })),
);
const SettingsPage = lazy(() =>
  import("./pages/SettingsPage").then((m) => ({ default: m.SettingsPage })),
);

export function App() {
  return (
    <Suspense
      fallback={
        <PageLayout>
          <LoadingState label="Loading page" />
        </PageLayout>
      }
    >
      <Routes>
        {/* Reached without a session. */}
        <Route path="/login" element={<LoginPage />} />
        <Route path="/invite/:token" element={<AcceptInvitePage />} />
        <Route path="/forgot-password" element={<ForgotPasswordPage />} />
        <Route path="/reset-password/:token" element={<ResetPasswordPage />} />

        <Route element={<ProtectedRoute />}>
          <Route path="/change-password" element={<ChangePasswordPage />} />
          <Route path="/" element={<DashboardPage />} />
          <Route path="/projects" element={<ProjectsPage />} />
          <Route path="/projects/:id" element={<ProjectPage />} />
          <Route path="/projects/:id/tasks/:number" element={<ProjectPage />} />
          <Route path="/sessions/:id" element={<SessionPage />} />
          <Route path="/secrets" element={<SecretsPage />} />
          <Route path="/settings" element={<SettingsPage />} />
          <Route path="/help" element={<HelpPage />} />

          <Route element={<AdminRoute />}>
            <Route path="/admin" element={<AdminPage />} />
          </Route>

          <Route path="*" element={<NotFoundPage />} />
        </Route>
      </Routes>
    </Suspense>
  );
}
