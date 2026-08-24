/**
 * Brief, self-dismissing message for outcomes the user needs to notice but does not need
 * to act on in a dialog.
 *
 * Failures that were previously logged to the console only are invisible in a packaged
 * desktop build, where there is no console to look at.
 */
export function showToast(message: string, durationMs = 4000): void {
  if (typeof document === "undefined") return;

  const toast = document.createElement("div");
  toast.style.cssText =
    "position:fixed;bottom:24px;left:50%;transform:translateX(-50%);" +
    "background:var(--bg-secondary);color:var(--text-primary);padding:10px 18px;" +
    "border-radius:8px;box-shadow:0 4px 12px rgba(0,0,0,.15);" +
    "border:1px solid var(--border-color);font-size:13px;z-index:10000;" +
    "opacity:0;transition:opacity .3s;pointer-events:none;max-width:min(90vw,420px);" +
    "text-align:center";
  toast.textContent = message;
  // Announce to screen readers: the message is otherwise purely visual.
  toast.setAttribute("role", "status");
  toast.setAttribute("aria-live", "polite");

  document.body.appendChild(toast);
  requestAnimationFrame(() => (toast.style.opacity = "1"));
  setTimeout(() => {
    toast.style.opacity = "0";
    setTimeout(() => toast.remove(), 300);
  }, durationMs);
}
