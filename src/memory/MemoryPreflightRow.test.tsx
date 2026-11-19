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

/** Open the disclosure (a click is what Enter and Space on its native button produce). */
async function expand() {
  fireEvent.click(await screen.findByTestId("memory-summary"));
}

function deferred<T>() {
  let resolve!: (v: T) => void;
  let reject!: (e: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
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
    await expand();
    await waitFor(() => expect(screen.getByTestId("memory-estimate").textContent).toContain("%maxcore 4000"));
    // The older one arrives late with a different (warning) verdict: it must not replace it.
    await act(async () => older.d.resolve(reply(older.seq, localCheck({}, null))));
    expect(screen.getByTestId("memory-estimate").textContent).toContain("%maxcore 4000");
    expect(screen.getByTestId("memory-estimate").textContent).not.toContain("ORCA's default");
  });

  // 5.5 F3. CONTROL (`run_p5fu_controls.sh`, `row-older-error-shown`): the failure of any request
  // shown, not only the latest's (`if (seq === latest.current)` dropped) → red.
  it("never shows the failure of an older request over a newer answer", async () => {
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
    await act(async () => newer.d.resolve(reply(newer.seq, localCheck({}, 4000))));
    await screen.findByTestId("memory-summary");
    await act(async () => older.d.reject("sidecar went away"));
    expect(screen.queryByText(/Memory check unavailable/)).toBeNull();
    expect(screen.getByTestId("memory-summary")).toBeTruthy();
    // The latest request's own failure is shown (the guard suppresses only the older one).
    fireEvent.change(screen.getByTestId("buffer"), { target: { value: "! HF\n%maxcore 4100" } });
    await waitFor(() => expect(pending.length).toBe(3));
    await act(async () => pending[2].d.reject("boom"));
    expect((await screen.findByText(/Memory check unavailable: boom/)).textContent).toContain("boom");
  });

  // 5.5 F3. CONTROL (`row-button-without-ram-warning`, through the DOM): P offered for a disk-only
  // warning → the button is there, red.
  it("has no button for a disk-only warning", async () => {
    const diskOnly = localCheck({
      disk: { state: "known", avail_bytes: 5 * 1024 ** 3, min_free_bytes: 10 * 1024 ** 3, warns: true },
      warns: true,
    });
    const bridge: MemoryBridge = { estimate: async (seq) => reply(seq, diskOnly), applyMaxcore: async (i) => i };
    render(<Harness bridge={bridge} initial="! HF\n%maxcore 1600" target={null} />);
    await expand();
    await screen.findByTestId("memory-line-local");
    expect(screen.getByTestId("memory-line-local").textContent).toContain("free disk 5.0 GiB — below 10.0 GiB");
    expect(screen.queryByRole("button", { name: /Set %maxcore/ })).toBeNull();
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
    // Collapsed by default: the button is behind the disclosure.
    expect(screen.queryByRole("button", { name: "Set %maxcore to 1600" })).toBeNull();
    await expand();
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
    await expand();
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
    await expand();
    await screen.findByTestId("memory-line-local");
    expect(screen.queryByTestId("memory-line-server")).toBeNull();
    cleanup();
    const fits: MemoryBridge = { estimate: async (seq) => reply(seq, localCheck(), uniCheck()), applyMaxcore: async (i) => i };
    render(<Harness bridge={fits} initial="! HF\n%maxcore 1600" target="p1" />);
    await new Promise((r) => setTimeout(r, 20));
    expect(screen.queryByTestId("memory-preflight")).toBeNull();
  });
});

describe("MemoryPreflightRow — the collapsed disclosure", () => {
  const twoWarnings: MemoryBridge = {
    estimate: async (seq) => reply(seq, localCheck({}, null), uniCheck({}, null)),
    applyMaxcore: async (i) => i,
  };

  // CONTROL (`run_p5fu_controls.sh`, `row-open-by-default`): `useState(true)` → the lines and the
  // button are there before any click → red.
  it("is closed by default: one summary line, no long lines and no button", async () => {
    render(<Harness bridge={twoWarnings} initial="! HF" target="p1" />);
    const summary = await screen.findByTestId("memory-summary");
    expect(summary.getAttribute("aria-expanded")).toBe("false");
    expect(screen.queryByTestId("memory-details")).toBeNull();
    expect(screen.queryByTestId("memory-line-local")).toBeNull();
    expect(screen.queryByRole("button", { name: /Set %maxcore/ })).toBeNull();
    expect(screen.getByRole("status")).toBe(screen.getByTestId("memory-preflight"));
    expect(screen.getByTestId("memory-preflight").contains(summary)).toBe(true);
  });

  // CONTROL (`row-summary-without-threshold`): the summary without `> threshold` → red.
  it("names each warning target in the summary with U and its threshold", async () => {
    render(<Harness bridge={twoWarnings} initial="! HF" target="p1" />);
    const summary = await screen.findByTestId("memory-summary");
    expect(summary.textContent).toBe(
      "⚠ Memory: Create (local) ≈32.9 GiB > 13.7 GiB · Create & Run (uni) ≈98.5 GiB > 56.5 GiB▸",
    );
    expect(summary.className).toContain("memory-warns");
  });

  it("says an unread RAM as 'uni: memory unknown', with no U for it", async () => {
    const bridge: MemoryBridge = { estimate: async (seq) => reply(seq, localCheck({}, null), uniUnknown()), applyMaxcore: async (i) => i };
    render(<Harness bridge={bridge} initial="! HF" target="p1" />);
    const text = (await screen.findByTestId("memory-summary")).textContent ?? "";
    expect(text).toContain("Create (local) ≈32.9 GiB > 13.7 GiB");
    expect(text).toContain("uni: memory unknown");
    expect(text).not.toMatch(/Create & Run \(uni\)|98\.5/);
    // Nothing warns, only unknown: no warning sign, and no warn tone.
    cleanup();
    const unknownOnly: MemoryBridge = { estimate: async (seq) => reply(seq, localCheck(), uniUnknown()), applyMaxcore: async (i) => i };
    render(<Harness bridge={unknownOnly} initial="! HF\n%maxcore 1600" target="p1" />);
    const only = await screen.findByTestId("memory-summary");
    expect(only.textContent).toBe("Memory: uni: memory unknown; disk unknown▸");
    expect(only.className).not.toContain("memory-warns");
  });

  // CONTROLS (`run_p5fu_controls.sh`): `row-apply-error-only-while-open` (the error shown only
  // while the disclosure is open) and `row-apply-error-removed` (the error div gone) → red.
  it("keeps a failed apply's error visible after the row is collapsed", async () => {
    const bridge: MemoryBridge = {
      estimate: async (seq) => reply(seq, localCheck({}, null)),
      applyMaxcore: async () => {
        throw "the input has 2 %maxcore lines; edit it by hand";
      },
    };
    render(<Harness bridge={bridge} initial="! HF" target={null} />);
    await expand();
    fireEvent.click(await screen.findByRole("button", { name: "Set %maxcore to 1600" }));
    const error = await screen.findByText(/Could not set %maxcore: the input has 2 %maxcore lines/);
    expect(error).toBeTruthy();
    // Collapse: the lines and the button go, the error stays — it is the only word of the failure.
    fireEvent.click(screen.getByTestId("memory-summary"));
    expect(screen.queryByTestId("memory-details")).toBeNull();
    expect(screen.queryByRole("button", { name: /Set %maxcore/ })).toBeNull();
    expect(screen.getByText(/Could not set %maxcore: the input has 2 %maxcore lines/)).toBeTruthy();
  });

  // CONTROL (`row-toggle-one-way`): the click only opens (`setOpen(true)`) → the second click
  // leaves it open, red.
  it("opens and closes on the summary, a native button that takes Enter and Space", async () => {
    render(<Harness bridge={twoWarnings} initial="! HF" target="p1" />);
    const summary = await screen.findByTestId("memory-summary");
    // jsdom does not turn Enter/Space into a click; a native `<button type="button">` does it in the
    // browser, so the guarantee pinned here is the element, plus the click that key produces.
    expect(summary.tagName).toBe("BUTTON");
    expect(summary.getAttribute("type")).toBe("button");
    summary.focus();
    expect(document.activeElement).toBe(summary);
    expect(summary.getAttribute("aria-controls")).toBeTruthy();
    fireEvent.click(summary);
    expect(summary.getAttribute("aria-expanded")).toBe("true");
    expect(screen.getByTestId("memory-details").id).toBe(summary.getAttribute("aria-controls"));
    expect(screen.getByTestId("memory-line-server")).toBeTruthy();
    expect(summary.textContent?.endsWith("▾")).toBe(true);
    fireEvent.click(summary);
    expect(summary.getAttribute("aria-expanded")).toBe("false");
    expect(screen.queryByTestId("memory-details")).toBeNull();
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
