// @vitest-environment jsdom
import { describe, it, expect, vi, afterEach, beforeEach } from "vitest";
import { render, screen, fireEvent, cleanup, waitFor } from "@testing-library/react";

// The Tauri bridge is mocked: the NEB section (jobType NEB-TS) lists/reads parsed jobs through it.
const invokeMock = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

import { InputBuilderForm } from "./InputBuilderForm";

afterEach(cleanup);

/** A water buffer with an inline `* xyz` block — the form derives its Scene (charge/coords)
 * from this, so the generated input is deterministic. */
const WATER =
  "! r2SCAN-3c Opt Freq TightSCF\n" +
  "* xyz 0 1\n" +
  "O 0 0 0.11779\n" +
  "H 0 0.755453 -0.471161\n" +
  "H 0 -0.755453 -0.471161\n" +
  "*\n";

/** Render the form, return the `onGenerate` spy + the last input it was handed. */
function renderForm() {
  const onGenerate = vi.fn();
  render(<InputBuilderForm currentContent={WATER} onGenerate={onGenerate} />);
  const generate = () => fireEvent.click(screen.getByText("Generate Input"));
  const lastInput = () => {
    const calls = onGenerate.mock.calls;
    return String(calls.length ? calls[calls.length - 1][0] : "");
  };
  return { onGenerate, generate, lastInput };
}

const keywordLine = (input: string) =>
  input.split(/\r?\n/).find((l) => l.startsWith("!"))!;

describe("InputBuilderForm — the MethodPicker extraction is a no-behaviour-change refactor", () => {
  // GOLDEN — New Job's emitted input for the default (composite) method, pinned byte-for-byte.
  // If the extraction changed New Job's input by so much as a space, this snapshot goes red.
  it("new_job_default_input_is_byte_identical", () => {
    const { generate, lastInput } = renderForm();
    generate();
    expect(lastInput()).toMatchInlineSnapshot(`
      "! r2SCAN-3c Opt Freq TightSCF
      %maxcore 2000

      * xyz 0 1
      O     0.00000000    0.00000000    0.11779000
      H     0.00000000    0.75545300   -0.47116100
      H     0.00000000   -0.75545300   -0.47116100
      *
      "
    `);
  });

  // WIRING — selecting the "Functional + Basis" family through the extracted MethodPicker must
  // update the form's state so the emitted `!` line carries the DFT functional + basis + the
  // PAIRED def2/J aux + RIJCOSX + D4. This proves the picker is wired to the same state the
  // form emits from (the family logic still runs via buildOrcaInput, not a flattened string).
  it("switching family via the picker flows through to the emitted input", () => {
    const { generate, lastInput } = renderForm();
    fireEvent.click(screen.getByRole("radio", { name: "Functional + Basis" }));
    generate();
    const kw = keywordLine(lastInput());
    expect(kw).toBe("! B3LYP def2-TZVP def2/J RIJCOSX D4 Opt Freq TightSCF");
  });

  // New Job never shows the "Inherit from source" option (no `inherit` prop) — that option is
  // exclusive to the OptTS-derived spawn sites. BITE: a picker that always rendered it would
  // leak the option into New Job.
  it("new_job_has_no_inherit_option", () => {
    renderForm();
    expect(screen.queryByRole("radio", { name: "Inherit from source" })).toBeNull();
  });
});

// ── ADR-025 items 2 + 9 — the nprocs field: raw text, blank = auto, invalid blocks Generate ──
const palLines = (input: string) => input.split(/\r?\n/).filter((l) => l.startsWith("%pal"));
const nprocsField = () => screen.getByLabelText("nprocs") as HTMLInputElement;
const typeNprocs = (text: string) => fireEvent.change(nprocsField(), { target: { value: text } });
const generateButton = () => screen.getByText("Generate Input") as HTMLButtonElement;

describe("InputBuilderForm — nprocs field (ADR-025)", () => {
  it("is a raw-text field with an `auto` placeholder, empty by default", () => {
    renderForm();
    expect(nprocsField().type).toBe("text");
    expect(nprocsField().inputMode).toBe("numeric");
    expect(nprocsField().placeholder).toBe("auto");
    expect(nprocsField().value).toBe("");
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("clearing_the_field_yields_no_pal", () => {
    // BITE: `Number(value)` (empty → 0) would emit `%pal nprocs 0 end` here.
    const { generate, lastInput } = renderForm();
    typeNprocs("6");
    typeNprocs("");
    generate();
    expect(palLines(lastInput())).toEqual([]);
  });

  it("typing_6_emits_exactly_one_pal_6", () => {
    const { generate, lastInput } = renderForm();
    typeNprocs(" 6 "); // surrounding whitespace is trimmed
    generate();
    expect(palLines(lastInput())).toEqual(["%pal nprocs 6 end"]);
  });

  it.each(["0", "2.5", "-1", "abc"])(
    "invalid %j: alert visible, Generate disabled, onGenerate never called",
    (bad) => {
      // BITE: dropping the `disabled` predicate, or mapping invalid to null (auto), turns this red.
      const { onGenerate } = renderForm();
      typeNprocs(bad);
      expect(screen.getByRole("alert").textContent).toMatch(/nprocs/);
      expect(generateButton().disabled).toBe(true);
      fireEvent.click(generateButton());
      expect(onGenerate).not.toHaveBeenCalled();
    },
  );

  it("a valid integer after an invalid one re-enables Generate and emits it", () => {
    const { onGenerate, generate, lastInput } = renderForm();
    typeNprocs("2.5");
    expect(generateButton().disabled).toBe(true);
    typeNprocs("12");
    expect(screen.queryByRole("alert")).toBeNull();
    expect(generateButton().disabled).toBe(false);
    generate();
    expect(onGenerate).toHaveBeenCalledTimes(1);
    expect(palLines(lastInput())).toEqual(["%pal nprocs 12 end"]);
  });

  it("clearing an invalid field (= auto) re-enables Generate with no `%pal`", () => {
    const { generate, lastInput } = renderForm();
    typeNprocs("abc");
    expect(generateButton().disabled).toBe(true);
    typeNprocs("");
    expect(generateButton().disabled).toBe(false);
    generate();
    expect(palLines(lastInput())).toEqual([]);
  });

  it("an invalid value never leaks into the state: fixing it emits the fixed value only", () => {
    // 6 → "6x" (invalid, disabled) → "8": the stale 6 must not be what is emitted.
    const { generate, lastInput } = renderForm();
    typeNprocs("6");
    typeNprocs("6x");
    typeNprocs("8");
    generate();
    expect(palLines(lastInput())).toEqual(["%pal nprocs 8 end"]);
  });
});

describe("InputBuilderForm — nprocs with jobType NEB-TS (ADR-025 item 9)", () => {
  // Two parsed jobs sharing atom order (N, C); the reactant input carries the (charge, mult).
  const GEOM = { elements: ["N", "C"], xyz_angstrom: [[0, 0, 0], [2, 0, 0]] };
  beforeEach(() => {
    invokeMock.mockReset();
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "list_jobs") {
        return [
          { id: "r", title: "reactant job", status: "parsed" },
          { id: "p", title: "product job", status: "parsed" },
        ];
      }
      if (cmd === "get_job") return { input_content: "! Opt\n* xyz 0 1\nN 0 0 0\nC 2 0 0\n*\n" };
      if (cmd === "read_job_results") return { final_geometry: GEOM };
      throw new Error(`unexpected invoke ${cmd}`);
    });
  });

  /** Render, switch to NEB-TS and pick two parsed jobs; returns the NEB Generate button. */
  async function renderNeb() {
    const out = renderForm();
    const jobType = screen
      .getAllByRole("combobox")
      .find((el) => (el as HTMLSelectElement).value === "Opt Freq")!;
    fireEvent.change(jobType, { target: { value: "NEB-TS" } });
    await screen.findAllByRole("option", { name: "reactant job" });
    const [reactant, product] = screen.getAllByDisplayValue("— pick a parsed job —");
    fireEvent.change(reactant, { target: { value: "r" } });
    fireEvent.change(product, { target: { value: "p" } });
    const nebButton = () => screen.getByText("Generate NEB input") as HTMLButtonElement;
    return { ...out, nebButton };
  }

  it.each(["0", "2.5", "-1", "abc"])(
    "invalid %j disables the NEB Generate",
    async (bad) => {
      // BITE: dropping the `nprocsValid` prop from the NEB `canGenerate` turns this red.
      const { onGenerate, nebButton } = await renderNeb();
      expect(nebButton().disabled).toBe(false); // both jobs picked, nprocs auto: enabled
      typeNprocs(bad);
      expect(screen.getByRole("alert")).toBeTruthy();
      expect(nebButton().disabled).toBe(true);
      fireEvent.click(nebButton());
      expect(onGenerate).not.toHaveBeenCalled();
    },
  );

  it("a valid integer re-enables the NEB Generate and emits exactly that `%pal`", async () => {
    const { onGenerate, nebButton } = await renderNeb();
    typeNprocs("abc");
    expect(nebButton().disabled).toBe(true);
    typeNprocs("6");
    expect(nebButton().disabled).toBe(false);
    fireEvent.click(nebButton());
    await waitFor(() => expect(onGenerate).toHaveBeenCalledTimes(1));
    const inp = String(onGenerate.mock.calls[0][0]);
    expect(palLines(inp)).toEqual(["%pal nprocs 6 end"]);
    expect(inp).toContain("NEB-TS");
  });

  it("auto (blank) emits the NEB input with no `%pal`", async () => {
    const { onGenerate, nebButton } = await renderNeb();
    fireEvent.click(nebButton());
    await waitFor(() => expect(onGenerate).toHaveBeenCalledTimes(1));
    expect(palLines(String(onGenerate.mock.calls[0][0]))).toEqual([]);
  });
});
