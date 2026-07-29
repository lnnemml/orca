import { describe, it, expect } from "vitest";

import { describeSidecarError } from "./sidecar-client";

describe("describeSidecarError", () => {
  it("404 → the endpoint is missing; the message names the route and says restart", () => {
    const e = describeSidecarError({
      status: 404,
      detail: "Not Found",
      route: "/geometry/set-internal",
    });
    expect(e.kind).toBe("missing-endpoint");
    expect(e.message).toMatch(/older build/i);
    expect(e.message).toMatch(/restart/i);
    expect(e.message).toContain("/geometry/set-internal"); // names the route
    expect(e.message).not.toBe("Not Found"); // never the bare FastAPI text
  });

  it("422 → shows the validation detail verbatim (already human)", () => {
    const detail = "reference atoms [0] must NOT be in the mask";
    const e = describeSidecarError({ status: 422, detail, route: "/x" });
    expect(e.kind).toBe("validation");
    expect(e.message).toBe(detail);
  });

  it("500 → server error, shows the post-condition detail", () => {
    const detail = "distance not reached: target 1.5, measured 1.7";
    const e = describeSidecarError({ status: 500, detail, route: "/x" });
    expect(e.kind).toBe("server-error");
    expect(e.message).toBe(detail);
  });

  it("500 with no detail still reads as an internal inconsistency", () => {
    const e = describeSidecarError({ status: 500, route: "/x" });
    expect(e.kind).toBe("server-error");
    expect(e.message).toMatch(/inconsistency|post-condition/i);
  });

  it("network → the sidecar isn't running", () => {
    const e = describeSidecarError({ status: "network", route: "/x" });
    expect(e.kind).toBe("not-running");
    expect(e.message).toMatch(/isn't running/i);
  });

  it("a bare 404 body still yields a route-named message (no silent 'Not Found')", () => {
    const e = describeSidecarError({ status: 404, route: "/formats" });
    expect(e.message).toContain("/formats");
  });
});
