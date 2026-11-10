// Loaded last by select-badges.html: drives the collector's selection badges the way the app does
// (scan, setSelection, takeToggles) and writes what the page did into the page, where
// scripts/test-collector.mjs reads it. ?demo=mixed leaves cards 2, 5 and 9 unselected and
// writes nothing else, for screenshots.
(() => {
  const collector = window.__vdlCollector;
  const absolute = (path) => new URL(path, location.href).href;
  const pathOf = (url) => new URL(url).pathname;
  const params = new URLSearchParams(location.search);

  // What every badge shows: its item, the state it announces and whether its card is outlined as off.
  function badges() {
    return [...document.querySelectorAll(".vdl-collector-badge")].map((badge) => ({
      url: pathOf(badge.dataset.vdlUrl),
      checked: badge.getAttribute("aria-checked"),
      off: badge.parentElement.classList.contains("vdl-collector-off"),
    }));
  }
  const offUrls = () => badges().filter((badge) => badge.checked !== "true").map((badge) => badge.url);
  const togglesOf = (list) => list.map((toggle) => ({ url: pathOf(toggle.url), selected: toggle.selected }));

  // A click on a badge as a pointing device sends it; reports whether the page could still act on it.
  function click(path) {
    const badge = document.querySelector(`.vdl-collector-badge[data-vdl-url="${absolute(path)}"]`);
    const before = location.href;
    pageSawClick = false;
    const notPrevented = badge.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true, view: window }));
    return { prevented: !notPrevented, pageSawClick, stayed: location.href === before };
  }

  let pageSawClick = false;
  document.addEventListener("click", () => {
    pageSawClick = true;
  });

  const result = {};
  const first = collector.scan(null);
  result.items = first.items.map((item) => pathOf(item.url));
  result.initial = { count: badges().length, off: offUrls() };

  if (params.get("demo") === "mixed") {
    collector.setSelection([2, 5, 9].map((n) => absolute(`/watch/${n}`)));
  } else {
    // App -> page.
    collector.setSelection([absolute("/watch/2"), absolute("/watch/5")]);
    result.afterSet = { off: offUrls(), outlinedOff: badges().filter((badge) => badge.off).map((badge) => badge.url) };

    // Page -> app: a click flips one item, is taken once, and leaves the page alone.
    result.click1 = click("/watch/4");
    result.afterClick1 = { off: offUrls() };
    result.toggles1 = togglesOf(collector.takeToggles());
    result.toggles1Again = togglesOf(collector.takeToggles());
    result.click2 = click("/watch/4");
    result.toggles2 = togglesOf(collector.takeToggles());

    // A click the app has not taken yet survives the app's next selection.
    click("/watch/6");
    collector.setSelection([absolute("/watch/2")]);
    result.pendingKept = { off: offUrls() };
    result.pendingToggles = togglesOf(collector.takeToggles());

    // A scan again keeps the badge elements and what they show.
    const keptBadge = document.querySelector(".vdl-collector-badge");
    collector.scan(null);
    result.afterRescan = {
      sameBadge: document.querySelector(".vdl-collector-badge") === keptBadge,
      count: badges().length,
      off: offUrls(),
    };

    // Items the tray's filter hides lose their outline and badge (and the position the badge
    // needed); a scan does not mark them again; shown again they get both back, unchecked or not.
    const marks = () => ({
      badges: badges().map((badge) => badge.url),
      marked: document.querySelectorAll(".vdl-collector-found").length,
      off: offUrls(),
      hiddenCards: document.querySelectorAll(".vdl-collector-hidden").length,
      positioned: document.querySelectorAll("[data-vdl-static]").length,
    });
    collector.setSelection(
      [absolute("/watch/2"), absolute("/watch/3")],
      [absolute("/watch/3"), absolute("/watch/7")],
    );
    result.hidden = marks();
    collector.scan(null);
    result.hiddenRescan = marks();
    collector.setSelection([absolute("/watch/2"), absolute("/watch/3")], [absolute("/watch/7")]);
    result.hiddenPartly = marks();
    collector.setSelection([absolute("/watch/2")], []);
    result.shownAgain = marks();

    // Closing the session takes everything off the page and forgets the choices.
    collector.clear();
    result.afterClear = {
      badges: document.querySelectorAll(".vdl-collector-badge").length,
      marked: document.querySelectorAll(".vdl-collector-found").length,
    };
    collector.scan(null);
    result.afterClearScan = { off: offUrls() };

    // The played streams have no cards: a scan asked not to mark still finds the items but takes
    // every mark off the page.
    const unmarked = collector.scan(null, { mark: false });
    result.unmarkedScan = {
      items: unmarked.items.length,
      badges: document.querySelectorAll(".vdl-collector-badge").length,
      marked: document.querySelectorAll(".vdl-collector-found").length,
    };
  }

  const out = document.createElement("pre");
  out.id = "vdl-result";
  out.hidden = Boolean(params.get("demo"));
  out.textContent = JSON.stringify(result);
  document.body.appendChild(out);
})();
