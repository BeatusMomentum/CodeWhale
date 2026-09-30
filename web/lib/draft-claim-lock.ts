/**
 * The exclusive hold on one community-agent draft while a maintainer action
 * (post or discard) runs.
 *
 * The `DraftClaimLock` Durable Object (exported from `worker.ts`, one instance
 * per draft identity via `idFromName`) runs `applyDraftLock` against its own
 * storage. A Durable Object handles one event at a time and its input gate
 * holds other events while a storage call is pending, so the read and write
 * below cannot interleave with another request's: exactly one claim wins.
 *
 * This module has no `cloudflare:workers` import so it runs under vitest and
 * the Next.js build; `worker.ts` holds the thin class around it.
 */

export type DraftLockAction = "post" | "discard";

export type DraftLockRequest =
  | { op: "claim"; token: string; action: DraftLockAction; leaseMs: number }
  /**
   * `holdMs` > 0 keeps refusing other claims for that long after an action
   * whose decision was recorded, covering the time the KV decision marker
   * takes to reach other locations. 0 frees the draft at once.
   */
  | { op: "release"; token: string; holdMs: number };

export type DraftLockResponse =
  | { ok: true }
  | { ok: false; holder: DraftLockAction };

/** The subset of `DurableObjectStorage` the lock uses. */
export interface DraftLockStorage {
  get<T>(key: string): Promise<T | undefined>;
  put<T>(key: string, value: T): Promise<void>;
  delete(key: string): Promise<boolean>;
}

interface Lease {
  token: string;
  action: DraftLockAction;
  /** Epoch ms. A lease past this no longer holds, so a crashed action cannot wedge the draft. */
  expiresAt: number;
}

const LEASE_KEY = "lease";

export async function applyDraftLock(
  storage: DraftLockStorage,
  now: number,
  req: DraftLockRequest
): Promise<DraftLockResponse> {
  const lease = await storage.get<Lease>(LEASE_KEY);
  const live = lease && lease.expiresAt > now ? lease : undefined;

  if (req.op === "claim") {
    if (live && live.token !== req.token) return { ok: false, holder: live.action };
    await storage.put<Lease>(LEASE_KEY, {
      token: req.token,
      action: req.action,
      expiresAt: now + Math.max(0, req.leaseMs),
    });
    return { ok: true };
  }

  // Release: only the holder's own token can release, so a request whose
  // lease expired and was retaken cannot free the new holder's claim.
  if (!live || live.token !== req.token) return { ok: true };
  if (req.holdMs > 0) {
    await storage.put<Lease>(LEASE_KEY, { ...live, expiresAt: now + req.holdMs });
  } else {
    await storage.delete(LEASE_KEY);
  }
  return { ok: true };
}

/** The RPC surface of a `DraftClaimLock` stub. */
export interface DraftClaimLockStub {
  act(req: DraftLockRequest): Promise<DraftLockResponse>;
}

/** The `DRAFT_CLAIM_LOCK` Durable Object namespace binding, as the app uses it. */
export interface DraftClaimLockNamespace {
  idFromName(name: string): DraftClaimLockId;
  get(id: DraftClaimLockId): DraftClaimLockStub;
}

/** Opaque `DurableObjectId`. */
export interface DraftClaimLockId {
  toString(): string;
}
