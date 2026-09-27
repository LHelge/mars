// The two hooks the signed-in pages and the auth forms share. The media hooks
// beside them (`useMediaQuery` and its queries, `useVisualViewportHeight`) are
// imported by path, as every one of their readers already does: a barrel
// carries what its importers import and nothing else (`ARCHITECTURE.md`,
// "Frontend architecture", Barrels and the first-paint path).

export { useAuth } from "./useAuth";
export type { UseAuth } from "./useAuth";

export { useFormSubmit } from "./useFormSubmit";
export type { UseFormSubmit } from "./useFormSubmit";
