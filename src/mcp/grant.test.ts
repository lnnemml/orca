// The grant card's and editor's pure logic (ADR-027 M4 amendment item 5). Times are shown in the
// laptop's local time, so the zone is Europe/Kyiv here too (set and restored as in `format.test.ts`).
//
// Control (`m4a_parta_controls.py`): REPLACE-FRESH — the replace confirm decided from the card's
// snapshot instead of the fresh read → red.
import { afterAll, beforeAll, describe, expect, it, vi } from "vitest";
import type { GrantView } from "../types";
import {
  DEFAULT_MAX_IN_FLIGHT,
  DEFAULT_MAX_JOBS,
  chargedLabel,
  createGrantFlow,
  grantStateLabel,
  grantTimeLabel,
  inFlightLabel,
  replacesActive,
  revokeConfirmText,
  targetsLabel,
  type CreateGrantArgs,
  type CreateGrantDeps,
} from "./grant";

let outerTz: string | undefined;
beforeAll(() => {
  outerTz = process.env.TZ;
  process.env.TZ = "Europe/Kyiv";
});
afterAll(() => {
  if (outerTz === undefined) delete process.env.TZ;
  else process.env.TZ = outerTz;
  expect(process.env.TZ).toBe(outerTz);
});

const ACTIVE: GrantView = {
  id: "g1",
  state: "active",
  created_at: "2026-11-21 08:00:00",
  expires_at: "2026-11-21 16:00:00",
  revoked_at: null,
  targets: ["local", "p-uni"],
  levels: ["CPCM(DMF) WB97X-3C"],
  max_jobs: 5,
  max_in_flight: 2,
  charged_jobs: 3,
  in_flight: 1,
};
const REVOKED: GrantView = { ...ACTIVE, state: "revoked", revoked_at: "2026-11-21 09:30:00" };
const EXPIRED: GrantView = { ...ACTIVE, state: "expired" };
const ARGS: CreateGrantArgs = { targets: ["local"], levels: ["! wB97X-3c\n"], maxJobs: 5, maxInFlight: 1, expiresAt: "2026-11-22 00:00:00" };

describe("the grant card", () => {
  it("shows the counters, in flight across all grants", () => {
    expect(chargedLabel(ACTIVE)).toBe("3 of 5 jobs charged");
    expect(inFlightLabel(ACTIVE)).toBe("1 of 2 in flight (all grants)");
    expect(grantStateLabel(ACTIVE)).toBe("Active");
    expect([DEFAULT_MAX_JOBS, DEFAULT_MAX_IN_FLIGHT]).toEqual([5, 1]);
  });

  it("shows the expiry or the revocation in local time", () => {
    expect(grantTimeLabel(ACTIVE)).toBe("expires 2026-11-21 18:00:00");
    expect(grantTimeLabel(EXPIRED)).toBe("expired 2026-11-21 18:00:00");
    expect(grantTimeLabel(REVOKED)).toBe("revoked 2026-11-21 11:30:00");
  });

  it("names a server target by its profile when known, and by its id otherwise", () => {
    expect(targetsLabel(ACTIVE, (id) => (id === "p-uni" ? "uni" : undefined))).toBe("local, uni");
    expect(targetsLabel(ACTIVE, () => undefined)).toBe("local, p-uni");
  });

  it("confirms a revoke with what stays running", () => {
    expect(revokeConfirmText(ACTIVE)).toContain("running jobs are not cancelled");
  });
});

/** Deps that record the order of calls; `fresh` is what `mcp_get_grant` answers at the click. */
function deps(fresh: GrantView | null, confirmAnswer = true) {
  const order: string[] = [];
  const d: CreateGrantDeps = {
    readGrant: vi.fn(async () => {
      order.push("read");
      return fresh;
    }),
    confirm: vi.fn(async () => {
      order.push("confirm");
      return confirmAnswer;
    }),
    create: vi.fn(async () => {
      order.push("create");
      return { ...ACTIVE, id: "g2" };
    }),
  };
  return { d, order };
}

describe("Create and the replace confirm", () => {
  it("decides from the active state alone", () => {
    expect(replacesActive(ACTIVE)).toBe(true);
    for (const g of [null, REVOKED, EXPIRED]) expect(replacesActive(g)).toBe(false);
  });

  // REPLACE-FRESH: the card showed no grant, but one was made since: the fresh read decides.
  it("confirms when the fresh read shows an active grant the card did not show", async () => {
    const { d, order } = deps(ACTIVE);
    const out = await createGrantFlow(d, ARGS, null);
    expect(order, "REPLACE-FRESH: no confirm while a fresh read shows an active grant").toEqual(["read", "confirm", "create"]);
    expect(vi.mocked(d.confirm).mock.calls[0][0]).toContain("This replaces the active grant (3 of 5 jobs charged");
    expect(vi.mocked(d.confirm).mock.calls[0][0]).toContain("changed since this screen last read it");
    expect(d.create).toHaveBeenCalledWith(ARGS);
    expect(out).toEqual({ kind: "created", grant: { ...ACTIVE, id: "g2" } });
  });

  // REPLACE-FRESH: the card showed an active grant, but it was revoked or expired since.
  it("does not confirm when the fresh read shows no active grant, whatever the card showed", async () => {
    for (const fresh of [null, REVOKED, EXPIRED]) {
      const { d, order } = deps(fresh);
      await createGrantFlow(d, ARGS, ACTIVE);
      expect(order, `REPLACE-FRESH: ${fresh?.state ?? "none"}`).toEqual(["read", "create"]);
    }
  });

  it("reads at the click, once, and a declined confirm creates nothing", async () => {
    const { d, order } = deps(ACTIVE, false);
    const out = await createGrantFlow(d, ARGS, ACTIVE);
    expect(d.readGrant).toHaveBeenCalledTimes(1);
    expect(order).toEqual(["read", "confirm"]);
    expect(d.create).not.toHaveBeenCalled();
    expect(out).toEqual({ kind: "declined", fresh: ACTIVE });
    expect(vi.mocked(d.confirm).mock.calls[0][0]).not.toContain("changed since");
  });

  it("passes a refusal of the core through unchanged", async () => {
    const { d } = deps(null);
    d.create = async () => {
      throw "invalid input: level 1: names no method keyword";
    };
    await expect(createGrantFlow(d, ARGS, null)).rejects.toBe("invalid input: level 1: names no method keyword");
  });
});
