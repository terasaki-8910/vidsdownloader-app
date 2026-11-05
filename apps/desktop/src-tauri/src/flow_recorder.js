(() => {
  if (window.__vdlFlowRecorder) return;

  const steps = [];
  const clickableSelector = "a, button, [role='button'], [onclick]";

  function stableClass(name) {
    if (!name || name.startsWith("vdl-")) return false;
    if (name.length >= 6 && /[a-z]/i.test(name) && /\d/.test(name)) return false;
    if (name.length >= 8 && /^[a-f\d]+$/i.test(name)) return false;
    return true;
  }

  function escapeName(name) {
    if (window.CSS?.escape) return CSS.escape(name);
    return name.replace(/[^a-zA-Z0-9_-]/g, (character) => `\\${character}`);
  }

  function segment(element) {
    const classes = [...element.classList].filter(stableClass).slice(0, 3);
    return `${element.tagName.toLowerCase()}${classes.map((name) => `.${escapeName(name)}`).join("")}`;
  }

  // cursor: pointer is inherited, so a pointer element counts only where the pointer starts
  // (its parent does not have it); a card's title text then resolves to the card.
  function startsPointer(element) {
    if (getComputedStyle(element).cursor !== "pointer") return false;
    const parent = element.parentElement;
    return !parent || getComputedStyle(parent).cursor !== "pointer";
  }

  function clickable(target) {
    for (let element = target instanceof Element ? target : target?.parentElement; element; element = element.parentElement) {
      if (element.matches(clickableSelector) || startsPointer(element)) return element;
    }
    return target instanceof Element ? target : null;
  }

  function patternFor(element) {
    const parts = [segment(element)];
    for (let ancestor = element.parentElement; ancestor && parts.length < 3; ancestor = ancestor.parentElement) {
      parts.unshift(segment(ancestor));
    }
    return parts.join(" ");
  }

  function matches(pattern) {
    try {
      return [...document.querySelectorAll(pattern)];
    } catch {
      return [];
    }
  }

  function rowFor(element, allMatches) {
    for (let ancestor = element.parentElement; ancestor; ancestor = ancestor.parentElement) {
      const rowMatches = allMatches.filter((candidate) => ancestor.contains(candidate));
      if (rowMatches.length >= 2) {
        return { rowIndex: rowMatches.indexOf(element), rowCount: rowMatches.length };
      }
    }
    return { rowIndex: 0, rowCount: 1 };
  }

  function labelFor(element) {
    const text = (element.innerText || element.textContent || "").replace(/\s+/g, " ").trim();
    const label = text || element.getAttribute("aria-label")?.replace(/\s+/g, " ").trim() || element.tagName.toLowerCase();
    return [...label].slice(0, 60).join("");
  }

  function describe(element) {
    const pattern = patternFor(element);
    const allMatches = matches(pattern);
    const pageIndex = allMatches.indexOf(element);
    const row = rowFor(element, allMatches);
    return {
      pattern,
      label: labelFor(element),
      url: location.href,
      matchCount: allMatches.length,
      rowIndex: row.rowIndex,
      rowCount: row.rowCount,
      pageIndex: pageIndex < 0 ? 0 : pageIndex,
    };
  }

  function record(event) {
    if (!event.isTrusted && !window.__vdlFlowAllowSynthetic) return;
    const element = clickable(event.target);
    if (element) steps.push(describe(element));
  }

  document.addEventListener("click", record, true);
  window.__vdlFlowRecorder = {
    drain() {
      return steps.splice(0, steps.length);
    },
    describe,
    matches,
  };
})();
