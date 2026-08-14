import type { Group } from "../types";

/**
 * The internal `<option value>` for the "(ungrouped)" choice. A `<select>` value cannot be
 * `null`, so the ungrouped choice needs a sentinel — but it **NEVER escapes** the component:
 * `onChange` always emits a clean `string | null`. This is the **canonical** home for the
 * sentinel; `JobsScreen` / `GroupSidebar` import it from here rather than re-declaring it.
 */
export const ROOT_OPTION = "__root__";

/**
 * Map a raw `<option>` value to the clean group id the consumer wants: the ungrouped
 * sentinel → `null`, any real group id → itself. The sentinel never leaves the component.
 */
export function groupIdFromOptionValue(value: string): string | null {
  return value === ROOT_OPTION ? null : value;
}

/** The inverse: a group id (or `null` = ungrouped) → the `<select>` value to show selected. */
export function optionValueFromGroupId(groupId: string | null): string {
  return groupId ?? ROOT_OPTION;
}

/**
 * Whether a New-Job create should call `move_job`, and with what group id, given the picker
 * state (`groupTouched` + the picked value). Encodes the **zero-regression + explicit-pick-wins**
 * rule for the New Job destination picker:
 *
 * - **untouched + no active group** (`pickedGroupId == null`) → **NO-OP**: exactly today's
 *   assign-on-create behavior, and — load-bearing — it does NOT clobber a create-path default,
 *   e.g. a NEB-TS's reactant-inherited group (Unit 1, stamped in Rust);
 * - **untouched + an active group** → assign it (the 4.7.3 assign-on-create);
 * - **an explicit pick** → the picker WINS, including `"(ungrouped)"` (`null`), overriding any
 *   default.
 *
 * Pure so this branch is a testable bite rather than an inline guard buried in an async handler.
 */
export function resolveGroupAssignment(
  groupTouched: boolean,
  pickedGroupId: string | null,
): { assign: false } | { assign: true; groupId: string | null } {
  if (!groupTouched && pickedGroupId == null) return { assign: false };
  return { assign: true, groupId: pickedGroupId };
}

interface GroupSelectProps {
  groups: Group[];
  /** The selected group id, or `null` for ungrouped. Controlled. */
  value: string | null;
  /** Fires with a CLEAN group id (or `null` for ungrouped) — never the internal sentinel. */
  onChange: (groupId: string | null) => void;
  className?: string;
  id?: string;
  disabled?: boolean;
  "aria-label"?: string;
}

/**
 * A reusable controlled dropdown of existing groups plus an "(ungrouped)" choice. Purely
 * **presentational** — no `invoke` / store / `move_job`: the consumer decides what to do with
 * the emitted `string | null` (New Job assigns via `move_job`, JobsScreen re-groups likewise).
 * A flat list (mirrors JobsScreen's Move-to select); it **picks among EXISTING groups only** —
 * no create affordance, no drag-and-drop.
 */
export function GroupSelect({
  groups,
  value,
  onChange,
  className,
  id,
  disabled,
  "aria-label": ariaLabel,
}: GroupSelectProps) {
  return (
    <select
      className={className ?? "select"}
      id={id}
      disabled={disabled}
      aria-label={ariaLabel ?? "Group"}
      value={optionValueFromGroupId(value)}
      onChange={(e) => onChange(groupIdFromOptionValue(e.currentTarget.value))}
    >
      <option value={ROOT_OPTION}>(ungrouped)</option>
      {groups.map((g) => (
        <option key={g.id} value={g.id}>
          {g.name}
        </option>
      ))}
    </select>
  );
}
