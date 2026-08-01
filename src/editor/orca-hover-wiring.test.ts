import { describe, it, expect, vi } from "vitest";

import { orcaLanguageId } from "./orca-language";
import { orcaEditorOptions } from "./editor-options";

// The wiring layer — registration, not the pure lookup — is where the hover broke while
// every pure-function test stayed green (`wiki/debugging/010-hover-clipped-on-top-line.md`).
// These tests exercise that layer with a FAKE monaco (no real Monaco, no jsdom): they
// record what `registerOrcaHover` calls and assert the wire is intact.

function makeFakeMonaco(opts: { commandThrows?: boolean } = {}) {
  const hoverRegistrations: Array<{ languageId: string; provider: unknown }> = [];
  const commandRegistrations: Array<{ id: string }> = [];
  const monaco = {
    languages: {
      registerHoverProvider(languageId: string, provider: unknown) {
        hoverRegistrations.push({ languageId, provider });
        return { dispose() {} };
      },
    },
    editor: {
      registerCommand(id: string, _handler: unknown) {
        if (opts.commandThrows) throw new Error("registerCommand boom");
        commandRegistrations.push({ id });
        return { dispose() {} };
      },
    },
  };
  return { monaco, hoverRegistrations, commandRegistrations };
}

// `registerOrcaHover` guards itself with a module-level `registered` flag, so each
// scenario needs a fresh module instance.
async function freshRegisterOrcaHover() {
  vi.resetModules();
  const mod = await import("./orca-hover");
  return mod.registerOrcaHover;
}

describe("orca hover wiring (fake monaco)", () => {
  it("registers a hover provider for the SAME language id the <Editor> is given", async () => {
    const registerOrcaHover = await freshRegisterOrcaHover();
    const fake = makeFakeMonaco();

    // InputEditor passes this exact `orcaLanguageId` both to <Editor language={…}> and to
    // registerOrcaHover; a mismatch here is precisely the (excluded, but untested) failure
    // mode where the provider is registered for a language the model never uses.
    registerOrcaHover(fake.monaco as never, orcaLanguageId);

    expect(fake.hoverRegistrations).toHaveLength(1);
    expect(fake.hoverRegistrations[0].languageId).toBe(orcaLanguageId);
  });

  it("still registers the hover when the OPTIONAL open-in-drawer command throws", async () => {
    const registerOrcaHover = await freshRegisterOrcaHover();
    const fake = makeFakeMonaco({ commandThrows: true });
    const errSpy = vi.spyOn(console, "error").mockImplementation(() => {});

    // The command is optional (it only powers a click-to-open link). Its failure must not
    // take the mandatory hover down with it — hover is registered FIRST, command is guarded.
    expect(() => registerOrcaHover(fake.monaco as never, orcaLanguageId)).not.toThrow();

    expect(fake.hoverRegistrations).toHaveLength(1); // hover survived the command failure
    expect(fake.commandRegistrations).toHaveLength(0); // the command did fail
    errSpy.mockRestore();
  });

  it("pins fixedOverflowWidgets so a hover on the top `!` line is not clipped", () => {
    // The measured root cause: without this the popup on line 1 renders inside the editor's
    // overflow:hidden guard and is clipped. Removing it would silently reintroduce the bug.
    expect(orcaEditorOptions.fixedOverflowWidgets).toBe(true);
  });
});
