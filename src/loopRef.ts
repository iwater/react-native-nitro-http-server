/**
 * LoopRef bridges the Node.js handle-ref model to host keep-alive capabilities.
 *
 * In Node.js, a listening server or connected socket keeps the event loop alive
 * via libuv handle refs. Other JS hosts may differ:
 *   - Headless JSI hosts: the event loop exits when idle and requires explicit
 *     keep-alive via `globalThis.__qjsHandleRef`.
 *   - React Native: app lifecycle is managed by the OS, so `globalThis.__qjsHandleRef`
 *     is absent and all methods on this class are no-ops.
 *
 * Use one LoopRef per handle; `acquire()` and `release()` are idempotent.
 * Used here so a running server keeps the loop alive until `stop()` succeeds.
 */

interface HostHandleRef {
  acquire(label: string): number;
  release(token: number): void;
}

// Probe once at module load; host capabilities do not change at runtime.
let host: HostHandleRef | undefined =
  (globalThis as { __qjsHandleRef?: HostHandleRef }).__qjsHandleRef;

/** Test-only: override or clear the detected host handle ref before creating LoopRef instances. */
export function __setHostHandleRefForTest(h: HostHandleRef | undefined): void {
  host = h;
}

export class LoopRef {
  private token: number | null = null;
  private readonly label: string;

  constructor(label: string) {
    this.label = label;
  }

  /** No-op if already acquired. */
  acquire(): void {
    if (host && this.token === null) {
      this.token = host.acquire(this.label);
    }
  }

  /** No-op if not currently held. */
  release(): void {
    if (host && this.token !== null) {
      const t = this.token;
      this.token = null;
      host.release(t);
    }
  }

  /** Whether the loop ref is currently held (for diagnostics and tests). */
  get refed(): boolean {
    return this.token !== null;
  }
}
