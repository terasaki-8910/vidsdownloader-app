// Loaded last by picker.html: starts the element picker and points at the video, a link card,
// an image outside links and plain text, the way a person would (pointer moves, then a press),
// and writes what the page labelled and what takePick() handed over into the page, where
// scripts/test-collector.mjs reads it. ?demo=video|link|image|none only moves the pointer over
// that target and leaves the picker on, for screenshots.
(() => {
  const collector = window.__vdlCollector;
  const params = new URLSearchParams(location.search);
  const pathOf = (url) => (url === null || url === undefined ? null : new URL(url).pathname);

  function centerOf(element) {
    const rect = element.getBoundingClientRect();
    return [rect.left + rect.width / 2, rect.top + rect.height / 2];
  }

  function moveTo(element) {
    const [x, y] = centerOf(element);
    (document.elementFromPoint(x, y) || document).dispatchEvent(
      new MouseEvent("mousemove", { bubbles: true, cancelable: true, view: window, clientX: x, clientY: y }),
    );
    return [x, y];
  }

  function press(element) {
    const [x, y] = moveTo(element);
    const target = document.elementFromPoint(x, y) || document;
    const sawClick = [];
    const spy = () => sawClick.push(true);
    document.addEventListener("click", spy);
    const notPrevented = target.dispatchEvent(
      new MouseEvent("click", { bubbles: true, cancelable: true, view: window, clientX: x, clientY: y }),
    );
    document.removeEventListener("click", spy);
    return { prevented: !notPrevented, pageSawClick: sawClick.length > 0 };
  }

  const labelText = () => document.getElementById("vdl-collector-pick-label")?.textContent ?? null;
  const picking = () => document.getElementById("vdl-collector-pick-box") !== null;
  const summary = (pick) =>
    pick && {
      kind: pick.kind,
      url: pathOf(pick.url),
      shape: pick.shape ?? null,
      count: pick.count ?? null,
      videoIndex: pick.videoIndex ?? null,
    };

  const targets = {
    video: () => document.querySelector("video"),
    link: () => document.querySelector('a.thumb[href="/watch/3"]'),
    image: () => document.querySelector(".large img"),
    none: () => document.querySelector(".post h2"),
  };

  const demo = params.get("demo");
  const result = {};
  if (demo) {
    collector.startPick(params.get("lang") || "ja");
    moveTo(targets[demo]());
  } else {
    // Each kind of target: its label while the pointer is over it, then what a press hands over.
    for (const kind of ["video", "link", "image"]) {
      collector.startPick("en");
      moveTo(targets[kind]());
      const label = labelText();
      const pressed = press(targets[kind]());
      result[kind] = { label, ...pressed, pick: summary(collector.takePick()), stillPicking: picking() };
      result[`${kind}Again`] = collector.takePick();
    }

    // Plain text is nothing: the press is recorded as "none" and picking goes on.
    collector.startPick("en");
    moveTo(targets.none());
    const label = labelText();
    const pressed = press(targets.none());
    result.none = { label, ...pressed, pick: summary(collector.takePick()), stillPicking: picking() };

    // Escape ends it with "cancel"; stopPick() ends it with no answer at all.
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
    result.escape = { pick: summary(collector.takePick()), stillPicking: picking() };
    collector.startPick("ja");
    moveTo(targets.link());
    result.japaneseLabel = labelText();
    collector.stopPick();
    result.stopped = { pick: collector.takePick(), stillPicking: picking() };

    // A badge sits over its card; pointing at it means pointing at what is under it.
    collector.scan(null);
    collector.startPick("en");
    const badge = document.querySelector(".vdl-collector-badge");
    moveTo(badge);
    result.overBadge = { label: labelText() };
    collector.stopPick();
  }

  const out = document.createElement("pre");
  out.id = "vdl-result";
  out.hidden = Boolean(params.get("demo"));
  out.textContent = JSON.stringify(result);
  document.body.appendChild(out);
})();
