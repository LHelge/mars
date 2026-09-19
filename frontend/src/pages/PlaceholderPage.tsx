// A route that exists so the route table of `SPEC.md`, "Frontend", is complete
// from the first commit. Each sibling page task replaces one `<Route>` element
// in `App.tsx` with its real page; nothing else imports this.

import { AuthLayout, PageLayout } from "../components";

export interface PlaceholderPageProps {
  title: string;
  /** `auth` for the routes reached without a session, `page` for the rest. */
  layout?: "auth" | "page";
}

export function PlaceholderPage({
  title,
  layout = "page",
}: PlaceholderPageProps) {
  const body = <p className="text-console-muted text-sm">not implemented yet</p>;

  if (layout === "auth") {
    return <AuthLayout title={title}>{body}</AuthLayout>;
  }
  return <PageLayout title={title}>{body}</PageLayout>;
}
