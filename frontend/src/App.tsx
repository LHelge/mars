// The route table of `SPEC.md`, "Frontend", Routes. Every route is one line
// with its own import line: each page task replaces exactly one element and one
// import, so the sibling branches barely touch each other.
//
// `/login`, `/invite/:token`, `/forgot-password` and `/reset-password/:token`
// render outside `ProtectedRoute`; `/change-password` is inside it but exempt
// from the must-change redirect, so the user can actually clear the flag.

import { Route, Routes } from "react-router";
import { AdminRoute } from "./components/AdminRoute";
import { AdminPage } from "./pages/AdminPage";
import { ProtectedRoute } from "./components/ProtectedRoute";
import { ChangePasswordPage } from "./pages/ChangePasswordPage";
import { AcceptInvitePage } from "./pages/AcceptInvitePage";
import { DashboardPage } from "./pages/DashboardPage";
import { ForgotPasswordPage } from "./pages/ForgotPasswordPage";
import { LoginPage } from "./pages/LoginPage";
import { NotFoundPage } from "./pages/NotFoundPage";
import { PlaceholderPage } from "./pages/PlaceholderPage";
import { ResetPasswordPage } from "./pages/ResetPasswordPage";
import { SecretsPage } from "./pages/SecretsPage";
import { SettingsPage } from "./pages/SettingsPage";

export function App() {
  return (
    <Routes>
      {/* Reached without a session. */}
      <Route path="/login" element={<LoginPage />} />
      <Route path="/invite/:token" element={<AcceptInvitePage />} />
      <Route path="/forgot-password" element={<ForgotPasswordPage />} />
      <Route path="/reset-password/:token" element={<ResetPasswordPage />} />

      <Route element={<ProtectedRoute />}>
        <Route path="/change-password" element={<ChangePasswordPage />} />
        <Route path="/" element={<DashboardPage />} />
        <Route path="/projects" element={<PlaceholderPage title="Projects" />} />
        <Route path="/projects/:id" element={<PlaceholderPage title="Project" />} />
        <Route path="/projects/:id/tasks/:number" element={<PlaceholderPage title="Task" />} />
        <Route path="/sessions/:id" element={<PlaceholderPage title="Session" />} />
        <Route path="/secrets" element={<SecretsPage />} />
        <Route path="/settings" element={<SettingsPage />} />

        <Route element={<AdminRoute />}>
          <Route path="/admin" element={<AdminPage />} />
        </Route>

        <Route path="*" element={<NotFoundPage />} />
      </Route>
    </Routes>
  );
}
