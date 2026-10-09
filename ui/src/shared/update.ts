// Mirror of crates/pulse-core/src/update.rs (serde camelCase) plus the IPC hook About uses.
// Ported from upstream App/AppUpdate.swift.
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useEffect, useState } from "react";

export interface UpdateState {
  current: string;
  /** False for a build that is not an installed release: "Built from source — no update check." */
  canCheck: boolean;
  checking: boolean;
  /** The last check couldn't reach the feed. */
  failed: boolean;
  /** The version a check found, if it is newer than this one. */
  newer: string | null;
  stage: "idle" | "downloading" | "installing" | "failed";
  downloaded: number;
  total: number | null;
}

/** Live update state: read once, then kept current from `update-changed`. */
export function useUpdate(): UpdateState | null {
  const [state, setState] = useState<UpdateState | null>(null);
  useEffect(() => {
    invoke<UpdateState>("update_state").then(setState);
    const un = listen<UpdateState>("update-changed", (e) => setState(e.payload));
    return () => void un.then((f) => f());
  }, []);
  return state;
}

/** Ask now. "Check now" and the quiet probe when About opens are the same call. */
export const checkForUpdate = () => invoke("update_check");

/** Download and install the version on offer; Pulse restarts when the installer is done. */
export const installUpdate = () => invoke("update_install");
