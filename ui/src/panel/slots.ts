// Ported from upstream Usage/MonitoredAccount.swift (`RailSlot.rail`, `RailSlot.modelGroups`): the rail's rings.
// One per account, except an account that is split by model group (Account settings > "A ring for each model
// group"), which gets one ring per group once a reading carries them.
import { accountId, type ProviderUsage } from "../shared/model";

/** Providers whose limits can be drawn as one ring per model group, and how many (`modelGroupCount`). Antigravity alone. */
const modelGroupCount: Record<string, number> = { antigravity: 2 };

export interface RailSlot {
  /** The account's own id for an unsplit slot (so a hover or selection keeps matching); `<account>@<group>` for a group. */
  id: string;
  /** The account's id: what refresh, tint, pinned window and detailed card are keyed by. */
  account: string;
  /** The reading this ring draws: the account's, or only the windows of this ring's group. */
  usage: ProviderUsage;
}

/**
 * The model groups a reading carries, in the order the provider reported them. All or nothing: a reading that
 * mixes scoped and unscoped windows gets no groups, so the account stays whole rather than losing a limit.
 */
export function modelGroups(usage: ProviderUsage): string[] {
  if (!usage.windows.length || !usage.windows.every((w) => w.scope != null)) return [];
  return [...new Set(usage.windows.map((w) => w.scope as string))];
}

export function railSlots(usages: ProviderUsage[], splitAccounts: string[]): RailSlot[] {
  return usages.flatMap((usage): RailSlot[] => {
    const account = accountId(usage.account);
    const whole = [{ id: account, account, usage }];
    if (!splitAccounts.includes(account)) return whole;
    const groups = modelGroups(usage);
    // More groups than the rail budgeted for: stay whole rather than lose a limit.
    if (groups.length < 2 || groups.length > (modelGroupCount[usage.account.provider] ?? 1)) return whole;
    return groups.map((group) => ({
      id: `${account}@${group}`,
      account,
      usage: { ...usage, windows: usage.windows.filter((w) => w.scope === group) },
    }));
  });
}
