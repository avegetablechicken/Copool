/* Drag-to-reorder for the account cards, after OpenQuota's pointerReorder:
   a card lifts once the pointer moves 4 px from its handle, the others make
   way live with a spring, and Escape cancels. Used by the UI and node:test. */
(function (root, factory) {
  const api = factory();
  if (typeof module === "object" && module.exports) module.exports = api;
  else root.CopoolReorder = api;
})(globalThis, function () {
  const START_DISTANCE = 4;
  // How far into a neighbour the pointer must go before the two swap.
  const CROSSING_THRESHOLD = 0.2;
  const EDGE = 52;
  const MAX_SCROLL = 12;
  const LIFT_INSET = 6;
  // How far the pointer must travel in a swap's direction after the last one.
  const SWAP_HYSTERESIS = 4;
  // SwiftUI-style spring: response 0.42 s, damping 0.8, run for 1.5 responses.
  const RESPONSE = 0.42;
  const DAMPING = 0.8;
  const SETTLE = 1.5;
  const DURATION = RESPONSE * SETTLE * 1000;
  const INTERACTIVE =
    "button, input, label, a, select, textarea, [contenteditable], [role=button]";

  function spring(progress) {
    const root = Math.sqrt(1 - DAMPING * DAMPING);
    const raw = (t) =>
      1 -
      Math.exp(-DAMPING * 2 * Math.PI * t) *
        (Math.cos(2 * Math.PI * root * t) +
          (DAMPING / root) * Math.sin(2 * Math.PI * root * t));
    return raw(progress * SETTLE) / raw(SETTLE);
  }
  const springEasing = `linear(${Array.from({ length: 41 }, (_, i) =>
    Number(spring(i / 40).toFixed(4)),
  ).join(", ")})`;

  const reducedMotion = () =>
    globalThis.matchMedia?.("(prefers-reduced-motion: reduce)").matches;
  const easing = () =>
    globalThis.CSS?.supports?.("transition-timing-function", springEasing)
      ? springEasing
      : "cubic-bezier(0.2, 0.9, 0.3, 1)";
  /// Slides `node` with the reorder spring from `from`, its rect before the
  /// DOM changed. A raised node passes over the others on its way.
  function slide(node, from, raise = false) {
    if (reducedMotion()) return;
    for (const running of node.getAnimations()) running.cancel();
    const to = node.getBoundingClientRect();
    const dx = from.left - to.left;
    const dy = from.top - to.top;
    if (!dx && !dy) return;
    const animation = node.animate(
      [{ transform: `translate(${dx}px, ${dy}px)` }, { transform: "none" }],
      { duration: DURATION, easing: easing() },
    );
    if (!raise) return;
    node.classList.add("reorder-raised");
    const lower = () => node.classList.remove("reorder-raised");
    animation.addEventListener("finish", lower);
    animation.addEventListener("cancel", lower);
  }

  /// The index of the card the pointer has gone far enough into to take its
  /// place, or null. `frames` are the cards' rects in list order.
  function targetAt(frames, point, from) {
    for (let to = 0; to < frames.length; to++) {
      if (to === from) continue;
      const f = frames[to];
      if (
        point.x < f.left ||
        point.x > f.right ||
        point.y < f.top - 2 ||
        point.y > f.bottom + 2
      )
        continue;
      const height = Math.max(1, f.bottom - f.top);
      if (to > from && point.y < f.top + height * CROSSING_THRESHOLD) return null;
      if (to < from && point.y > f.bottom - height * CROSSING_THRESHOLD) return null;
      return to;
    }
    return null;
  }
  /// Whether the pointer, at `position` in the list's scrolled content, has
  /// moved toward `to` since the last swap at `anchor`. A swap back to where
  /// a card came from needs a move back; a tall neighbour that is still under
  /// the pointer after a swap then cannot swap straight back.
  function movedToward(to, from, position, anchor) {
    return to > from
      ? position >= anchor + SWAP_HYSTERESIS
      : position <= anchor - SWAP_HYSTERESIS;
  }
  /// Pixels to scroll this frame with the pointer near a scroller's edge.
  function edgeScrollDelta(y, top, bottom) {
    const edge = Math.min(EDGE, (bottom - top) * 0.22);
    if (edge <= 0) return 0;
    const depth = (into) => Math.ceil(Math.min(1, into / edge) * MAX_SCROLL);
    if (y < top + edge) return -depth(top + edge - y);
    if (y > bottom - edge) return depth(y - (bottom - edge));
    return 0;
  }
  function scrollParent(node) {
    for (let n = node.parentElement; n; n = n.parentElement) {
      const overflow = getComputedStyle(n).overflowY;
      if (/(auto|scroll)/.test(overflow) && n.scrollHeight > n.clientHeight)
        return n;
    }
    return null;
  }
  /// Swallows the click a drag's release would otherwise send.
  function suppressClick() {
    const swallow = (event) => {
      event.preventDefault();
      event.stopPropagation();
    };
    window.addEventListener("click", swallow, true);
    setTimeout(() => window.removeEventListener("click", swallow, true), 0);
  }

  /// Makes the children of `list` with `data-reorder-id` draggable by their
  /// `data-reorder-handle` parts. One marked `data-reorder-fixed` keeps its
  /// place: it cannot be dragged or swapped with. `onEnd({ order, moved,
  /// cancelled })` gets the ids in their final order.
  function attach(list, { onStart = () => {}, onEnd = () => {} } = {}) {
    let gesture = null;
    const items = () =>
      [...list.children].filter((node) => node.dataset.reorderId);
    const movable = () =>
      items().filter((node) => !("reorderFixed" in node.dataset));

    /// Reorders the DOM, sliding every other card from where it was.
    function flip(apply) {
      const nodes = items();
      const before = new Map(nodes.map((n) => [n, n.getBoundingClientRect()]));
      apply();
      for (const node of nodes)
        if (node !== gesture?.item) slide(node, before.get(node));
    }
    function moveLift(point) {
      const { lift, offset, width, height } = gesture;
      const left = Math.min(
        Math.max(point.x - offset.x, LIFT_INSET),
        Math.max(LIFT_INSET, window.innerWidth - width - LIFT_INSET),
      );
      const top = Math.min(
        Math.max(point.y - offset.y, LIFT_INSET),
        Math.max(LIFT_INSET, window.innerHeight - height - LIFT_INSET),
      );
      lift.style.left = `${left}px`;
      lift.style.top = `${top}px`;
    }
    function reorderAt(point) {
      const nodes = movable();
      const from = nodes.indexOf(gesture.item);
      const to = targetAt(
        nodes.map((n) => n.getBoundingClientRect()),
        point,
        from,
      );
      const position = point.y + (gesture.scroller?.scrollTop ?? 0);
      if (to === null || !movedToward(to, from, position, gesture.anchor)) return;
      gesture.anchor = position;
      flip(() =>
        list.insertBefore(
          gesture.item,
          to > from ? nodes[to].nextSibling : nodes[to],
        ),
      );
    }
    function lift(item, point) {
      const rect = item.getBoundingClientRect();
      const layer = document.createElement("div");
      layer.className = "reorder-layer";
      const clone = item.cloneNode(true);
      clone.classList.add("reorder-lift");
      clone.removeAttribute("id");
      clone.setAttribute("aria-hidden", "true");
      for (const node of clone.querySelectorAll("[tabindex], button, input, a"))
        node.tabIndex = -1;
      // The list's shared column widths, which the copy leaves behind.
      for (const name of list.style)
        if (name.startsWith("--"))
          clone.style.setProperty(name, list.style.getPropertyValue(name));
      clone.style.width = `${rect.width}px`;
      clone.style.height = `${rect.height}px`;
      layer.append(clone);
      document.body.append(layer);
      Object.assign(gesture, {
        dragging: true,
        layer,
        lift: clone,
        width: rect.width,
        height: rect.height,
        offset: {
          x: Math.min(Math.max(point.x - rect.left, 0), rect.width),
          y: Math.min(Math.max(point.y - rect.top, 0), rect.height),
        },
        scroller: scrollParent(list),
      });
      gesture.anchor = gesture.start.y + (gesture.scroller?.scrollTop ?? 0);
      moveLift(point);
      item.classList.add("reorder-source");
      document.documentElement.classList.add("is-reordering");
      try {
        list.setPointerCapture(gesture.pointerId);
      } catch {}
      onStart();
      const scroll = () => {
        if (!gesture?.dragging) return;
        const scroller = gesture.scroller;
        if (scroller && gesture.point) {
          const frame = scroller.getBoundingClientRect();
          const delta = edgeScrollDelta(gesture.point.y, frame.top, frame.bottom);
          const before = scroller.scrollTop;
          if (delta) scroller.scrollTop += delta;
          if (scroller.scrollTop !== before) reorderAt(gesture.point);
        }
        gesture.frame = requestAnimationFrame(scroll);
      };
      gesture.frame = requestAnimationFrame(scroll);
    }
    function finish(cancelled) {
      const g = gesture;
      gesture = null;
      window.removeEventListener("pointermove", move, true);
      window.removeEventListener("pointerup", up, true);
      window.removeEventListener("pointercancel", cancel, true);
      window.removeEventListener("blur", cancel);
      window.removeEventListener("keydown", key, true);
      if (!g.dragging) return;
      cancelAnimationFrame(g.frame);
      try {
        list.releasePointerCapture(g.pointerId);
      } catch {}
      g.layer.remove();
      document.documentElement.classList.remove("is-reordering");
      if (cancelled) {
        gesture = g;
        flip(() => list.append(...g.original));
        gesture = null;
      }
      g.item.classList.remove("reorder-source");
      suppressClick();
      const order = items().map((n) => n.dataset.reorderId);
      onEnd({
        order,
        moved: order.join("\n") !== g.original.map((n) => n.dataset.reorderId).join("\n"),
        cancelled,
      });
    }
    function move(event) {
      if (!gesture || event.pointerId !== gesture.pointerId) return;
      const point = { x: event.clientX, y: event.clientY };
      if (!gesture.dragging) {
        if (
          Math.hypot(point.x - gesture.start.x, point.y - gesture.start.y) <
          START_DISTANCE
        )
          return;
        lift(gesture.item, point);
      }
      event.preventDefault();
      gesture.point = point;
      moveLift(point);
      reorderAt(point);
    }
    function up(event) {
      if (gesture && event.pointerId === gesture.pointerId) finish(false);
    }
    function cancel() {
      if (gesture) finish(true);
    }
    function key(event) {
      if (!gesture?.dragging || event.key !== "Escape") return;
      // Escape cancels the drag and must not also close the panel.
      event.preventDefault();
      event.stopPropagation();
      finish(true);
    }
    list.addEventListener("pointerdown", (event) => {
      if (gesture || event.button !== 0 || !event.isPrimary) return;
      const item = event.target.closest("[data-reorder-id]");
      if (!item || item.parentElement !== list || "reorderFixed" in item.dataset)
        return;
      const handle = event.target.closest("[data-reorder-handle]");
      if (!handle || !item.contains(handle)) return;
      const control = event.target.closest(INTERACTIVE);
      if (control && item.contains(control)) return;
      gesture = {
        item,
        pointerId: event.pointerId,
        start: { x: event.clientX, y: event.clientY },
        original: items(),
        dragging: false,
        anchor: null,
      };
      window.addEventListener("pointermove", move, { capture: true, passive: false });
      window.addEventListener("pointerup", up, true);
      window.addEventListener("pointercancel", cancel, true);
      window.addEventListener("blur", cancel);
      window.addEventListener("keydown", key, true);
    });
    return { get active() { return !!gesture?.dragging; } };
  }

  return { attach, slide, targetAt, movedToward, edgeScrollDelta, spring, springEasing };
});
