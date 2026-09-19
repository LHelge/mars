// The route table of `SPEC.md`, "Frontend", Routes. Every route is one line
// with its own import line: each page task replaces exactly one element and one
// import, so the sibling branches barely touch each other.
//
// `/login`, `/invite/:token`, `/forgot-password` and `/reset-password/:token`
// render outside `ProtectedRoute`; `/change-password` is inside it but exempt
// from the must-change redirect, so the user can actually clear the flag.

import { Route, Routes } from "react-router";
import { AdminRoute } from "./components/AdminRoute";
import { ProtectedRoute } from "./components/ProtectedRoute";
import { DashboardPage } from "./pages/DashboardPage";
import { NotFoundPage } from "./pages/NotFoundPage";
import { PlaceholderPage } from "./pages/PlaceholderPage";

export function App() {
  return (
    <Routes>
      {/* Reached without a session. */}
      <Route path="/login" element={<PlaceholderPage title="Sign in" layout="auth" />} />
      <Route path="/invite/:token" element={<PlaceholderPage title="Accept invitation" layout="auth" />} />
      <Route path="/forgot-password" element={<PlaceholderPage title="Forgot your password?" layout="auth" />} />
      <Route path="/reset-password/:token" element={<PlaceholderPage title="Choose a new password" layout="auth" />} />

      <Route element={<ProtectedRoute />}>
        <Route path="/change-password" element={<PlaceholderPage title="Change your password" layout="auth" />} />
        <Route path="/" element={<DashboardPage />} />
        <Route path="/projects" element={<PlaceholderPage title="Projects" />} />
        <Route path="/projects/:id" element={<PlaceholderPage title="Project" />} />
        <Route path="/projects/:id/tasks/:number" element={<PlaceholderPage title="Task" />} />
        <Route path="/sessions/:id" element={<PlaceholderPage title="Session" />} />
        <Route path="/secrets" element={<PlaceholderPage title="Secrets" />} />
        <Route path="/settings" element={<PlaceholderPage title="Settings" />} />

        <Route element={<AdminRoute />}>
          <Route path="/admin" element={<PlaceholderPage title="Administration" />} />
        </Route>

        <Route path="*" element={<NotFoundPage />} />
      </Route>
    </Routes>
  );
}
