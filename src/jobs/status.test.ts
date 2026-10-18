import { describe, expect, it } from "vitest";
import { STATUS_CHIPS, queueCounts } from "./status";

describe("STATUS_CHIPS (o22.14.8)", () => {
  // CONTROL: remove "cancelling" from the list → red here (and a type error in `status.ts`: a
  // status without a chip cannot be filtered on).
  it("has a chip for cancelling, between running and completed", () => {
    expect(STATUS_CHIPS).toContain("cancelling");
    const at = (s: string) => (STATUS_CHIPS as readonly string[]).indexOf(s);
    expect(at("running")).toBeLessThan(at("cancelling"));
    expect(at("cancelling")).toBeLessThan(at("completed"));
    expect(new Set(STATUS_CHIPS).size).toBe(STATUS_CHIPS.length);
  });
});

describe("queueCounts (o22.14.8)", () => {
  const rows = (...status: Parameters<typeof queueCounts>[0][number]["status"][]) => status.map((s) => ({ status: s }));

  // CONTROL: count only `running` (the old test) → a cancelling job leaves the bar at "idle", red.
  it("counts a cancelling job as running: the server is still busy with it", () => {
    expect(queueCounts(rows("running", "cancelling", "queued", "queued", "draft", "completed", "cancelled"))).toEqual({
      running: 2,
      queued: 2,
    });
    expect(queueCounts(rows("cancelling"))).toEqual({ running: 1, queued: 0 });
    expect(queueCounts(rows("draft", "parsed", "failed"))).toEqual({ running: 0, queued: 0 });
  });
});

describe("cancelling in the screens (o22.14.8, source pins)", () => {
  // The screens have no render harness; what is pinned is that each site reads the pure value (or
  // helper) above instead of its own `queued`/`running` list, and that the badge has a style. The look
  // is the live gate's.
  const code = async (file: string) => {
    const { readFileSync } = await import("node:fs");
    const { resolve } = await import("node:path");
    return readFileSync(resolve(process.cwd(), file), "utf8")
      .replace(/\/\*[\s\S]*?\*\//g, "")
      .replace(/(^|[^:])\/\/.*$/gm, "$1");
  };

  // CONTROLS: the status bar counting `running` by its own filter; the Jobs screen with its own chip
  // list; the detail's `isActive` back to `running || queued` → red.
  it("reads the active count, the chips and the detail's liveness from the shared values", async () => {
    const app = await code("src/App.tsx");
    expect(app).toMatch(/queueCounts\(jobs\)/);
    expect(app).not.toMatch(/status\s*===\s*"running"/);
    const jobs = await code("src/screens/JobsScreen.tsx");
    expect(jobs).toMatch(/import \{ STATUS_CHIPS \} from "\.\.\/jobs\/status"/);
    expect(jobs).not.toMatch(/const STATUS_CHIPS/);
    const detail = await code("src/screens/JobDetailScreen.tsx");
    expect(detail).toMatch(/const isActive = job != null && isLiveStatus\(job\.status\)/);
  });

  // CONTROL: delete `.badge.cancelling` from the stylesheet → the chip and the header badge of a
  // cancelling job are unstyled, red.
  // …and the two rules differ: "still being cancelled" must not read as "cancelled" (o22.14.14.4).
  // CONTROL: give `.badge.cancelling` the `.badge.cancelled` declarations → red.
  it("styles the cancelling badge apart from the cancelled one", async () => {
    const css = await code("src/styles/app.css");
    const rule = (selector: string) => css.match(new RegExp(`\\.badge\\.${selector}\\s*\\{([^}]*)\\}`))?.[1].replace(/\s+/g, " ").trim();
    expect(rule("cancelling")).toBeTruthy();
    expect(rule("cancelled")).toBeTruthy();
    expect(rule("cancelling")).not.toBe(rule("cancelled"));
  });

  // CONTROL: drop `locked={busy !== null}` from the panel → a Retry in flight leaves Undo / Retry
  // cancel enabled, red (the behaviour itself is `cancelButtons`' test in remote.test.ts).
  it("locks the panel's cancel buttons while a retry is in flight", async () => {
    expect(await code("src/jobs/RemotePanel.tsx")).toMatch(/<CancelButtons\b[^>]*locked=\{busy !== null\}/);
  });

  // CONTROL: a surface that draws its own Cancel button instead of `CancelButtons` (the row or the
  // panel) → a pending row can show no Undo, red.
  it("draws the cancel family with CancelButtons in the Jobs row, the panel and the detail's toolbar", async () => {
    for (const file of ["src/screens/JobsScreen.tsx", "src/jobs/RemotePanel.tsx", "src/screens/JobDetailScreen.tsx"]) {
      expect(await code(file), file).toMatch(/<CancelButtons\b/);
    }
    expect(await code("src/screens/JobDetailScreen.tsx")).toMatch(/onCancelAction=\{cancelFamily\}/);
  });

  // CONTROL: `setNotice(null)` back at the top of the Jobs screen's click handler (or the handler not
  // using `bannerAfterClick`) → a declined dialog wipes the notice (a live-gate behaviour; this is
  // its source pin), red.
  it("clears the Jobs banner only through bannerAfterClick, never at the start of a click", async () => {
    const jobs = await code("src/screens/JobsScreen.tsx");
    const handler = jobs.slice(jobs.indexOf("const cancelFamily = useCallback("), jobs.indexOf("const remove = useCallback("));
    expect(handler).toMatch(/bannerAfterClick\(prev, outcome\)/);
    expect(handler).toMatch(/bannerAfterClick\(prev, "error"\)/);
    expect(handler).not.toMatch(/setNotice\(null\)/);
  });
});
