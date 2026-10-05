/** Test-only fixtures for the Servers section (imported by the vitest files only). */
import type { ServerProfile } from "../types";

/** An unverified uni profile, as `list_server_profiles` returns it; override any field. */
export function makeProfile(over: Partial<ServerProfile> = {}): ServerProfile {
  return {
    id: "p1",
    name: "uni",
    host: "uni",
    remote_orca_path: "/opt/orca/orca",
    remote_scratch_dir: "/home/anton/.orcastudio",
    core_mask: "0-3",
    orca_version: null,
    openmpi_version: null,
    core_count: null,
    verified_at: null,
    created_at: "2026-10-03 14:20:01",
    slot_count: 1,
    availability_window: null,
    run_target: {
      is_run_target: false,
      reason: "the profile has not passed the connection test",
    },
    ...over,
  };
}
