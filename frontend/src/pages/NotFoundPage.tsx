// The catch-all inside `ProtectedRoute`: an authenticated visitor to an unknown
// path keeps the console frame and a way back. An unauthenticated one never
// reaches here — the guard sends them to `/login` first.

import { Link } from "react-router";
import { EmptyState, PageLayout } from "../components";

export function NotFoundPage() {
  return (
    <PageLayout title="Not found">
      <EmptyState
        title="No such page"
        description="The address does not match any route in Mars."
        action={
          <Link to="/" className="text-console-accent text-sm hover:underline">
            Back to the dashboard
          </Link>
        }
      />
    </PageLayout>
  );
}
