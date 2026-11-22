"use strict";
// Loaded before the body so that the first paint already has the platform's
// panel shape and saved display theme: WebKitGTK does not clear pixels that
// later turn transparent.
if (navigator.userAgent.includes("Mac"))
  document.documentElement.classList.add("macos");
else if (navigator.userAgent.includes("Linux"))
  document.documentElement.classList.add("linux");
try {
  const theme = localStorage.getItem("copool.theme");
  if (theme === "light" || theme === "dark") {
    document.documentElement.dataset.theme = theme;
    const meta = document.querySelector('meta[name="color-scheme"]');
    if (meta) meta.content = theme;
  }
} catch {}
