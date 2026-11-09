// Loaded last by scroll-step.html: scans the page as the app would, takes one scroll step and
// writes what step() reported into the page, where scripts/test-collector.mjs reads it.
(() => {
  window.__vdlCollector.scan(null, { includeAds: false });
  const reported = window.__vdlCollector.step();
  const out = document.createElement("pre");
  out.id = "vdl-result";
  out.textContent = JSON.stringify(reported);
  document.body.appendChild(out);
})();
