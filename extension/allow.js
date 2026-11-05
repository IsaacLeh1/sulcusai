// SPDX-License-Identifier: AGPL-3.0-only
// permissions.request needs a click on a page, which the background can't give.
document.getElementById("allow").addEventListener("click", async () => {
  const ok = await chrome.permissions.request({ permissions: ["debugger"] }).catch(() => false);
  document.getElementById("msg").textContent = ok ? "Allowed. You can close this tab." : "Not allowed.";
  if (ok) setTimeout(() => window.close(), 1200);
});
