// @vitest-environment jsdom
import { describe, it, expect, vi, afterEach } from "vitest";
import { render, screen, fireEvent, cleanup } from "@testing-library/react";

import { OptTSMethodPicker } from "./OptTSMethodPicker";
import { DEFAULT_BUILDER_STATE, methodSliceOf } from "./build-input";

afterEach(cleanup);

/** The override the parent will hand to buildOptTSInput. `undefined` = never emitted. */
function lastOverride(spy: ReturnType<typeof vi.fn>) {
  const calls = spy.mock.calls;
  return calls.length ? calls[calls.length - 1][0] : undefined;
}

describe("OptTSMethodPicker — an explicit method pick turns inherit off (the m2 gate)", () => {
  // THE gate-failure bite: picking a family must emit `{ methodState }`, NOT `{}`. A stale-inherit
  // impl emits `{}` (the inherit path → the source's method) — exactly the m2 XTB→XTB failure.
  it("picking_a_family_emits_the_override", () => {
    const onChange = vi.fn();
    render(<OptTSMethodPicker onChange={onChange} />);
    // Inherit is the default; the per-family controls are hidden until a family is picked.
    fireEvent.click(screen.getByRole("radio", { name: "Composite (3c)" }));
    // The LAST emit is from MethodPicker.onChange (the buggy path) — it must be the override.
    expect(lastOverride(onChange)).toEqual({
      methodState: methodSliceOf({ ...DEFAULT_BUILDER_STATE, methodFamily: "composite" }),
    });
    // And explicitly NOT the inherit sentinel — a stale-inherit impl lands here (red).
    expect(lastOverride(onChange)).not.toEqual({});
  });

  // The byte-identical default: an untouched picker never fires, so the parent keeps its own `{}`
  // (inherit) and buildOptTSInput takes the unchanged path. No regression for anyone.
  it("untouched_picker_stays_inherit", () => {
    const onChange = vi.fn();
    render(<OptTSMethodPicker onChange={onChange} />);
    expect(onChange).not.toHaveBeenCalled();
  });

  // Toggling "Inherit from source" back ON after a pick returns to `{}` — the override is
  // withdrawn, inherit is restored (not stuck off after touching a control).
  it("toggling_inherit_back_on_returns_to_inherit", () => {
    const onChange = vi.fn();
    render(<OptTSMethodPicker onChange={onChange} />);
    fireEvent.click(screen.getByRole("radio", { name: "Composite (3c)" }));
    expect(lastOverride(onChange)).not.toEqual({}); // now an override…
    fireEvent.click(screen.getByRole("radio", { name: "Inherit from source" }));
    expect(lastOverride(onChange)).toEqual({}); // …and inherit restored
  });
});
