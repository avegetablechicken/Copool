/* Morphs the account list between full cards and collapsed rows with
   continuity: the cards stay a contiguous stack while each changes size in
   turn, the parts a card and its row share (name, plan, usage, controls)
   travel between their places, and the rest fades out or in where it is.
   Used by the UI and node:test. */
(function (root, factory) {
  const api = factory();
  if (typeof module === "object" && module.exports) module.exports = api;
  else root.CopoolMorph = api;
})(globalThis, function () {
  const lerp = (a, b, e) => a + (b - a) * e;
  const clamp01 = (x) => Math.min(1, Math.max(0, x));
  // A shared part's two forms swap over the middle of its travel, not
  // across all of it, so neither shows doubled for long.
  const swap = (e) => {
    const x = clamp01((e - 0.3) / 0.4);
    return x * x * (3 - 2 * x);
  };
  // Usage and controls line up by their right edges, where their numbers
  // and buttons are; names and plans by their left edges.
  const RIGHT_ALIGNED = /:usage\d$|^controls$/;

  /// A critically damped spring over [0, 1]: quick to start, settling
  /// without overshoot, so no card or text swings past its place.
  function ease(p) {
    const w = 6.6;
    const f = (x) => 1 - (1 + w * x) * Math.exp(-w * x);
    return f(clamp01(p)) / f(1);
  }

  /// Each item's progress and rect at `t` ms. Items change in turn, `stagger`
  /// ms apart, yet stay a stack: every top follows the item above it, the
  /// gap between them easing from its old size to its new one.
  function stack(items, t, { stagger, duration }) {
    const out = [];
    items.forEach(({ from, to }, i) => {
      const p = clamp01((t - i * stagger) / duration);
      const e = ease(p);
      const rect = {
        left: lerp(from.left, to.left, e),
        width: lerp(from.width, to.width, e),
        height: lerp(from.height, to.height, e),
        top: lerp(from.top, to.top, e),
      };
      if (i > 0) {
        const above = items[i - 1];
        const gap = lerp(
          from.top - (above.from.top + above.from.height),
          to.top - (above.to.top + above.to.height),
          e,
        );
        const prev = out[i - 1].rect;
        rect.top = prev.top + prev.height + gap;
      }
      out.push({ p, e, rect });
    });
    return out;
  }

  /// The parts a card shares between its full and collapsed forms, by key.
  function sharedParts(entry) {
    const found = new Map();
    const cards = entry.dataset?.cardId ? [entry] : [];
    cards.push(...entry.querySelectorAll("[data-card-id]"));
    for (const card of cards) {
      const id = card.dataset.cardId;
      const put = (key, node) => node && found.set(`${id}:${key}`, node);
      put("name", card.querySelector(".account-name .block-title, :scope > .account-row-name"));
      put("plan", card.querySelector(".tag.plan:not(.placeholder)"));
      card
        .querySelectorAll(".usage > .usage-value, :scope > .usage-value")
        .forEach((node, i) => put(`usage${i}`, node));
    }
    const controls = entry.querySelector(
      ":scope > .account-footer > .block-aside, :scope > .block-aside, :scope > .account-row:first-child > .block-aside",
    );
    if (controls) found.set("controls", controls);
    return found;
  }

  /// The largest elements under `root` holding no shared part: they fade
  /// as wholes while the shared parts travel.
  function fadingParts(root, shared) {
    const out = [];
    const walk = (node) => {
      for (const child of node.children) {
        if (shared.has(child)) continue;
        if ([...shared].some((s) => child.contains(s))) walk(child);
        else out.push(child);
      }
    };
    walk(root);
    return out;
  }

  /// Rects of `entry` and its shared parts, relative to the entry.
  function measure(entry) {
    const rect = entry.getBoundingClientRect();
    const parts = new Map();
    for (const [key, node] of sharedParts(entry)) {
      const r = node.getBoundingClientRect();
      parts.set(key, {
        left: r.left - rect.left,
        top: r.top - rect.top,
        width: r.width,
        height: r.height,
      });
    }
    return { rect, parts };
  }

  /// Runs the morph in `layer` (positioned at `frame`). Each item has a
  /// `source` and `target` copy with its measures; `sheet` is the collapsed
  /// list's background, merging in at the end of a collapse and splitting
  /// at the start of an expansion. `onFrame(frames)` follows every frame, so
  /// the panel can follow the cards. Resolves when done; `stop()` ends early.
  function run({
    layer,
    frame,
    items,
    sheet,
    collapsing,
    duration,
    stagger,
    radius,
    onFrame = () => {},
  }) {
    // Backgrounds sit outside the clipping boxes, which would cut off their
    // outlines, and under all of them.
    const backgrounds = items.map(() => {
      const bg = document.createElement("div");
      bg.className = "morph-bg";
      layer.append(bg);
      return bg;
    });
    const views = items.map((item, i) => {
      const box = document.createElement("div");
      box.className = "morph-box";
      const bg = backgrounds[i];
      const copies = [item.source, item.target].map(({ node, measure: m }) => {
        node.classList.add("morph-copy");
        Object.assign(node.style, {
          width: `${m.rect.width}px`,
          height: `${m.rect.height}px`,
        });
        node.inert = true;
        box.append(node);
        const shared = sharedParts(node);
        // A provider's account panels hold shared parts, so their tint
        // fades as a part of its own, laid under them.
        const tints = [...node.querySelectorAll(".account-member")].map((member) => {
          const tint = document.createElement("div");
          tint.className = "account-member morph-tint";
          node.prepend(tint);
          member.style.background = "transparent";
          return [tint, member];
        });
        const fading = fadingParts(node, new Set(shared.values()));
        return { node, m, shared, fading, tints };
      });
      layer.append(box);
      for (const { tints } of copies)
        for (const [tint, member] of tints)
          Object.assign(tint.style, {
            left: `${member.offsetLeft}px`,
            top: `${member.offsetTop}px`,
            width: `${member.offsetWidth}px`,
            height: `${member.offsetHeight}px`,
          });
      const [source, target] = copies;
      const pairs = [];
      for (const [key, node] of source.shared) {
        const other = target.shared.get(key);
        const a = source.m.parts.get(key);
        const b = target.m.parts.get(key);
        if (!other || !a || !b) {
          source.fading.push(node);
          continue;
        }
        const right = RIGHT_ALIGNED.test(key);
        pairs.push({
          from: node,
          to: other,
          dx: right ? b.left + b.width - (a.left + a.width) : b.left - a.left,
          dy: b.top + b.height / 2 - (a.top + a.height / 2),
        });
      }
      for (const [key, node] of target.shared)
        if (!source.shared.has(key) || !pairs.some((p) => p.to === node))
          target.fading.push(node);
      return { box, bg, source, target, pairs };
    });
    const plan = items.map((item) => ({
      from: item.source.measure.rect,
      to: item.target.measure.rect,
    }));
    const total = (items.length - 1) * stagger + duration;
    let stopped = false;
    let start = null;
    const draw = (t) => {
      const frames = stack(plan, t, { stagger, duration });
      onFrame(frames);
      // The collapsed list's background: it merges in as the last cards
      // settle into rows, or splits away as the first rows start to grow.
      const merged = collapsing
        ? clamp01((frames[frames.length - 1].p - 0.55) / 0.45)
        : 1 - clamp01(frames[0].p / 0.45);
      if (sheet) sheet.style.opacity = String(merged);
      frames.forEach(({ p, e, rect }, i) => {
        const v = views[i];
        const place = {
          left: `${rect.left - frame.left}px`,
          top: `${rect.top - frame.top}px`,
          width: `${rect.width}px`,
          height: `${rect.height}px`,
          borderRadius: `${lerp(radius.source, radius.target, e)}px`,
        };
        Object.assign(v.box.style, place);
        Object.assign(v.bg.style, place);
        v.bg.style.opacity = String(1 - merged);
        // Folding, the full card's details clear before the row's text comes
        // in by the end; unfolding, the details come in early, so a growing
        // card is never bare.
        const [outEnd, inStart, inEnd] = collapsing ? [0.45, 0.5, 1] : [0.3, 0.15, 0.6];
        const out = String(1 - clamp01(p / outEnd));
        const into = String(clamp01((p - inStart) / (inEnd - inStart)));
        for (const node of v.source.fading) node.style.opacity = out;
        for (const node of v.target.fading) node.style.opacity = into;
        const shown = swap(e);
        for (const { from, to, dx, dy } of v.pairs) {
          from.style.transform = `translate(${dx * e}px, ${dy * e}px)`;
          from.style.opacity = String(1 - shown);
          to.style.transform = `translate(${-dx * (1 - e)}px, ${-dy * (1 - e)}px)`;
          to.style.opacity = String(shown);
        }
      });
    };
    draw(0);
    return {
      total,
      done: new Promise((resolve) => {
        const tick = (now) => {
          if (stopped) return resolve();
          start ??= now;
          const t = now - start;
          draw(t);
          if (t >= total) return resolve();
          requestAnimationFrame(tick);
        };
        requestAnimationFrame(tick);
      }),
      stop: () => {
        stopped = true;
      },
    };
  }

  return { ease, stack, sharedParts, fadingParts, measure, run };
});
