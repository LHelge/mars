// `/admin`, behind `AdminRoute` (`SPEC.md`, "Frontend", Routes and
// "User-facing features", Users (admin)).
//
// Two sections, both of them tables in the dashboard's idiom: who has an
// account, and who has been invited but has not accepted yet. Each section
// owns its queries and its row mutations; the page is the frame.

import { InvitesPanel } from "../components/admin/InvitesPanel";
import { UsersTable } from "../components/admin/UsersTable";
import { PageLayout } from "../components/PageLayout";

export function AdminPage() {
  return (
    <PageLayout title="Administration">
      <div className="space-y-8">
        <UsersTable />
        <InvitesPanel />
      </div>
    </PageLayout>
  );
}
