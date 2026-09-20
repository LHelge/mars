// Every `data-testid` the end-to-end suite addresses the application by.
//
// The constants are the components' own (`src/utils/testIds.ts`), re-exported
// here so a spec imports them from `tests/utils/` like every other helper. One
// definition on both sides means a rename fails `npx tsc -b` — the name is
// gone from the module — instead of failing a scenario on a selector that no
// longer matches anything.
//
// Nothing new is declared here. A hook the suite needs is added to
// `src/utils/testIds.ts` and re-exported below.

export {
  STREAMING_CURSOR,
  SUBAGENT_CHILDREN,
  TASK_COLUMN_PREFIX,
  TRANSCRIPT_SCROLL,
  taskCardTestId,
  taskColumnTestId,
} from "../../src/utils/testIds";
