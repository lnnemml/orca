// @vitest-environment jsdom
/**
 * Unit 5.5 at screen level (Phase 5 follow-up, 5.5 F1): New Job renders the memory preflight row
 * and never lets it gate Create or Create & Run (ADR-024 o26.7). The screen is rendered whole with
 * `invoke` mocked; the Monaco editor and the 3Dmol viewer are stubs (their output is the live gate's,
 * not a unit test's).
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";

import { localCheck, reply, uniCheck } from "../memory/test-fixtures";
import { makeProfile } from "../servers/test-fixtures";

const invokeMock = vi.fn<(cmd: string, args?: Record<string, unknown>) => Promise<unknown>>();
vi.mock("@tauri-apps/api/core", () => ({ invoke: (cmd: string, args?: Record<string, unknown>) => invokeMock(cmd, args) }));
vi.mock("@tauri-apps/api/event", () => ({ listen: async () => () => {} }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ confirm: async () => true, message: async () => {}, open: async () => null }));
vi.mock("../editor/InputEditor", () => ({
  InputEditor: ({ value, onChange }: { value: string; onChange: (v: string) => void }) => (
    <textarea data-testid="editor" value={value} onChange={(e) => onChange(e.currentTarget.value)} />
  ),
}));
vi.mock("../viewer/MoleculeViewer", () => ({ MoleculeViewer: () => null }));

import { NewJobScreen } from "./NewJobScreen";

const UNI = makeProfile({ run_target: { is_run_target: true, reason: null } });

/** Every command the screen makes on mount, answered; `memory_estimate` with a warning on both targets. */
function answer(estimate: (seq: number) => unknown) {
  invokeMock.mockImplementation(async (cmd, args) => {
    switch (cmd) {
      case "memory_estimate":
        return estimate(args?.seq as number);
      case "list_server_profiles":
        return [UNI];
      case "get_settings":
        return { last_run_target: "p1" };
      case "list_groups":
      case "list_molecules":
      case "list_reagents":
        return [];
      case "create_job":
        return { id: "j-new", title: "t", input_content: String(args?.inputContent ?? "") };
      case "local_fitted_maxcore":
        return { default_maxcore_mib: 1600 };
      default:
        return null;
    }
  });
}

beforeEach(() => {
  invokeMock.mockReset();
});
afterEach(cleanup);

describe("NewJobScreen — the memory preflight (ADR-024 o26.7)", () => {
  // CONTROL (`run_p5fu_controls.sh`, `newjob-create-disabled-by-warning`): `canCreate` also requires
  // `!memory.reply?.local.warns` → both buttons disabled while the row warns → red.
  it("shows the warning and leaves Create and Create & Run enabled", async () => {
    answer((seq) => reply(seq, localCheck({}, null), uniCheck({}, null)));
    render(<NewJobScreen onCreatedDraft={() => {}} onOpenDetail={() => {}} />);
    fireEvent.change(screen.getByTestId("editor"), { target: { value: "! HF\n* xyz 0 1\nH 0 0 0\n*\n" } });
    const summary = await screen.findByTestId("memory-summary", undefined, { timeout: 3000 });
    expect(summary.textContent).toContain("Create & Run (uni) ≈98.5 GiB > 56.5 GiB");
    expect((screen.getByRole("button", { name: "Create Job" }) as HTMLButtonElement).disabled).toBe(false);
    expect((screen.getByRole("button", { name: "Create & Run" }) as HTMLButtonElement).disabled).toBe(false);
  });

  it("Create & Run with a warning still creates and opens the job on the server", async () => {
    answer((seq) => reply(seq, localCheck({}, null), uniCheck({}, null)));
    const opened: unknown[][] = [];
    render(<NewJobScreen onCreatedDraft={() => {}} onOpenDetail={(...a) => opened.push(a)} />);
    fireEvent.change(screen.getByTestId("editor"), { target: { value: "! HF\n* xyz 0 1\nH 0 0 0\n*\n" } });
    await screen.findByTestId("memory-summary", undefined, { timeout: 3000 });
    fireEvent.click(screen.getByRole("button", { name: "Create & Run" }));
    await waitFor(() => expect(opened).toEqual([["j-new", true, "p1"]]));
  });
});
