// A UUID as the API spells its ids (`CLAUDE.md`, "API conventions": "Ids UUID
// strings"). A route parameter that is not one names nothing, so a page can
// answer not-found without asking the server.

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

export function isUuid(value: string | undefined): value is string {
  return value !== undefined && UUID.test(value);
}
