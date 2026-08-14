import { describe, it, expect, vi, beforeEach } from "vitest";

// The Tauri bridges are mocked — these are unit tests over the plumbing, not the app.
const invokeMock = vi.fn();
const openMock = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: (...args: unknown[]) => openMock(...args),
  save: vi.fn(),
}));

import { exportGroup } from "./save";

describe("exportGroup — group export plumbing (ADR-021)", () => {
  beforeEach(() => {
    invokeMock.mockReset();
    openMock.mockReset();
  });

  it("invokes export_group with the chosen folder and copy mode, returns the path", async () => {
    openMock.mockResolvedValue("/home/anton/exports");
    invokeMock.mockResolvedValue("/home/anton/exports/hcn-reduction-export");

    const path = await exportGroup("g-1", "curated");

    expect(openMock).toHaveBeenCalledWith({
      directory: true,
      title: expect.any(String),
    });
    expect(invokeMock).toHaveBeenCalledWith("export_group", {
      groupId: "g-1",
      destParent: "/home/anton/exports",
      copyMode: "curated",
    });
    expect(path).toBe("/home/anton/exports/hcn-reduction-export");
  });

  it("passes copyMode through for a full export", async () => {
    openMock.mockResolvedValue("/tmp/out");
    invokeMock.mockResolvedValue("/tmp/out/study-export");
    await exportGroup("g-2", "full");
    expect(invokeMock).toHaveBeenCalledWith("export_group", {
      groupId: "g-2",
      destParent: "/tmp/out",
      copyMode: "full",
    });
  });

  it("returns null and does NOT invoke when the folder picker is cancelled", async () => {
    openMock.mockResolvedValue(null);
    const path = await exportGroup("g-1", "full");
    expect(path).toBeNull();
    expect(invokeMock).not.toHaveBeenCalled();
  });
});
