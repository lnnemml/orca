// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

import { InputBuilderForm } from "./InputBuilderForm";
import { setLocalMaxcore } from "../memory/local-maxcore";
import { initialPreferences, initialWork, useNewJobDraft } from "../screens/new-job-draft";

// The builder's state lives in the New Job draft store (ADR-026): reset it between tests, as
// `store.test.ts` resets the scene store.
beforeEach(() => {
  useNewJobDraft.setState({ ...initialWork(), ...initialPreferences(), generation: 0, consumedSeedId: 0 });
});

afterEach(() => {
  cleanup();
  setLocalMaxcore(null);
});

const WATER = "! r2SCAN-3c Opt\n* xyz 0 1\nO 0 0 0.11779\nH 0 0.755453 -0.471161\nH 0 -0.755453 -0.471161\n*\n";

function renderForm(runTarget: string | null) {
  const onGenerate = vi.fn();
  const view = render(<InputBuilderForm currentContent={WATER} onGenerate={onGenerate} runTarget={runTarget} />);
  const field = () => screen.getByLabelText("maxcore (MB)") as HTMLInputElement;
  const generated = () => {
    fireEvent.click(screen.getByText("Generate Input"));
    const calls = onGenerate.mock.calls;
    return String(calls[calls.length - 1][0]).split("\n").filter((l) => l.startsWith("%maxcore"));
  };
  const retarget = (t: string | null) =>
    view.rerender(<InputBuilderForm currentContent={WATER} onGenerate={onGenerate} runTarget={t} />);
  return { field, generated, retarget };
}

describe("the builder's %maxcore default (ADR-024 o26.5, o26.13.1)", () => {
  // CONTROLS: 2000 for Local (builderMaxcoreDefault ignoring the local value) → red; the field not
  // following a late cache load → red.
  it("follows Local's fitted value and a server's 2000, while untouched", () => {
    setLocalMaxcore(1600);
    const { field, generated, retarget } = renderForm(null);
    expect(field().value).toBe("1600");
    expect(generated()).toEqual(["%maxcore 1600"]);
    retarget("p1");
    expect(field().value).toBe("2000");
    retarget(null);
    expect(field().value).toBe("1600");
    // The cache re-read after a preset change (Max throughput: 1000) reaches an untouched field.
    act(() => setLocalMaxcore(1000));
    expect(field().value).toBe("1000");
  });

  // CONTROL: the default overriding a touched field (the `maxcoreTouched` guard removed) → red.
  it("never overrides a value the user typed", () => {
    setLocalMaxcore(1600);
    const { field, generated, retarget } = renderForm(null);
    fireEvent.change(field(), { target: { value: "3000" } });
    retarget("p1");
    expect(field().value).toBe("3000");
    act(() => setLocalMaxcore(1000));
    retarget(null);
    expect(field().value).toBe("3000");
    expect(generated()).toEqual(["%maxcore 3000"]);
  });

  it("is 2000 before the cache loads (no regression)", () => {
    const { field } = renderForm(null);
    expect(field().value).toBe("2000");
  });
});
