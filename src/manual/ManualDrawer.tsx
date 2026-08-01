//! A side drawer that shows a manual section opened from an editor hover — WITHOUT
//! pulling the author out of the editor (task 4.4). It reuses the SAME `SectionView`
//! as `ManualScreen` (no second copy), and resolves the hover's descriptor to a DB row
//! through `resolve_manual_section` (the keywords.json→DB bridge, with a version check).

import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

import { SectionView } from "./SectionView";
import { orcaMapVersion, type SectionDescriptor } from "./keyword-lookup";
import { setManualOpenHandler } from "../editor/orca-hover";
import type { ManualSection } from "./types";

export function ManualDrawer() {
  const [section, setSection] = useState<ManualSection | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    setManualOpenHandler(async (d: SectionDescriptor) => {
      setError(null);
      try {
        setSection(
          await invoke<ManualSection>("resolve_manual_section", {
            file: d.file,
            breadcrumb: d.breadcrumb,
            title: d.title,
            nth: d.nth,
            mapVersion: orcaMapVersion, // → map_version; a stale map is reported, not resolved
          }),
        );
      } catch (e) {
        setError(String(e));
        setSection(null);
      }
    });
    return () => setManualOpenHandler(null);
  }, []);

  if (!section && !error) return null;
  return (
    <aside className="manual-drawer">
      <div className="manual-drawer-head">
        <span className="muted mono">Manual</span>
        <button
          className="btn btn-sm"
          onClick={() => {
            setSection(null);
            setError(null);
          }}
        >
          Close
        </button>
      </div>
      <div className="manual-drawer-body">
        {error ? (
          <div className="banner err">{error}</div>
        ) : section ? (
          <SectionView section={section} />
        ) : null}
      </div>
    </aside>
  );
}
