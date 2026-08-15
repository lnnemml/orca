// @vitest-environment jsdom
import { describe, it, expect, vi, afterEach } from "vitest";
import { render, screen, fireEvent, cleanup } from "@testing-library/react";

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
      %pal nprocs 4 end
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
