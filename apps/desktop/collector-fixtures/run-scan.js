// Loaded last by every fixture page: scans the page as the app would and writes the result
// into the page, where scripts/test-collector.mjs reads it from the dumped DOM.
(() => {
  const params = new URLSearchParams(location.search);
  const result = window.__vdlCollector.scan(params.get("shape"), { includeAds: params.get("includeAds") === "1" });
  const out = document.createElement("pre");
  out.id = "vdl-result";
  out.textContent = JSON.stringify(result);
  document.body.appendChild(out);
})();
