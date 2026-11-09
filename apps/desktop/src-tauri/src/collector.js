(() => {
  const MARK_CLASS = "vdl-collector-found";
  const BADGE_CLASS = "vdl-collector-badge";
  const OFF_CLASS = "vdl-collector-off";
  const REVEAL_CLASS = "vdl-collector-reveal";
  const STYLE_ID = "vdl-collector-style";
  const PICK_STYLE_ID = "vdl-collector-pick-style";
  const PICK_BOX_ID = "vdl-collector-pick-box";
  const PICK_LABEL_ID = "vdl-collector-pick-label";
  // The script is evaluated again for every call, so what must outlive one call lives on window:
  // the card last seen for each item URL, for reveal(), and the number given to each page
  // image, so a later scan that finds more images keeps the earlier numbers. The page never gets
  // IPC, so what it wants to tell the app waits here until the app asks: toggles (badges clicked
  // in the page) and the element picked in pick mode.
  const memory =
    window.__vdlCollectorMemory ||
    (window.__vdlCollectorMemory = { cards: new Map(), imagesPage: null, imageIndex: new Map() });
  if (!memory.imageIndex) memory.imageIndex = new Map();
  // The item URLs the user has switched off; every other found item is selected.
  if (!memory.unselected) memory.unselected = new Set();
  // Badge clicks not yet taken by the app, in order: { url, selected }.
  if (!memory.toggles) memory.toggles = [];
  // The item URL each marked card stands for (an item can have several cards).
  if (!memory.cardUrl) memory.cardUrl = new WeakMap();
  // The last element-picker result, until takePick() hands it over; pickStop ends pick mode.
  if (memory.pickResult === undefined) memory.pickResult = null;
  if (memory.pickStop === undefined) memory.pickStop = null;
  // At most this many badge clicks wait for the app (it takes them every half second).
  const MAX_PENDING_TOGGLES = 2000;
  // The reserved shape of the group holding the page's own images (not inside links).
  const IMAGES_SHAPE = "{images}";
  // At most this many URLs of the best link group are listed under the images.
  const MAX_OTHER_LINKS = 200;
  // A link to an image file opens that image (a lightbox), not another page.
  const IMAGE_FILE = /\.(?:jpe?g|png|gif|webp|avif)$/i;
  const MIN_GROUP_URLS = 3;
  const MAX_GROUP_URLS = 2000;
  // A query value that reads as a word stays literal while its parameter has at most this
  // many such values on the page (tabs such as f=live, f=media); more make it {id}.
  const MAX_WORD_VALUES = 5;
  // A path position holding at least this many different words, among links of the same
  // length and query names, is a variable part ({any}): the user name in X post links, or
  // photo / video in X media tiles (a grid mixing both is one list).
  const MIN_VARIANTS = 2;
  // Images smaller than this are icons or avatars, never media or thumbnails.
  const MIN_MEDIA_SIDE = 64;
  const MIN_MEDIA_AREA = 10000;
  const AD_LABELS = new Set(["広告", "ad", "ads", "promoted", "sponsored", "スポンサー", "プロモーション"]);
  // Links in these are navigation (menus, tabs, the site's header and footer), never items.
  const NAV_SELECTOR =
    "nav, [role=navigation], [role=tablist], [role=tab], [role=menubar], [role=menu], [role=banner], [role=contentinfo]";

  function normalizedHost(host) {
    return host.toLowerCase().replace(/^www\./, "");
  }

  function isSameSite(url) {
    const pageHost = location.hostname.toLowerCase();
    const baseHost = normalizedHost(pageHost);
    const candidate = url.hostname.toLowerCase();
    return candidate === pageHost || candidate === baseHost || candidate.endsWith(`.${baseHost}`);
  }

  function isPlaceholder(token) {
    return token === "{num}" || token === "{id}" || token === "{any}" || token === "{slug}";
  }

  function compareNames(a, b) {
    return a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0;
  }

  function segmentShape(segment) {
    if (/^\d+$/.test(segment)) return "{num}";
    // An id mixes letters and digits, or mixes upper and lower case (YouTube-like ids).
    // Plain words with underscores or hyphens ("header_photo") stay literal.
    if (segment.length >= 8 && /^[A-Za-z0-9_-]+={0,2}$/.test(segment)) {
      const mixesLettersAndDigits = /[A-Za-z]/.test(segment) && /\d/.test(segment);
      const mixesCase = segment.length >= 10 && /[a-z]/.test(segment) && /[A-Z]/.test(segment);
      if (mixesLettersAndDigits || mixesCase) return "{id}";
    }
    return segment;
  }

  function isWordLike(value) {
    return /^[A-Za-z][A-Za-z_-]{0,23}$/.test(value) && segmentShape(value) === value;
  }

  function queryToken(name, value, wordValues) {
    if (value === "") return "";
    if (/^\d+$/.test(value)) return "{num}";
    if (isWordLike(value) && wordValues.get(name).size <= MAX_WORD_VALUES) return encodeURIComponent(value);
    return "{id}";
  }

  function shapeString(path, query) {
    const head = `/${path.join("/")}`;
    return query.length ? `${head}?${query.map(([name, token]) => `${name}=${token}`).join("&")}` : head;
  }

  // A shape is also a pattern: a saved rule or the user's choice is matched against every link,
  // so it keeps working when the page's own statistics change.
  function parseShape(shape) {
    const mark = shape.indexOf("?");
    const path = (mark < 0 ? shape : shape.slice(0, mark)).split("/").slice(1);
    const rawQuery = mark < 0 ? "" : shape.slice(mark + 1);
    const query = rawQuery
      ? rawQuery
          .split("&")
          .map((pair) => {
            const eq = pair.indexOf("=");
            return eq < 0 ? [pair, ""] : [pair.slice(0, eq), pair.slice(eq + 1)];
          })
          .sort(compareNames)
      : [];
    return { path, query };
  }

  function tokenMatches(pattern, value) {
    if (pattern === "{num}") return /^\d+$/.test(value);
    if (isPlaceholder(pattern)) return value.length > 0;
    return pattern === value;
  }

  function matchesShape(pattern, link) {
    if (pattern.path.length !== link.segments.length || pattern.query.length !== link.query.length) return false;
    for (let i = 0; i < pattern.path.length; i += 1) {
      if (!tokenMatches(pattern.path[i], link.segments[i])) return false;
    }
    for (let i = 0; i < pattern.query.length; i += 1) {
      const [name, token] = pattern.query[i];
      const [linkName, value] = link.query[i];
      if (name !== linkName || !tokenMatches(token, encodeURIComponent(value))) return false;
    }
    return true;
  }

  function isNavigation(element) {
    if (element.closest(NAV_SELECTOR)) return true;
    // A header or footer inside an article belongs to that article (its title, its tags).
    const edge = element.closest("header, footer");
    return Boolean(edge && !edge.closest("article"));
  }

  // Same-site, visible links outside navigation, with their path segments and sorted query.
  function candidateLinks() {
    const links = [];
    for (const anchor of document.querySelectorAll("a[href]")) {
      let url;
      try {
        url = new URL(anchor.href, location.href);
      } catch {
        continue;
      }
      if (!/^https?:$/.test(url.protocol) || !isSameSite(url)) continue;
      url.hash = "";
      if (url.href.length > 2048) continue;
      if (anchor.getClientRects().length === 0 || isNavigation(anchor)) continue;
      links.push({
        anchor,
        url: url.href,
        segments: url.pathname.split("/").slice(1),
        query: [...url.searchParams].map(([name, value]) => [encodeURIComponent(name), value]).sort(compareNames),
      });
    }
    return links;
  }

  function assignShapes(links) {
    const wordValues = new Map();
    for (const link of links) {
      for (const [name, value] of link.query) {
        if (!wordValues.has(name)) wordValues.set(name, new Set());
        if (isWordLike(value)) wordValues.get(name).add(value);
      }
    }
    const buckets = new Map();
    for (const link of links) {
      link.path = link.segments.map(segmentShape);
      link.queryTokens = link.query.map(([name, value]) => [name, queryToken(name, value, wordValues)]);
      const key = `${link.path.length}?${link.query.map(([name]) => name).join("&")}`;
      if (!buckets.has(key)) buckets.set(key, []);
      buckets.get(key).push(link);
    }
    for (const bucket of buckets.values()) {
      const variants = [];
      for (let p = 0; p < bucket[0].path.length; p += 1) {
        const words = new Set();
        for (const link of bucket) if (!isPlaceholder(link.path[p])) words.add(link.path[p]);
        if (words.size >= MIN_VARIANTS) variants.push({ p, count: words.size });
      }
      // The most varied position first: that is the one naming the item (a user, a slug).
      variants.sort((a, b) => b.count - a.count || a.p - b.p);
      for (const link of bucket) {
        for (const { p } of variants) {
          // An id-like word at a variable position (a user name with digits) joins the rest.
          if (link.path[p] === "{num}" || link.path[p] === "{any}") continue;
          const next = link.path.slice();
          next[p] = "{any}";
          // A shape of placeholders only would join unrelated links.
          if (next.some((token) => token !== "" && !isPlaceholder(token))) link.path = next;
        }
      }
    }
    for (const link of links) link.shape = shapeString(link.path, link.queryTokens);
  }

  // Each item's card: the outermost ancestor that still holds no other URL of the group
  // (a post, a grid tile). Works the same on any site; no site knowledge needed.
  function cardsFor(links) {
    const holds = new Map();
    for (const { anchor, url } of links) {
      for (let el = anchor.parentElement; el && el !== document.body && el !== document.documentElement; el = el.parentElement) {
        const seen = holds.get(el);
        // Every ancestor of an element holding two URLs holds two as well.
        if (seen === null) break;
        holds.set(el, seen === undefined || seen === url ? url : null);
      }
    }
    return links.map(({ anchor, url }) => {
      let card = anchor;
      for (let el = anchor.parentElement; el && holds.get(el) === url; el = el.parentElement) card = el;
      return card;
    });
  }

  function area(element) {
    const rect = element.getBoundingClientRect();
    return rect.width * rect.height;
  }

  function isMediaSized(element) {
    const rect = element.getBoundingClientRect();
    return Math.min(rect.width, rect.height) >= MIN_MEDIA_SIDE && rect.width * rect.height >= MIN_MEDIA_AREA;
  }

  function httpUrl(value) {
    if (!value) return null;
    try {
      const url = new URL(value, location.href);
      return /^https?:$/.test(url.protocol) ? url.href : null;
    } catch {
      return null;
    }
  }

  function hasMedia(card) {
    // A page image is its own card.
    if (card.tagName === "IMG") return isMediaBox(imageBox(card));
    if (card.querySelector("video")) return true;
    for (const img of card.querySelectorAll("img")) if (isMediaSized(img)) return true;
    return false;
  }

  function largestImage(root) {
    let best = null;
    let bestArea = 0;
    for (const img of root.querySelectorAll("img")) {
      const src = httpUrl(img.currentSrc || img.src);
      if (!src || !isMediaSized(img)) continue;
      const size = area(img);
      if (size > bestArea) {
        best = src;
        bestArea = size;
      }
    }
    return best;
  }

  // The largest image big enough to be media (icons and avatars are skipped), looked for in the
  // item's own links first, then in its card; else a video poster.
  function cardThumbnail(anchors, card) {
    for (const anchor of anchors) {
      const own = largestImage(anchor);
      if (own) return own;
    }
    const best = largestImage(card);
    if (best) return best;
    for (const video of card.querySelectorAll("video[poster]")) {
      const poster = httpUrl(video.getAttribute("poster"));
      if (poster) return poster;
    }
    return null;
  }

  function cardTitle(anchors, card) {
    for (const anchor of anchors) {
      const aria = anchor.getAttribute("aria-label")?.trim();
      if (aria && aria.length >= 12) return aria;
      const own = anchor.textContent?.replace(/\s+/g, " ").trim() || "";
      if (own.length >= 12) return own;
    }
    return (card.innerText || card.textContent || "").replace(/\s+/g, " ").trim().slice(0, 160);
  }

  // An ad card carries its own short label ("広告", "Ad", "Promoted", ...) as a text of its own.
  function isAd(card) {
    const walker = document.createTreeWalker(card, NodeFilter.SHOW_TEXT);
    for (let node = walker.nextNode(); node; node = walker.nextNode()) {
      const text = node.nodeValue;
      if (text.length <= 40 && AD_LABELS.has(text.replace(/\s+/g, " ").trim().toLowerCase())) return true;
    }
    return false;
  }

  // --- The page's own images -------------------------------------------------------------

  function pageKey() {
    const url = new URL(location.href);
    url.hash = "";
    return url.href;
  }

  // An absolute http(s) URL of an image the app can save; never data:, blob: or SVG.
  function imageFileUrl(value) {
    if (!value) return null;
    try {
      const url = new URL(value, location.href);
      if (!/^https?:$/.test(url.protocol) || /\.svg$/i.test(url.pathname)) return null;
      url.hash = "";
      return url.href.length > 2048 ? null : url.href;
    } catch {
      return null;
    }
  }

  // The candidate with the largest width descriptor; density-only candidates rank below any
  // width, by density (a candidate without a descriptor is 1x).
  function largestCandidate(srcsets) {
    let best = null;
    let bestScore = -1;
    for (const srcset of srcsets) {
      const text = srcset || "";
      let i = 0;
      while (i < text.length) {
        while (i < text.length && /[\s,]/.test(text[i])) i += 1;
        if (i >= text.length) break;
        let end = i;
        while (end < text.length && !/\s/.test(text[end])) end += 1;
        let url = text.slice(i, end);
        let descriptor = "";
        if (/,$/.test(url)) {
          url = url.replace(/,+$/, "");
          i = end;
        } else {
          let stop = end;
          while (stop < text.length && text[stop] !== ",") stop += 1;
          descriptor = text.slice(end, stop).trim();
          i = stop + 1;
        }
        const width = /(?:^|\s)(\d+)w(?:\s|$)/.exec(descriptor);
        const density = /(?:^|\s)(\d+(?:\.\d+)?)x(?:\s|$)/.exec(descriptor);
        const score = width ? 1000 + Number(width[1]) : density ? Number(density[1]) : 1;
        if (url && score > bestScore) {
          best = url;
          bestScore = score;
        }
      }
    }
    return best;
  }

  // The largest srcset candidate of the img or its <picture> sources (the image is saved, so
  // the best version beats the one sized for this window), else what the page shows
  // (currentSrc), else src.
  function pageImageUrl(img) {
    const srcsets = [img.getAttribute("srcset")];
    if (img.parentElement?.tagName === "PICTURE") {
      for (const source of img.parentElement.querySelectorAll(":scope > source[srcset]")) {
        srcsets.push(source.getAttribute("srcset"));
      }
    }
    return (
      imageFileUrl(largestCandidate(srcsets)) ||
      imageFileUrl(img.currentSrc) ||
      imageFileUrl(img.getAttribute("src"))
    );
  }

  // The image's size: natural once loaded, else its box on the page, else its width/height
  // attributes (a lazy image not laid out yet).
  function imageBox(img) {
    if (img.complete && img.naturalWidth > 0 && img.naturalHeight > 0) return [img.naturalWidth, img.naturalHeight];
    const rect = img.getBoundingClientRect();
    if (rect.width > 0 && rect.height > 0) return [rect.width, rect.height];
    const width = Number(img.getAttribute("width"));
    const height = Number(img.getAttribute("height"));
    return width > 0 && height > 0 ? [width, height] : [0, 0];
  }

  function isMediaBox([width, height]) {
    return Math.min(width, height) >= MIN_MEDIA_SIDE && width * height >= MIN_MEDIA_AREA;
  }

  // Inside a link to another page (a thumbnail of an item, an ad, a profile): not a page image.
  // A link to the image itself, to an image file or to this same page opens nothing else.
  function leadsElsewhere(img, imageUrl) {
    const anchor = img.closest("a[href]");
    if (!anchor) return false;
    let target;
    try {
      target = new URL(anchor.href, location.href);
    } catch {
      return false;
    }
    if (!/^https?:$/.test(target.protocol)) return false;
    target.hash = "";
    return target.href !== imageUrl && !IMAGE_FILE.test(target.pathname) && target.href !== pageKey();
  }

  // Media-sized images outside navigation, ads and links to other pages, one per URL, in page
  // order. Each keeps the number it got when first found on this page.
  function pageImages(includeAds) {
    const key = pageKey();
    if (memory.imagesPage !== key) {
      memory.imagesPage = key;
      memory.imageIndex = new Map();
    }
    const found = [];
    const ads = [];
    const seen = new Set();
    for (const img of document.querySelectorAll("img")) {
      if (found.length >= MAX_GROUP_URLS) break;
      if (img.getClientRects().length === 0) continue;
      const url = pageImageUrl(img);
      if (!url || seen.has(url)) continue;
      if (!isMediaBox(imageBox(img)) || isNavigation(img) || leadsElsewhere(img, url)) continue;
      seen.add(url);
      const card = img.closest("figure") || img;
      if (!includeAds && isAd(card)) {
        ads.push(url);
        continue;
      }
      if (!memory.imageIndex.has(url)) memory.imageIndex.set(url, memory.imageIndex.size + 1);
      found.push({ img, card, url, index: memory.imageIndex.get(url) });
    }
    return { found, ads };
  }

  function imageCard(url) {
    const img = [...document.querySelectorAll("img")].find((candidate) => pageImageUrl(candidate) === url);
    return img ? img.closest(`.${MARK_CLASS}`) || img.closest("figure") || img : null;
  }

  // --- Page facts, for naming saved page images ---------------------------------------------

  function looksLikeUrl(text) {
    return /^(?:[a-z][a-z0-9+.-]*:)?\/\//i.test(text);
  }

  function metaContent(selector) {
    return document.querySelector(selector)?.getAttribute("content")?.trim() || null;
  }

  // Every JSON-LD object in page order, arrays and @graph unfolded; malformed blocks are skipped.
  function jsonLdNodes() {
    const nodes = [];
    const visit = (value, depth) => {
      if (depth > 6 || nodes.length >= 200 || !value || typeof value !== "object") return;
      if (Array.isArray(value)) {
        for (const entry of value) visit(entry, depth + 1);
        return;
      }
      nodes.push(value);
      if (value["@graph"]) visit(value["@graph"], depth + 1);
    };
    for (const script of document.querySelectorAll('script[type="application/ld+json" i]')) {
      try {
        visit(JSON.parse(script.textContent || ""), 0);
      } catch {
        // Not JSON: ignored, like a browser would.
      }
    }
    return nodes;
  }

  // The first author's name; an author given only as {"@id"} is looked up among the nodes.
  function authorName(author, byId) {
    let first = Array.isArray(author) ? author[0] : author;
    if (first && typeof first === "object" && first.name === undefined && typeof first["@id"] === "string") {
      first = byId.get(first["@id"]);
    }
    const name = typeof first === "string" ? first : first && typeof first === "object" ? first.name : null;
    if (typeof name !== "string") return null;
    const trimmed = name.replace(/\s+/g, " ").trim();
    return trimmed && !looksLikeUrl(trimmed) ? trimmed : null;
  }

  function unixSeconds(value) {
    if (typeof value !== "string" || !value.trim()) return null;
    const milliseconds = Date.parse(value.trim());
    return Number.isFinite(milliseconds) ? Math.floor(milliseconds / 1000) : null;
  }

  function pageFacts() {
    const nodes = jsonLdNodes();
    const byId = new Map();
    for (const node of nodes) if (typeof node["@id"] === "string" && !byId.has(node["@id"])) byId.set(node["@id"], node);
    const title = metaContent('meta[property="og:title"]') || (document.title || "").replace(/\s+/g, " ").trim();
    let uploader = null;
    for (const node of nodes) {
      uploader = authorName(node.author, byId);
      if (uploader) break;
    }
    if (!uploader) uploader = metaContent('meta[name="author" i]');
    if (!uploader) {
      const articleAuthor = metaContent('meta[property="article:author"]');
      uploader = articleAuthor && !looksLikeUrl(articleAuthor) ? articleAuthor : null;
    }
    let publishedAt = null;
    for (const node of nodes) {
      publishedAt = unixSeconds(node.datePublished);
      if (publishedAt !== null) break;
    }
    if (publishedAt === null) publishedAt = unixSeconds(metaContent('meta[property="article:published_time"]'));
    if (publishedAt === null) publishedAt = unixSeconds(document.querySelector("article time[datetime]")?.getAttribute("datetime"));
    const segments = location.pathname.split("/").filter(Boolean);
    let id = segments.length ? segments[segments.length - 1] : "";
    try {
      id = decodeURIComponent(id);
    } catch {
      // Keep the raw segment.
    }
    const dot = id.lastIndexOf(".");
    if (dot > 0) id = id.slice(0, dot);
    return { title, uploader, publishedAt, id: [...id].length >= 3 ? id : null };
  }

  function median(values) {
    if (!values.length) return 0;
    const sorted = values.slice().sort((a, b) => a - b);
    return sorted[Math.floor(sorted.length / 2)];
  }

  // Groups whose cards mostly hold media come first; then many large cards beat many small
  // links; a tie goes to the shorter path (a post's own link rather than one of its sub-pages).
  function rankGroups(groups) {
    for (const group of groups) {
      // Page images bring their cards (each image is its own); link groups find theirs.
      const cards = group.cards || [...new Set(cardsFor(group.links))];
      group.media = cards.filter(hasMedia).length * 2 >= cards.length;
      group.score = group.urls.size * Math.sqrt(median(cards.map(area)));
    }
    return groups.sort(
      (a, b) =>
        Number(b.media) - Number(a.media) ||
        b.score - a.score ||
        b.urls.size - a.urls.size ||
        a.path.length - b.path.length ||
        (a.shape < b.shape ? -1 : a.shape > b.shape ? 1 : 0),
    );
  }

  function ensureStyle() {
    if (document.getElementById(STYLE_ID)) return;
    const style = document.createElement("style");
    style.id = STYLE_ID;
    style.textContent = `
      .${MARK_CLASS} {
        outline: 2.5px solid #1E9E56 !important;
        outline-offset: 1px !important;
      }
      .${MARK_CLASS}.${OFF_CLASS} {
        outline: 2px dashed #8C8C8C !important;
        outline-offset: 1px !important;
      }
      .${MARK_CLASS}.${REVEAL_CLASS} {
        outline-width: 5px !important;
        outline-offset: 3px !important;
      }
      .${BADGE_CLASS} {
        position: absolute !important;
        z-index: 2147483647 !important;
        top: 6px !important;
        left: 6px !important;
        width: 24px !important;
        height: 24px !important;
        box-sizing: border-box !important;
        margin: 0 !important;
        padding: 0 !important;
        border: 0 none !important;
        border-radius: 50% !important;
        display: grid !important;
        place-items: center !important;
        background: #1E9E56 !important;
        color: #FFFFFF !important;
        font: 700 15px/1 sans-serif !important;
        pointer-events: auto !important;
        cursor: pointer !important;
        user-select: none !important;
      }
      .${BADGE_CLASS}::before {
        content: "" !important;
      }
      .${BADGE_CLASS}[aria-checked="true"]::before {
        content: "✓" !important;
      }
      .${BADGE_CLASS}[aria-checked="false"] {
        background: rgba(255, 255, 255, 0.9) !important;
        border: 2px solid #6A6A6A !important;
      }
      .${BADGE_CLASS}:hover {
        box-shadow: 0 0 0 3px rgba(30, 158, 86, 0.35) !important;
      }
    `;
    (document.head || document.documentElement).appendChild(style);
  }

  function unmark(element) {
    element.classList.remove(MARK_CLASS, OFF_CLASS);
    for (const badge of element.querySelectorAll(`:scope > .${BADGE_CLASS}`)) badge.remove();
    // Undo only what mark() changed: a static element made relative for the badge.
    if (element.dataset.vdlStatic === "1") {
      element.style.position = "";
      delete element.dataset.vdlStatic;
    }
  }

  // Draw a marked card as selected (green outline, check badge) or not (dashed grey outline,
  // empty badge), by whether its item URL is in memory.unselected.
  function paint(element) {
    const url = memory.cardUrl.get(element);
    if (url === undefined) return;
    const selected = !memory.unselected.has(url);
    element.classList.toggle(OFF_CLASS, !selected);
    const badge = element.querySelector(`:scope > .${BADGE_CLASS}`);
    if (!badge) return;
    // The check is drawn by CSS, so the badge holds no text a title or an ad check could read.
    badge.setAttribute("aria-checked", selected ? "true" : "false");
  }

  // The badge's click flips the item; the page's own handlers (the card's link included) must
  // never see it, so the events are stopped on the badge and the click's default action is
  // cancelled (see mark()).
  function toggle(url) {
    const nowSelected = memory.unselected.has(url);
    if (nowSelected) memory.unselected.delete(url);
    else memory.unselected.add(url);
    memory.toggles.push({ url, selected: nowSelected });
    if (memory.toggles.length > MAX_PENDING_TOGGLES) memory.toggles.splice(0, memory.toggles.length - MAX_PENDING_TOGGLES);
    for (const element of document.querySelectorAll(`.${MARK_CLASS}`)) {
      if (memory.cardUrl.get(element) === url) paint(element);
    }
  }

  function mark(element, url) {
    memory.cardUrl.set(element, url);
    element.classList.add(MARK_CLASS);
    // An image cannot hold the badge; its outline is enough.
    if (element.tagName === "IMG") {
      paint(element);
      return;
    }
    // A scan keeps the badges it finds (a click needs the same element at press and release);
    // a recycled card (virtualized lists) may stand for another item now.
    const existing = element.querySelector(`:scope > .${BADGE_CLASS}`);
    if (existing) {
      existing.dataset.vdlUrl = url;
      paint(element);
      return;
    }
    // The badge needs a positioned parent; never override a page's own positioning
    // (virtualized lists place their rows absolutely).
    if (getComputedStyle(element).position === "static") {
      element.style.position = "relative";
      element.dataset.vdlStatic = "1";
    }
    const badge = document.createElement("span");
    badge.className = BADGE_CLASS;
    badge.setAttribute("role", "checkbox");
    badge.dataset.vdlUrl = url;
    for (const type of ["pointerdown", "pointerup", "mousedown", "mouseup"]) {
      badge.addEventListener(type, (event) => event.stopPropagation(), true);
    }
    badge.addEventListener(
      "click",
      (event) => {
        event.stopPropagation();
        event.preventDefault();
        const current = memory.cardUrl.get(element);
        if (current !== undefined) toggle(current);
      },
      true,
    );
    element.appendChild(badge);
    paint(element);
  }

  // The app's choice: the item URLs that are NOT selected. Cards already marked are redrawn.
  // Clicks the app has not taken yet are newer than what it knows, so they are applied on top.
  function setSelection(unselectedUrls) {
    memory.unselected = new Set(Array.isArray(unselectedUrls) ? unselectedUrls : []);
    for (const { url, selected } of memory.toggles) {
      if (selected) memory.unselected.delete(url);
      else memory.unselected.add(url);
    }
    let redrawn = 0;
    for (const element of document.querySelectorAll(`.${MARK_CLASS}`)) {
      paint(element);
      redrawn += 1;
    }
    return { unselected: memory.unselected.size, redrawn };
  }

  // The badge clicks since the last call, oldest first.
  function takeToggles() {
    return memory.toggles.splice(0);
  }

  // Cards a scan did not mark again lose their badge too.
  function dropStale(marked) {
    for (const element of marked) if (!element.classList.contains(MARK_CLASS)) unmark(element);
  }

  // options: includeAds keeps ad cards in the result; mark: false leaves the page unmarked (the
  // tray shows the videos that were played in the page, which have no cards in it).
  function scan(requestedShape, options) {
    const includeAds = Boolean(options?.includeAds);
    const markCards = options?.mark !== false;
    ensureStyle();
    // Take the outlines off before measuring and measure everything before marking, so the page
    // lays out once. The badges stay (see mark()); those of cards no longer marked go at the end.
    const marked = [...document.querySelectorAll(`.${MARK_CLASS}`)];
    for (const element of marked) element.classList.remove(MARK_CLASS, OFF_CLASS);

    const links = candidateLinks();
    assignShapes(links);
    const byShape = new Map();
    for (const link of links) {
      let group = byShape.get(link.shape);
      if (!group) {
        group = { shape: link.shape, path: link.path, urls: new Set(), links: [] };
        byShape.set(link.shape, group);
      }
      if (group.urls.size < MAX_GROUP_URLS || group.urls.has(link.url)) {
        group.urls.add(link.url);
        group.links.push(link);
      }
    }
    const candidates = [...byShape.values()].filter((group) => group.urls.size >= MIN_GROUP_URLS);
    // The page's own images form one group from a single image up, ranked like the others.
    const images = pageImages(includeAds);
    const imageGroup = images.found.length
      ? {
          shape: IMAGES_SHAPE,
          path: [],
          urls: new Set(images.found.map((image) => image.url)),
          links: [],
          cards: images.found.map((image) => image.img),
        }
      : null;
    if (imageGroup) candidates.push(imageGroup);
    const ranked = rankGroups(candidates);
    const groups = ranked.map((group) => ({ shape: group.shape, count: group.urls.size }));

    let chosen = null;
    let selected = [];
    let imagesChosen = false;
    if (requestedShape === IMAGES_SHAPE) {
      chosen = IMAGES_SHAPE;
      imagesChosen = true;
      if (!imageGroup) groups.unshift({ shape: IMAGES_SHAPE, count: 0 });
    } else if (requestedShape) {
      const pattern = parseShape(requestedShape);
      chosen = requestedShape;
      selected = links.filter((link) => matchesShape(pattern, link));
      if (!groups.some((group) => group.shape === requestedShape)) {
        groups.unshift({ shape: requestedShape, count: new Set(selected.map((link) => link.url)).size });
      }
    } else if (ranked.length) {
      chosen = ranked[0].shape;
      imagesChosen = ranked[0] === imageGroup;
      selected = ranked[0].links;
    }

    if (imagesChosen) {
      const items = [];
      const toMark = new Map();
      for (const { img, card, url, index } of images.found) {
        const title = (img.getAttribute("alt") || "").replace(/\s+/g, " ").trim();
        items.push({ url, title, thumbnail: url, index });
        if (!toMark.has(card)) toMark.set(card, url);
        memory.cards.set(url, card);
      }
      if (markCards) for (const [card, url] of toMark) mark(card, url);
      dropStale(marked);
      // The best link group goes along, folded under the images (「ほかのリンク」).
      const bestLinks = ranked.find((group) => group !== imageGroup);
      const otherLinks = bestLinks
        ? { shape: bestLinks.shape, urls: [...bestLinks.urls].slice(0, MAX_OTHER_LINKS) }
        : null;
      return { groups, shape: chosen, items, ads: images.ads, page: pageFacts(), otherLinks };
    }

    const cards = cardsFor(selected);
    const byUrl = new Map();
    selected.forEach((link, index) => {
      if (!byUrl.has(link.url)) byUrl.set(link.url, { anchors: [], cards: [] });
      const entry = byUrl.get(link.url);
      entry.anchors.push(link.anchor);
      if (!entry.cards.includes(cards[index])) entry.cards.push(cards[index]);
    });
    const items = [];
    const ads = [];
    const toMark = [];
    for (const [url, entry] of byUrl) {
      if (items.length >= MAX_GROUP_URLS) break;
      if (!includeAds && entry.cards.some(isAd)) {
        ads.push(url);
        continue;
      }
      items.push({ url, title: cardTitle(entry.anchors, entry.cards[0]), thumbnail: cardThumbnail(entry.anchors, entry.cards[0]) });
      for (const card of entry.cards) toMark.push([card, url]);
      memory.cards.set(url, entry.cards[0]);
    }
    if (markCards) for (const [card, url] of toMark) mark(card, url);
    dropStale(marked);
    return { groups, shape: chosen, items, ads, page: pageFacts(), otherLinks: null };
  }

  // --- The scroll step ---------------------------------------------------------------------

  // Some sites keep the window still and scroll an inner element (an app shell with
  // `overflow: auto` on its main column), so scrolling the window finds nothing new there.
  const MAX_SCROLLER_SEARCH = 5000;

  function isScrollable(element) {
    const overflow = getComputedStyle(element).overflowY;
    return (
      (overflow === "auto" || overflow === "scroll" || overflow === "overlay") &&
      element.scrollHeight > element.clientHeight + 1
    );
  }

  // The window itself scrolls through these elements (the body's overflow goes to the window
  // when the root element's own overflow is visible).
  function isRootScroller(element) {
    if (element === document.documentElement || element === document.scrollingElement) return true;
    return element === document.body && getComputedStyle(document.documentElement).overflowY === "visible";
  }

  // The thing that really scrolls the content: the nearest scrollable ancestor of the first
  // card still on the page, else the window if it scrolls, else the largest scrollable element
  // on the page, else the window. Returns an element or the window.
  function scrollTarget() {
    for (const card of memory.cards.values()) {
      if (!card.isConnected) continue;
      for (let el = card.parentElement; el && !isRootScroller(el); el = el.parentElement) {
        if (isScrollable(el)) return el;
      }
      break;
    }
    const root = document.scrollingElement || document.documentElement;
    if (root.scrollHeight > window.innerHeight + 1) return window;
    let best = null;
    let bestArea = 0;
    const all = document.querySelectorAll("*");
    for (let i = 0; i < all.length && i < MAX_SCROLLER_SEARCH; i += 1) {
      const el = all[i];
      if (isRootScroller(el) || !isScrollable(el)) continue;
      const size = el.clientWidth * el.clientHeight;
      if (size > bestArea) {
        best = el;
        bestArea = size;
      }
    }
    return best || window;
  }

  function measureScroll(target) {
    const isWindow = target === window;
    const source = isWindow ? document.scrollingElement || document.documentElement : target;
    const top = Math.round(source.scrollTop);
    const height = Math.round(source.scrollHeight);
    const client = Math.round(isWindow ? window.innerHeight : target.clientHeight);
    return { target: isWindow ? "window" : "element", top, height, client, atEnd: top + client >= height - 2 };
  }

  // Where the content scrolls now, without scrolling.
  function scrollInfo() {
    return measureScroll(scrollTarget());
  }

  // Scroll the scroller by 0.9 of its visible height; report where it is afterwards, and
  // whether the position changed at all (it did not when the scroller was already at its end).
  // "instant" overrides a page's own `scroll-behavior: smooth`, which would otherwise leave the
  // position unchanged until later and read as "did not move".
  function step() {
    const target = scrollTarget();
    const before = measureScroll(target);
    target.scrollBy({ top: Math.max(1, before.client * 0.9), left: 0, behavior: "instant" });
    const after = measureScroll(target);
    return { ...after, moved: after.top !== before.top };
  }

  // --- The element picker ------------------------------------------------------------------
  // Pick mode lets the user point at something on the page: a box follows the pointer and says
  // what the app would take from there (a video, a link and the cards shaped like it, an image).
  // One click on one of those ends it and leaves the answer in memory.pickResult for takePick();
  // a click on nothing is recorded as { kind: "none" } and pick mode stays on; Escape ends it
  // with { kind: "cancel" }.

  const PICK_BLUE = "#1F63D6";
  // How long the page's link statistics may be reused while the pointer moves, and how soon an
  // unknown link may trigger a fresh pass (a feed that grew since).
  const PICK_STATS_MS = 1500;
  const PICK_REFRESH_MS = 250;
  // The label's words, by the app's language.
  const PICK_TEXT = {
    ja: {
      video: "動画",
      link: (shape, count) => `リンク · ${shape} · 同じ形 ${count} 件`,
      image: "画像",
      none: "動画もリンクも見つかりません",
    },
    en: {
      video: "Video",
      link: (shape, count) => `Link · ${shape} · ${count} of the same shape`,
      image: "Image",
      none: "No video or link here",
    },
  };

  // A <video> whose rectangle holds the point: players lay controls over the video, so what is
  // under the pointer is not the video. The smallest such video wins.
  function videoAt(x, y) {
    let best = null;
    let bestArea = Infinity;
    for (const video of document.querySelectorAll("video")) {
      const rect = video.getBoundingClientRect();
      if (rect.width <= 0 || rect.height <= 0) continue;
      if (x < rect.left || x >= rect.right || y < rect.top || y >= rect.bottom) continue;
      if (rect.width * rect.height < bestArea) {
        best = video;
        bestArea = rect.width * rect.height;
      }
    }
    return best;
  }

  // The topmost element at the point that is the page's own: the selection badges sit over the
  // card, so pointing at one means pointing at what is under it.
  function pageElementAt(x, y) {
    for (const element of document.elementsFromPoint(x, y)) {
      if (!element.classList.contains(BADGE_CLASS)) return element;
    }
    return null;
  }

  // Every candidate link with its shape and card, and how many distinct URLs have each shape.
  function linkStats() {
    const links = candidateLinks();
    assignShapes(links);
    const cards = cardsFor(links);
    const byAnchor = new Map();
    const urlsByShape = new Map();
    links.forEach((link, index) => {
      byAnchor.set(link.anchor, { link, card: cards[index] });
      if (!urlsByShape.has(link.shape)) urlsByShape.set(link.shape, new Set());
      urlsByShape.get(link.shape).add(link.url);
    });
    return { time: Date.now(), byAnchor, urlsByShape };
  }

  function startPick(language) {
    if (memory.pickStop) memory.pickStop();
    memory.pickResult = null;
    const words = PICK_TEXT[language === "en" ? "en" : "ja"];
    let stats = null;
    let last = null;
    let current = null;

    const style = document.createElement("style");
    style.id = PICK_STYLE_ID;
    style.textContent = `html *, html .${BADGE_CLASS} { cursor: crosshair !important; }`;
    const box = document.createElement("div");
    box.id = PICK_BOX_ID;
    box.style.cssText = [
      "position: fixed",
      "display: none",
      "box-sizing: border-box",
      "pointer-events: none",
      `border: 2px solid ${PICK_BLUE}`,
      "background: rgba(31, 99, 214, 0.10)",
      "border-radius: 4px",
      "margin: 0",
      "z-index: 2147483647",
    ].join(" !important; ") + " !important";
    const label = document.createElement("div");
    label.id = PICK_LABEL_ID;
    label.style.cssText = [
      "position: fixed",
      "display: none",
      "box-sizing: border-box",
      "pointer-events: none",
      `background: ${PICK_BLUE}`,
      "color: #FFFFFF",
      "font: 600 12px/1.5 system-ui, sans-serif",
      "padding: 2px 6px",
      "border-radius: 3px",
      "margin: 0",
      "white-space: nowrap",
      "overflow: hidden",
      "text-overflow: ellipsis",
      "max-width: calc(100vw - 8px)",
      "z-index: 2147483647",
    ].join(" !important; ") + " !important";
    (document.head || document.documentElement).appendChild(style);
    document.documentElement.append(box, label);

    function statsFor(anchor) {
      if (!stats) stats = linkStats();
      let entry = stats.byAnchor.get(anchor);
      const age = Date.now() - stats.time;
      if ((!entry && age > PICK_REFRESH_MS) || age > PICK_STATS_MS) {
        stats = linkStats();
        entry = stats.byAnchor.get(anchor);
      }
      return entry ? { entry, urlsByShape: stats.urlsByShape } : null;
    }

    // What is at the point, in order: a video, a link, a media-sized image, nothing.
    function classify(x, y) {
      const video = videoAt(x, y);
      if (video) return { kind: "video", target: video, video };
      const under = pageElementAt(x, y);
      if (!under) return null;
      const anchor = under.closest("a[href]");
      const found = anchor ? statsFor(anchor) : null;
      if (found) {
        const { link, card } = found.entry;
        return { kind: "link", target: card, url: link.url, shape: link.shape, count: found.urlsByShape.get(link.shape).size };
      }
      if (under.tagName === "IMG" && isMediaBox(imageBox(under))) {
        return { kind: "image", target: under, url: pageImageUrl(under) };
      }
      return { kind: "none", target: under };
    }

    function labelText(info) {
      if (info.kind === "video") return words.video;
      if (info.kind === "link") return words.link(info.shape, info.count);
      if (info.kind === "image") return words.image;
      return words.none;
    }

    function draw() {
      if (!current) {
        box.style.setProperty("display", "none", "important");
        label.style.setProperty("display", "none", "important");
        return;
      }
      const rect = current.target.getBoundingClientRect();
      for (const [name, value] of [
        ["display", "block"],
        ["left", `${rect.left}px`],
        ["top", `${rect.top}px`],
        ["width", `${rect.width}px`],
        ["height", `${rect.height}px`],
      ]) {
        box.style.setProperty(name, value, "important");
      }
      label.textContent = labelText(current);
      label.style.setProperty("display", "block", "important");
      const labelWidth = label.offsetWidth;
      const labelHeight = label.offsetHeight;
      const viewWidth = document.documentElement.clientWidth || window.innerWidth;
      // Above the box; inside its top-left corner when there is no room above.
      const above = rect.top - labelHeight >= 0;
      const top = above ? rect.top - labelHeight : Math.max(rect.top, 0) + 4;
      const left = Math.max(0, Math.min(above ? rect.left : rect.left + 4, viewWidth - labelWidth));
      label.style.setProperty("left", `${left}px`, "important");
      label.style.setProperty("top", `${top}px`, "important");
    }

    function update() {
      if (!last) return;
      current = classify(last.x, last.y);
      draw();
    }

    function pointOf(event) {
      // A click made by script has no position; the last pointer position stands in for it.
      if (last && event.clientX === 0 && event.clientY === 0) return last;
      return { x: event.clientX, y: event.clientY };
    }

    function swallow(event) {
      event.preventDefault();
      event.stopImmediatePropagation();
    }

    function onMove(event) {
      last = { x: event.clientX, y: event.clientY };
      update();
    }

    // The page moves under a still pointer when it scrolls or the window is resized.
    function onResize() {
      update();
    }

    let scrollFrame = 0;
    function onScroll() {
      if (scrollFrame) return;
      scrollFrame = requestAnimationFrame(() => {
        scrollFrame = 0;
        update();
      });
    }

    function onClick(event) {
      swallow(event);
      const point = pointOf(event);
      const info = classify(point.x, point.y);
      if (!info) {
        memory.pickResult = { kind: "cancel" };
        stop();
        return;
      }
      if (info.kind === "none") {
        // Nothing to take there: say so and wait for another press.
        memory.pickResult = { kind: "none" };
        return;
      }
      const rect = info.target.getBoundingClientRect();
      const videos = info.video ? [...document.querySelectorAll("video")] : [];
      memory.pickResult = {
        kind: info.kind,
        url:
          info.kind === "video"
            ? httpUrl(info.video.currentSrc) || httpUrl(info.video.getAttribute("src"))
            : info.url ?? null,
        shape: info.kind === "link" ? info.shape : null,
        count: info.kind === "link" ? info.count : null,
        videoIndex: info.video ? videos.indexOf(info.video) : null,
        rect: { x: rect.x, y: rect.y, width: rect.width, height: rect.height },
      };
      stop();
    }

    function onKey(event) {
      if (event.key !== "Escape") return;
      swallow(event);
      memory.pickResult = { kind: "cancel" };
      stop();
    }

    const swallowed = ["pointerdown", "pointerup", "mousedown", "mouseup"];
    function stop() {
      window.removeEventListener("mousemove", onMove, true);
      window.removeEventListener("click", onClick, true);
      window.removeEventListener("keydown", onKey, true);
      window.removeEventListener("scroll", onScroll, true);
      window.removeEventListener("resize", onResize, true);
      for (const type of swallowed) window.removeEventListener(type, swallow, true);
      if (scrollFrame) cancelAnimationFrame(scrollFrame);
      style.remove();
      box.remove();
      label.remove();
      if (memory.pickStop === stop) memory.pickStop = null;
    }

    window.addEventListener("mousemove", onMove, true);
    window.addEventListener("click", onClick, true);
    window.addEventListener("keydown", onKey, true);
    window.addEventListener("scroll", onScroll, true);
    window.addEventListener("resize", onResize, true);
    for (const type of swallowed) window.addEventListener(type, swallow, true);
    memory.pickStop = stop;
    return { picking: true };
  }

  // Ends pick mode without a result (the app's stop button, the tray closing, a page change).
  function stopPick() {
    if (memory.pickStop) memory.pickStop();
    return { picking: false };
  }

  // The last pick, once; null until something was picked (or after it was taken).
  function takePick() {
    const result = memory.pickResult;
    memory.pickResult = null;
    return result;
  }

  // Scroll an item's card into view and flash its outline. A card the page has dropped since
  // (virtualized lists remove what scrolled away) is looked up again by its link.
  function reveal(url) {
    let card = memory.cards.get(url);
    if (!card || !card.isConnected) {
      const anchor = [...document.querySelectorAll("a[href]")].find((a) => {
        try {
          const href = new URL(a.href, location.href);
          href.hash = "";
          return href.href === url;
        } catch {
          return false;
        }
      });
      card = anchor?.closest(`.${MARK_CLASS}`) || anchor || imageCard(url);
    }
    if (!card) return { found: false };
    const smooth = !window.matchMedia("(prefers-reduced-motion: reduce)").matches;
    card.scrollIntoView({ block: "center", inline: "nearest", behavior: smooth ? "smooth" : "auto" });
    if (card.classList.contains(MARK_CLASS)) {
      card.classList.add(REVEAL_CLASS);
      setTimeout(() => card.classList.remove(REVEAL_CLASS), 1600);
    }
    return { found: true };
  }

  function clear() {
    if (memory.pickStop) memory.pickStop();
    memory.pickResult = null;
    memory.unselected = new Set();
    memory.toggles = [];
    for (const element of document.querySelectorAll(`.${MARK_CLASS}`)) unmark(element);
    for (const element of document.querySelectorAll(`.${REVEAL_CLASS}`)) element.classList.remove(REVEAL_CLASS);
    memory.cards.clear();
    memory.imagesPage = null;
    memory.imageIndex = new Map();
    document.getElementById(STYLE_ID)?.remove();
  }

  window.__vdlCollector = {
    scan,
    step,
    scrollInfo,
    reveal,
    clear,
    pageFacts,
    setSelection,
    takeToggles,
    startPick,
    stopPick,
    takePick,
  };
})();
