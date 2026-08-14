import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { confirm, message } from "@tauri-apps/plugin-dialog";

import type { Group } from "../types";
import { exportGroup, type CopyMode } from "../export/save";
import {
  buildGroupTree,
  moveTargetsFor,
  type GroupNode,
  type GroupSelection,
} from "./tree";

interface GroupSidebarProps {
  groups: Group[];
  selection: GroupSelection;
  /** Set the active selection (lifted to App — the single source of truth). */
  onSelect: (sel: GroupSelection) => void;
  /** Reload groups + jobs after a mutation (create/rename/move/delete). */
  onChanged: () => void | Promise<void>;
  onError: (msg: string) => void;
}

/** Sentinel value for the "(root)" option in a native <select> (a select value
 * can't be null). */
const ROOT_OPTION = "__root__";

/**
 * The group tree sidebar of the Jobs view (Phase 4.7.3, ADR-019). Renders the two
 * special roots ("All jobs", "Ungrouped") plus `buildGroupTree(groups)` with
 * expand/collapse. All mutations compose the existing 4.7.2 commands
 * (`create_group` / `rename_group` / `move_group` / `delete_group`); there is **no
 * drag-and-drop** — moving a group uses an explicit picker whose targets exclude the
 * group's own self + descendants (`moveTargetsFor`), so it can never offer a cycle.
 */
export function GroupSidebar({
  groups,
  selection,
  onSelect,
  onChanged,
  onError,
}: GroupSidebarProps) {
  const [collapsed, setCollapsed] = useState<Set<string>>(new Set());
  // Inline name entry (native window.prompt is unreliable in WebKitGTK; an inline
  // input mirrors ReactionsScreen). `undefined` = not creating; `null` = at root;
  // a string = under that group id.
  const [creatingParent, setCreatingParent] = useState<string | null | undefined>(undefined);
  const [draftName, setDraftName] = useState("");
  const [renamingId, setRenamingId] = useState<string | null>(null);
  const [renameValue, setRenameValue] = useState("");
  const [movingId, setMovingId] = useState<string | null>(null);
  // Inline export-mode chooser (mirrors the move picker). `null` = not exporting.
  const [exportingId, setExportingId] = useState<string | null>(null);

  const tree = buildGroupTree(groups);

  const toggle = (id: string) =>
    setCollapsed((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });

  const submitCreate = async () => {
    const name = draftName.trim();
    if (!name) {
      setCreatingParent(undefined);
      return;
    }
    try {
      await invoke("create_group", { name, parentId: creatingParent ?? null });
      setCreatingParent(undefined);
      setDraftName("");
      await onChanged();
    } catch (e) {
      onError(String(e));
    }
  };

  const submitRename = async (id: string) => {
    const name = renameValue.trim();
    if (!name) {
      setRenamingId(null);
      return;
    }
    try {
      await invoke("rename_group", { id, name });
      setRenamingId(null);
      await onChanged();
    } catch (e) {
      onError(String(e));
    }
  };

  const removeGroup = async (group: Group) => {
    const ok = await confirm(
      `Delete "${group.name}"? Its sub-groups and jobs move up to the parent — ` +
        "no job is deleted.",
      { title: "Delete group", kind: "warning" },
    );
    if (!ok) return;
    try {
      await invoke("delete_group", { id: group.id });
      // If the deleted group was selected, fall back to "All jobs".
      if (selection.kind === "group" && selection.id === group.id) {
        onSelect({ kind: "all" });
      }
      await onChanged();
    } catch (e) {
      onError(String(e));
    }
  };

  const runExport = async (group: Group, mode: CopyMode) => {
    setExportingId(null);
    try {
      const path = await exportGroup(group.id, mode);
      if (path) {
        await message(`Exported “${group.name}” to:\n${path}`, {
          title: "Export complete",
        });
      }
    } catch (e) {
      onError(String(e));
    }
  };

  const submitMove = async (id: string, target: string) => {
    try {
      await invoke("move_group", {
        id,
        newParentId: target === ROOT_OPTION ? null : target,
      });
      setMovingId(null);
      await onChanged();
    } catch (e) {
      onError(String(e));
    }
  };

  const renderNode = (node: GroupNode, depth: number) => {
    const { group, children } = node;
    const hasChildren = children.length > 0;
    const isCollapsed = collapsed.has(group.id);
    const isSelected = selection.kind === "group" && selection.id === group.id;

    return (
      <div key={group.id}>
        <div
          className={"group-row" + (isSelected ? " selected" : "")}
          style={{ paddingLeft: 8 + depth * 14 }}
        >
          <button
            className="group-twisty"
            onClick={() => hasChildren && toggle(group.id)}
            style={{ visibility: hasChildren ? "visible" : "hidden" }}
            title={isCollapsed ? "Expand" : "Collapse"}
          >
            {isCollapsed ? "▶" : "▼"}
          </button>

          {renamingId === group.id ? (
            <input
              className="input group-rename"
              value={renameValue}
              autoFocus
              onChange={(e) => setRenameValue(e.currentTarget.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") submitRename(group.id);
                if (e.key === "Escape") setRenamingId(null);
              }}
              onBlur={() => submitRename(group.id)}
            />
          ) : (
            <button
              className="group-name"
              onClick={() => onSelect({ kind: "group", id: group.id })}
              title={group.name}
            >
              {group.name}
            </button>
          )}

          <span className="group-actions">
            <button
              className="icon-btn"
              title="New subgroup"
              onClick={() => {
                setCreatingParent(group.id);
                setDraftName("");
              }}
            >
              ＋
            </button>
            <button
              className="icon-btn"
              title="Rename"
              onClick={() => {
                setRenamingId(group.id);
                setRenameValue(group.name);
              }}
            >
              ✎
            </button>
            <button
              className="icon-btn"
              title="Move to…"
              onClick={() => setMovingId(movingId === group.id ? null : group.id)}
            >
              ⇄
            </button>
            <button
              className="icon-btn"
              title="Export…"
              onClick={() => setExportingId(exportingId === group.id ? null : group.id)}
            >
              ⭳
            </button>
            <button
              className="icon-btn"
              title="Delete group"
              onClick={() => removeGroup(group)}
            >
              🗑
            </button>
          </span>
        </div>

        {exportingId === group.id ? (
          <div className="group-export" style={{ paddingLeft: 8 + depth * 14 + 20 }}>
            <span className="muted">Export:</span>
            <button className="btn btn-sm" onClick={() => runExport(group, "curated")}>
              Curated…
            </button>
            <button className="btn btn-sm" onClick={() => runExport(group, "full")}>
              Full…
            </button>
            <button className="icon-btn" title="Cancel" onClick={() => setExportingId(null)}>
              ✕
            </button>
          </div>
        ) : null}

        {movingId === group.id ? (
          <div className="group-move" style={{ paddingLeft: 8 + depth * 14 + 20 }}>
            <span className="muted">Move to:</span>
            <select
              className="select group-move-select"
              defaultValue=""
              onChange={(e) => e.currentTarget.value && submitMove(group.id, e.currentTarget.value)}
            >
              <option value="" disabled>
                choose…
              </option>
              <option value={ROOT_OPTION}>(root)</option>
              {moveTargetsFor(groups, group.id).map((t) => (
                <option key={t.id} value={t.id}>
                  {t.name}
                </option>
              ))}
            </select>
            <button className="icon-btn" title="Cancel" onClick={() => setMovingId(null)}>
              ✕
            </button>
          </div>
        ) : null}

        {creatingParent === group.id ? (
          <div className="group-create" style={{ paddingLeft: 8 + depth * 14 + 20 }}>
            <input
              className="input"
              placeholder="Subgroup name…"
              value={draftName}
              autoFocus
              onChange={(e) => setDraftName(e.currentTarget.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") submitCreate();
                if (e.key === "Escape") setCreatingParent(undefined);
              }}
              onBlur={submitCreate}
            />
          </div>
        ) : null}

        {hasChildren && !isCollapsed
          ? children.map((c) => renderNode(c, depth + 1))
          : null}
      </div>
    );
  };

  return (
    <aside className="group-sidebar">
      <div className="group-special">
        <button
          className={"group-row special" + (selection.kind === "all" ? " selected" : "")}
          onClick={() => onSelect({ kind: "all" })}
        >
          All jobs
        </button>
        <button
          className={"group-row special" + (selection.kind === "ungrouped" ? " selected" : "")}
          onClick={() => onSelect({ kind: "ungrouped" })}
        >
          Ungrouped
        </button>
      </div>

      <div className="group-tree">{tree.map((n) => renderNode(n, 0))}</div>

      {creatingParent === null ? (
        <div className="group-create">
          <input
            className="input"
            placeholder="Group name…"
            value={draftName}
            autoFocus
            onChange={(e) => setDraftName(e.currentTarget.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") submitCreate();
              if (e.key === "Escape") setCreatingParent(undefined);
            }}
            onBlur={submitCreate}
          />
        </div>
      ) : (
        <button
          className="btn btn-sm group-new"
          onClick={() => {
            setCreatingParent(null);
            setDraftName("");
          }}
        >
          ＋ New group
        </button>
      )}
    </aside>
  );
}
