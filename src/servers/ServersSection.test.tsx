// @vitest-environment jsdom
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent, cleanup, waitFor, within } from "@testing-library/react";

// The Tauri bridge is mocked: these test the component over canned command results.
const invokeMock = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

import { ServersSection } from "./ServersSection";
import { makeProfile } from "./test-fixtures";
import type { ConnTestReport, ServerProfile } from "../types";

afterEach(cleanup);

const CHECKS = [
  { check: "orca", passed: true, reason: null },
  { check: "cores", passed: true, reason: null },
  { check: "core_mask", passed: true, reason: null },
  { check: "kill_user_processes", passed: true, reason: null },
  { check: "root", passed: true, reason: null },
] as const;

const VERIFIED_PROFILE = makeProfile({
  orca_version: "6.1.1",
  openmpi_version: "4.1.6",
  core_count: 48,
  verified_at: "2026-10-03 14:20:03",
  run_target: { is_run_target: true, reason: null },
});

/** Route each command to a handler; any other command fails the test. */
function commands(handlers: Record<string, (args?: Record<string, unknown>) => unknown>) {
  invokeMock.mockImplementation(async (cmd: string, args?: Record<string, unknown>) => {
    const h = handlers[cmd];
    if (!h) throw new Error(`unexpected command ${cmd}`);
    return h(args);
  });
}

function row(id = "p1") {
  return within(screen.getByTestId(`server-${id}`));
}

describe("ServersSection", () => {
  beforeEach(() => {
    invokeMock.mockReset();
  });

  it("lists profiles with the run-target headline and the not-a-run-target reason", async () => {
    commands({
      list_server_profiles: () => [
        makeProfile(),
        makeProfile({
          id: "p2",
          name: "verified, no mask",
          core_mask: null,
          verified_at: "2026-10-03 14:20:03",
          orca_version: "6.1.1",
          core_count: 48,
          run_target: { is_run_target: false, reason: "the profile has no core mask" },
        }),
      ],
    });
    render(<ServersSection />);
    await screen.findByTestId("server-p1");
    expect(row("p1").getByTestId("run-target").textContent).toBe(
      "Not a run target: the profile has not passed the connection test",
    );
    expect(row("p1").getByTestId("verification").textContent).toBe("Not verified");
    expect(row("p2").getByTestId("run-target").textContent).toBe(
      "Not a run target: the profile has no core mask",
    );
    expect(row("p2").getByTestId("verification").textContent).toContain(
      "Connection test passed 2026-10-03 14:20:03 UTC",
    );
  });

  it("Test connection shows a spinner, then the checks, facts and warnings", async () => {
    let finish: (r: ConnTestReport) => void = () => {};
    const report: ConnTestReport = {
      outcome: "verified",
      checks: [...CHECKS],
      facts: { orca_version: "6.1.1", openmpi_version: "4.1.6", core_count: 48 },
      warnings: [{ kind: "sudo_group" }],
      profile: VERIFIED_PROFILE,
      elapsed_ms: 1114,
    };
    commands({
      list_server_profiles: () => [makeProfile()],
      test_server_profile: () => new Promise<ConnTestReport>((resolve) => (finish = resolve)),
    });
    render(<ServersSection />);
    await screen.findByTestId("server-p1");

    fireEvent.click(row().getByRole("button", { name: "Test connection" }));
    const busy = await row().findByRole("button", { name: /Testing/ });
    expect((busy as HTMLButtonElement).disabled).toBe(true);
    expect(invokeMock).toHaveBeenCalledWith("test_server_profile", { id: "p1" });

    finish(report);
    await row().findByTestId("test-report");
    expect(row().getByTestId("test-summary").textContent).toContain(
      "Connection test passed; the profile is verified.",
    );
    expect(row().getByTestId("check-kill_user_processes").textContent).toContain("pass");
    expect(row().getByTestId("test-facts").textContent).toContain("ORCA 6.1.1");
    expect(row().getByTestId("test-facts").textContent).toContain("48 CPUs");
    expect(row().getByTestId("test-warning").textContent).toContain("sudo group");
    // The row now shows the profile Rust returned after the stamp.
    expect(row().getByTestId("run-target").textContent).toBe("Run target");
    expect(row().getByRole("button", { name: "Test connection" })).toBeTruthy();
  });

  it("a failed check shows FAIL with its reason, and the cleared state", async () => {
    commands({
      list_server_profiles: () => [VERIFIED_PROFILE],
      test_server_profile: (): ConnTestReport => ({
        outcome: "not_passed",
        checks: [
          { check: "orca", passed: false, reason: "ORCA is missing or not executable (test -x failed)" },
          ...CHECKS.slice(1),
        ],
        warnings: [],
        profile: makeProfile(),
        elapsed_ms: 285,
      }),
    });
    render(<ServersSection />);
    await screen.findByTestId("server-p1");
    expect(row().getByTestId("run-target").textContent).toBe("Run target");

    fireEvent.click(row().getByRole("button", { name: "Test connection" }));
    await row().findByTestId("test-report");
    const orca = row().getByTestId("check-orca").textContent ?? "";
    expect(orca).toContain("FAIL");
    expect(orca).toContain("test -x failed");
    expect(row().getByTestId("test-summary").textContent).toContain("did not pass");
    expect(row().getByTestId("run-target").textContent).toBe(
      "Not a run target: the profile has not passed the connection test",
    );
    expect(row().queryByTestId("test-facts")).toBeNull();
  });

  it("a test that could not run shows the reason", async () => {
    commands({
      list_server_profiles: () => [makeProfile()],
      test_server_profile: (): ConnTestReport => ({
        outcome: "failed",
        reason: "ssh did not finish within 30 s and was killed",
        profile: makeProfile(),
        elapsed_ms: 30000,
      }),
    });
    render(<ServersSection />);
    await screen.findByTestId("server-p1");
    fireEvent.click(row().getByRole("button", { name: "Test connection" }));
    await row().findByTestId("test-report");
    expect(row().getByTestId("test-summary").textContent).toContain(
      "ssh did not finish within 30 s",
    );
  });

  it("shows a backend validation error in the form and keeps the form open", async () => {
    const created: Record<string, unknown>[] = [];
    commands({
      list_server_profiles: () => [],
      create_server_profile: (args) => {
        created.push(args ?? {});
        throw "invalid input: the host alias \"-oProxyCommand=sh\" starts with '-', which ssh would read as an option";
      },
    });
    render(<ServersSection />);
    fireEvent.click(await screen.findByRole("button", { name: "Add server" }));
    const form = within(screen.getByTestId("server-form"));
    fireEvent.change(form.getByLabelText("Name"), { target: { value: "uni" } });
    fireEvent.change(form.getByLabelText("SSH host alias"), {
      target: { value: "-oProxyCommand=sh" },
    });
    fireEvent.change(form.getByLabelText("Remote root"), {
      target: { value: "/home/anton/.orcastudio" },
    });
    fireEvent.click(form.getByRole("button", { name: "Add server" }));

    await form.findByTestId("form-error");
    expect(form.getByTestId("form-error").textContent).toContain("starts with '-'");
    expect(screen.getByTestId("server-form")).toBeTruthy();
    // Empty optional fields are sent as null, never "".
    expect(created[0]).toEqual({
      name: "uni",
      host: "-oProxyCommand=sh",
      remoteOrcaPath: "/opt/orca/orca",
      remoteScratchDir: "/home/anton/.orcastudio",
      coreMask: null,
      availabilityWindow: null,
    });
  });

  it("an edit saves through update_server_profile and reloads", async () => {
    let profiles: ServerProfile[] = [VERIFIED_PROFILE];
    commands({
      list_server_profiles: () => profiles,
      update_server_profile: (args) => {
        profiles = [makeProfile({ host: String(args?.host) })];
        return profiles[0];
      },
    });
    render(<ServersSection />);
    await screen.findByTestId("server-p1");
    fireEvent.click(row().getByRole("button", { name: "Edit" }));
    const form = within(screen.getByTestId("server-form"));
    expect(form.getByText(/clears the verification/)).toBeTruthy();
    fireEvent.change(form.getByLabelText("SSH host alias"), { target: { value: "uni2" } });
    fireEvent.click(form.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(screen.queryByTestId("server-form")).toBeNull());
    expect(invokeMock).toHaveBeenCalledWith(
      "update_server_profile",
      expect.objectContaining({ id: "p1", host: "uni2", coreMask: "0-3" }),
    );
    expect(row().getByTestId("run-target").textContent).toContain("Not a run target");
  });

  it("delete asks for confirmation, then deletes", async () => {
    let profiles: ServerProfile[] = [makeProfile()];
    commands({
      list_server_profiles: () => profiles,
      delete_server_profile: () => {
        profiles = [];
      },
    });
    render(<ServersSection />);
    await screen.findByTestId("server-p1");
    fireEvent.click(row().getByRole("button", { name: "Delete" }));
    expect(invokeMock).not.toHaveBeenCalledWith("delete_server_profile", expect.anything());
    fireEvent.click(row().getByRole("button", { name: "Confirm delete" }));
    await waitFor(() => expect(screen.queryByTestId("server-p1")).toBeNull());
    expect(invokeMock).toHaveBeenCalledWith("delete_server_profile", { id: "p1" });
  });

  // F3 on the frontend side: nothing here can stamp a profile.
  it("never invokes a stamp command", async () => {
    commands({
      list_server_profiles: () => [makeProfile()],
      test_server_profile: (): ConnTestReport => ({
        outcome: "failed",
        reason: "x",
        profile: makeProfile(),
        elapsed_ms: 1,
      }),
    });
    render(<ServersSection />);
    await screen.findByTestId("server-p1");
    fireEvent.click(row().getByRole("button", { name: "Test connection" }));
    await row().findByTestId("test-report");
    const names = invokeMock.mock.calls.map((c) => c[0]);
    expect(names).not.toContain("set_profile_verified");
  });
});
