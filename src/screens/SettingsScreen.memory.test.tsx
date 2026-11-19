// @vitest-environment jsdom
/**
 * Unit 5.5 at screen level (Phase 5 follow-up, 5.5 F1): saving the CPU preset in Settings re-reads
 * the local default `%maxcore` (ADR-024 o26.13.4) — the fan-out builders and the New Job builder
 * follow the preset's rank count. The screen is rendered whole with `invoke` mocked.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";

const invokeMock = vi.fn<(cmd: string, args?: Record<string, unknown>) => Promise<unknown>>();
vi.mock("@tauri-apps/api/core", () => ({ invoke: (cmd: string, args?: Record<string, unknown>) => invokeMock(cmd, args) }));
vi.mock("@tauri-apps/api/event", () => ({ listen: async () => () => {} }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ confirm: async () => true, message: async () => {}, open: async () => null }));

import { localMaxcore, setLocalMaxcore } from "../memory/local-maxcore";
import { SettingsScreen } from "./SettingsScreen";

/** The fitted default `local_fitted_maxcore` answers after the preset is saved (not 2000). */
const FITTED_AFTER_SAVE = 1234;

beforeEach(() => {
  invokeMock.mockReset();
  setLocalMaxcore(null);
  invokeMock.mockImplementation(async (cmd) => {
    switch (cmd) {
      case "get_settings":
        return { cpu_preset: "interactive" };
      case "get_cpu_presets":
        return [{ id: "interactive", label: "Interactive", mask: "8-15", nprocs: 8, description: "E-cores" }];
      case "api_key_status":
        return { state: "none" };
      case "local_fitted_maxcore":
        return { default_maxcore_mib: FITTED_AFTER_SAVE };
      case "list_server_profiles":
        return [];
      default:
        return null;
    }
  });
});
afterEach(() => {
  cleanup();
  setLocalMaxcore(null);
});

describe("SettingsScreen — the local %maxcore default follows the CPU preset (o26.13.4)", () => {
  // CONTROL (`run_p5fu_controls.sh`, `settings-no-reload-after-preset-save`): `saveCpu` without
  // `reloadLocalMaxcore()` → the accessor stays at 2000, red.
  it("re-reads the local default after the preset is saved", async () => {
    render(<SettingsScreen />);
    fireEvent.click(await screen.findByRole("button", { name: "Save CPU settings" }));
    await waitFor(() => expect(localMaxcore()).toBe(FITTED_AFTER_SAVE));
    expect(invokeMock).toHaveBeenCalledWith("set_setting", { key: "cpu_preset", value: "interactive" });
    expect(invokeMock).toHaveBeenCalledWith("local_fitted_maxcore", undefined);
  });

  it("does not re-read it when saving the preset failed", async () => {
    const base = invokeMock.getMockImplementation()!;
    invokeMock.mockImplementation(async (cmd, args) => {
      if (cmd === "set_setting") throw new Error("disk full");
      return base(cmd, args);
    });
    render(<SettingsScreen />);
    fireEvent.click(await screen.findByRole("button", { name: "Save CPU settings" }));
    await screen.findByText(/disk full/);
    expect(localMaxcore()).toBe(2000);
    expect(invokeMock.mock.calls.some(([cmd]) => cmd === "local_fitted_maxcore")).toBe(false);
  });
});
