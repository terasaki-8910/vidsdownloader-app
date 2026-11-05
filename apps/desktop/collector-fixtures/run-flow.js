(() => {
  window.__vdlFlowAllowSynthetic = true;
  document.querySelectorAll(".card")[1].querySelector("strong").click();
  const [step] = window.__vdlFlowRecorder.drain();
  const out = document.createElement("pre");
  out.id = "vdl-result";
  out.textContent = JSON.stringify(step);
  document.body.appendChild(out);
})();
