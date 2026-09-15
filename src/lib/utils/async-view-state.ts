/**
 * What a data-backed view should render once a load has been attempted.
 *
 * A view that answers "did I get any rows?" cannot tell an empty vault from a backend that
 * failed, so a broken index renders the same encouraging "nothing here yet" copy as a fresh
 * install. The user is told there is no work to see, and the real error is only in a console
 * that a packaged desktop build never shows. Loading, genuinely empty, and failed are three
 * different answers and each needs its own one.
 */

export type LoadStatus = "loading" | "loaded" | "failed";

export type AsyncViewState =
  | { kind: "loading" }
  | { kind: "failed"; message: string; retry: true }
  | { kind: "empty" }
  | { kind: "content" };

export interface AsyncViewInput {
  status: LoadStatus;
  itemCount: number;
  error?: unknown;
}

/**
 * Classify a view's render state.
 *
 * Failure outranks everything: a stale row still on screen must not present a failed
 * refresh as success. A load in flight that already has content keeps showing it rather
 * than flashing a spinner over a usable view.
 */
export function asyncViewState({ status, itemCount, error }: AsyncViewInput): AsyncViewState {
  if (status === "failed") {
    return { kind: "failed", message: describeLoadFailure(error), retry: true };
  }
  if (status === "loading") {
    return itemCount > 0 ? { kind: "content" } : { kind: "loading" };
  }
  return itemCount > 0 ? { kind: "content" } : { kind: "empty" };
}

const UNKNOWN_FAILURE = "The backend did not report why this failed.";

/**
 * Turn whatever a load rejected with into something worth showing a user.
 *
 * Tauri command rejections arrive as plain strings, DOM errors as `Error`, and plugin
 * failures as objects carrying `message`. Interpolating those directly yields `undefined`
 * or `[object Object]`, which is worse than saying nothing useful honestly.
 */
export function describeLoadFailure(error: unknown): string {
  if (typeof error === "string" && error.trim().length > 0) return error;
  if (error instanceof Error && error.message.trim().length > 0) return error.message;
  if (error !== null && typeof error === "object") {
    const message = (error as { message?: unknown }).message;
    if (typeof message === "string" && message.trim().length > 0) return message;
  }
  return UNKNOWN_FAILURE;
}
