// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { useState } from "react";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

import { MemoryPreflightRow, useMemoryEstimate, type MemoryBridge } from "./MemoryPreflightRow";
import { SubmitNoticeBanners } from "./SubmitNoticeBanners";
import { localCheck, reply, uniCheck, uniUnknown } from "./test-fixtures";
import type { MemoryEstimateReply } from "../types";

afterEach(cleanup);

/** The screen's wiring in miniature: the hook on the buffer + target, the row under it, the buffer
 *  in state so the button's result is visible. */
function Harness({ bridge, initial, target }: { bridge: MemoryBridge; initial: string; target: string | null }) {
  const [input, setInput] = useState(initial);
  const memory = useMemoryEstimate(input, target, bridge, 0);
  return (
    <>
      <textarea data-testid="buffer" value={input} onChange={(e) => setInput(e.currentTarget.value)} />
      <MemoryPreflightRow
        reply={memory.reply}
        error={memory.error}
        input={input}
        targetName={target ? "uni" : null}
        onInputChange={setInput}
        bridge={bridge}
      />
    </>
  );
}

function deferred<T>() {
  let resolve!: (v: T) => void;
  const promise = new Promise<T>((r) => (resolve = r));
  return { promise, resolve };
}

describe("MemoryPreflightRow", () => {
  // CONTROL: apply every reply (drop the `isLatestReply` check) → the older one lands last → red.
  it("never applies an older reply over a newer one (o26.13.6)", async () => {
    const pending: { seq: number; d: ReturnType<typeof deferred<MemoryEstimateReply>> }[] = [];
    const bridge: MemoryBridge = {
      estimate: (seq) => {
        const d = deferred<MemoryEstimateReply>();
        pending.push({ seq, d });
        return d.promise;
      },
      applyMaxcore: async (i) => i,
    };
    render(<Harness bridge={bridge} initial="! HF" target={null} />);
    await waitFor(() => expect(pending.length).toBe(1));
    fireEvent.change(screen.getByTestId("buffer"), { target: { value: "! HF\n%maxcore 4000" } });
    await waitFor(() => expect(pending.length).toBe(2));
    const [older, newer] = pending;
    // The newer answer first: the laptop warns with %maxcore 4000.
    await act(async () => newer.d.resolve(reply(newer.seq, localCheck({}, 4000))));
    await waitFor(() => expect(screen.getByTestId("memory-estimate").textContent).toContain("%maxcore 4000"));
    // The older one arrives late with a different (warning) verdict: it must not replace it.
    await act(async () => older.d.resolve(reply(older.seq, localCheck({}, null))));
    expect(screen.getByTestId("memory-estimate").textContent).toContain("%maxcore 4000");
    expect(screen.getByTestId("memory-estimate").textContent).not.toContain("ORCA's default");
  });

  // CONTROL: the button appending P instead of taking apply_maxcore's text → red.
  it("the button replaces the buffer with apply_maxcore's text", async () => {
    const applied: [string, number][] = [];
    const bridge: MemoryBridge = {
      estimate: async (seq, input) => reply(seq, localCheck({}, input.includes("%maxcore 1600") ? 1600 : null)),
      applyMaxcore: async (input, p) => {
        applied.push([input, p]);
        return `! HF\n%maxcore ${p}\n* xyz 0 1\nH 0 0 0\n*\n`;
      },
    };
    render(<Harness bridge={bridge} initial={"! HF\n* xyz 0 1\nH 0 0 0\n*\n"} target={null} />);
    const button = await screen.findByRole("button", { name: "Set %maxcore to 1600" });
    fireEvent.click(button);
    await waitFor(() =>
      expect((screen.getByTestId("buffer") as HTMLTextAreaElement).value).toBe("! HF\n%maxcore 1600\n* xyz 0 1\nH 0 0 0\n*\n"),
    );
    expect(applied).toEqual([["! HF\n* xyz 0 1\nH 0 0 0\n*\n", 1600]]);
    // Now it fits: the row hides.
    await waitFor(() => expect(screen.queryByTestId("memory-preflight")).toBeNull());
  });

  // CONTROL: U shown for an unknown RAM → red (the same guard as preflight.test.ts, through the DOM).
  it("shows a server's unknown facts as unknown, with no U, next to the local verdict", async () => {
    const bridge: MemoryBridge = {
      estimate: async (seq) => reply(seq, localCheck({}, null), uniUnknown()),
      applyMaxcore: async (i) => i,
    };
    render(<Harness bridge={bridge} initial="! r2SCAN-3c Opt" target="p1" />);
    const server = await screen.findByTestId("memory-line-server");
    expect(server.textContent).toContain("Create & Run on uni");
    expect(server.textContent).toContain("server memory unknown — re-run the connection test");
    expect(server.querySelector("[data-testid=memory-estimate]")).toBeNull();
    const local = screen.getByTestId("memory-line-local");
    expect(local.textContent).toContain("Create (runs locally)");
    expect(local.textContent).toContain("≈ 32.9 GiB");
    expect(screen.getByRole("status")).toBe(screen.getByTestId("memory-preflight"));
  });

  it("one line when this machine is selected, and none when everything fits", async () => {
    const bridge: MemoryBridge = { estimate: async (seq) => reply(seq, localCheck({}, null)), applyMaxcore: async (i) => i };
    render(<Harness bridge={bridge} initial="! HF" target={null} />);
    await screen.findByTestId("memory-line-local");
    expect(screen.queryByTestId("memory-line-server")).toBeNull();
    cleanup();
    const fits: MemoryBridge = { estimate: async (seq) => reply(seq, localCheck(), uniCheck()), applyMaxcore: async (i) => i };
    render(<Harness bridge={fits} initial="! HF\n%maxcore 1600" target="p1" />);
    await new Promise((r) => setTimeout(r, 20));
    expect(screen.queryByTestId("memory-preflight")).toBeNull();
  });
});

describe("SubmitNoticeBanners (o26.6)", () => {
  // CONTROL: rendering only one of the two (the memory notice overwriting the %pal one) → red.
  it("shows the %pal notice and the memory notice side by side", () => {
    render(
      <SubmitNoticeBanners
        notice={{ text: "[OrcaStudio] %pal nprocs aligned to 24", tone: "ok" }}
        memoryNotice="[OrcaStudio] memory: the job's estimate is 98.5 GiB"
      />,
    );
    expect(screen.getByTestId("submit-notice").textContent).toBe("[OrcaStudio] %pal nprocs aligned to 24");
    expect(screen.getByTestId("memory-notice").textContent).toBe("[OrcaStudio] memory: the job's estimate is 98.5 GiB");
  });
});
