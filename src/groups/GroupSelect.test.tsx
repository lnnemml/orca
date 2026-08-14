import { describe, it, expect, vi } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import type { ChangeEvent, ReactElement } from "react";

import {
  GroupSelect,
  ROOT_OPTION,
  groupIdFromOptionValue,
  optionValueFromGroupId,
  resolveGroupAssignment,
} from "./GroupSelect";
import type { Group } from "../types";

const groups: Group[] = [
  { id: "g1", name: "HCN reduction", parent_id: null, created_at: "t" },
  { id: "g2", name: "si-face", parent_id: "g1", created_at: "t" },
];

describe("GroupSelect — value mapping (the sentinel never escapes)", () => {
  it("maps the ungrouped sentinel to null, a real id to itself", () => {
    expect(groupIdFromOptionValue("g1")).toBe("g1");
    const out = groupIdFromOptionValue(ROOT_OPTION);
    expect(out).toBeNull();
    expect(out).not.toBe("__root__"); // sentinel-leak bite
  });

  it("shows null as the ungrouped option value, an id as itself", () => {
    expect(optionValueFromGroupId(null)).toBe(ROOT_OPTION);
    expect(optionValueFromGroupId("g2")).toBe("g2");
  });
});

describe("resolveGroupAssignment — the New Job assign decision", () => {
  it("untouched + no active group (null) → NO-OP (don't clobber a create-path default)", () => {
    // e.g. a NEB-TS's reactant-inherited group (Unit 1) must survive an untouched picker.
    expect(resolveGroupAssignment(false, null)).toEqual({ assign: false });
  });

  it("untouched + an active group → assign it (today's assign-on-create)", () => {
    expect(resolveGroupAssignment(false, "g1")).toEqual({ assign: true, groupId: "g1" });
  });

  it("explicit pick of a group → assign that group (picker wins)", () => {
    expect(resolveGroupAssignment(true, "g2")).toEqual({ assign: true, groupId: "g2" });
  });

  it("explicit pick of (ungrouped) → assign null, overriding any default (the override bite)", () => {
    // A touched null must FORCE ungrouped — not fall through to the untouched no-op.
    expect(resolveGroupAssignment(true, null)).toEqual({ assign: true, groupId: null });
  });
});

describe("GroupSelect — rendering (SSR, no jsdom in this project)", () => {
  it("renders every group plus the (ungrouped) option", () => {
    const html = renderToStaticMarkup(
      <GroupSelect groups={groups} value={null} onChange={() => {}} />,
    );
    expect(html).toContain("(ungrouped)");
    expect(html).toContain("HCN reduction");
    expect(html).toContain("si-face");
    expect(html).toContain('value="g1"');
    expect(html).toContain('value="g2"');
  });

  it("value=null → the (ungrouped) option is the selected one", () => {
    const html = renderToStaticMarkup(
      <GroupSelect groups={groups} value={null} onChange={() => {}} />,
    );
    // React marks the option matching the select's value as `selected` in SSR.
    expect(html).toMatch(/<option value="__root__"[^>]*selected[^>]*>\(ungrouped\)/);
  });

  it("value=id → that group's option is selected, not ungrouped", () => {
    const html = renderToStaticMarkup(
      <GroupSelect groups={groups} value="g1" onChange={() => {}} />,
    );
    expect(html).toMatch(/<option value="g1"[^>]*selected/);
    expect(html).not.toMatch(/<option value="__root__"[^>]*selected/);
  });
});

describe("GroupSelect — onChange wiring (drives the real handler)", () => {
  // No DOM in this project, so we invoke the plain function component and drive its own
  // onChange with a synthetic event — proving the real wiring
  // (DOM value -> groupIdFromOptionValue -> onChange) end to end.
  function handlerOf(
    value: string | null,
    spy: (g: string | null) => void,
  ): (e: ChangeEvent<HTMLSelectElement>) => void {
    const el = GroupSelect({ groups, value, onChange: spy }) as ReactElement<{
      value: string;
      onChange: (e: ChangeEvent<HTMLSelectElement>) => void;
    }>;
    expect(el.props.value).toBe(optionValueFromGroupId(value)); // controlled value wired
    return el.props.onChange;
  }

  it("selecting a group → onChange fires with that group's id (a string)", () => {
    const spy = vi.fn();
    handlerOf(null, spy)({ currentTarget: { value: "g2" } } as unknown as ChangeEvent<HTMLSelectElement>);
    expect(spy).toHaveBeenLastCalledWith("g2");
  });

  it("selecting (ungrouped) → onChange fires with null, NOT the sentinel", () => {
    const spy = vi.fn();
    handlerOf("g1", spy)({ currentTarget: { value: ROOT_OPTION } } as unknown as ChangeEvent<HTMLSelectElement>);
    expect(spy).toHaveBeenLastCalledWith(null);
    expect(spy).not.toHaveBeenCalledWith("__root__"); // sentinel-leak bite
  });
});
