const { test } = require("node:test");
const assert = require("node:assert/strict");
const R = require("../ui/reorder.js");

const frames = [0, 1, 2].map((i) => ({
  left: 0,
  right: 100,
  top: i * 110,
  bottom: i * 110 + 100,
}));
test("A card takes a neighbour's place once the pointer is 20% into it", () => {
  // Down from 0: the next card's top 20% is not enough.
  assert.equal(R.targetAt(frames, { x: 50, y: 115 }, 0), null);
  assert.equal(R.targetAt(frames, { x: 50, y: 135 }, 0), 1);
  assert.equal(R.targetAt(frames, { x: 50, y: 250 }, 0), 2);
  // Up from 2: the bottom 20% of the card above is not enough.
  assert.equal(R.targetAt(frames, { x: 50, y: 195 }, 2), null);
  assert.equal(R.targetAt(frames, { x: 50, y: 150 }, 2), 1);
  // Its own slot, the gaps and outside the cards take nothing.
  assert.equal(R.targetAt(frames, { x: 50, y: 50 }, 0), null);
  assert.equal(R.targetAt(frames, { x: 50, y: 105 }, 0), null);
  assert.equal(R.targetAt(frames, { x: 150, y: 150 }, 0), null);
});
test("Edges scroll faster the deeper the pointer, up to 12 px a frame", () => {
  assert.equal(R.edgeScrollDelta(300, 0, 600), 0);
  assert.equal(R.edgeScrollDelta(0, 0, 600), -12);
  assert.equal(R.edgeScrollDelta(-40, 0, 600), -12);
  assert.equal(R.edgeScrollDelta(574, 0, 600), 6);
  // A short scroller's edge is 22% of its height.
  assert.equal(R.edgeScrollDelta(21, 0, 100), -1);
  assert.equal(R.edgeScrollDelta(50, 0, 100), 0);
});
test("The spring starts at 0, settles at 1 and overshoots on the way", () => {
  assert.equal(R.spring(0), 0);
  assert.ok(Math.abs(R.spring(1) - 1) < 1e-9);
  assert.ok(Math.max(...Array.from({ length: 41 }, (_, i) => R.spring(i / 40))) > 1);
  assert.match(R.springEasing, /^linear\(0, .*, 1\)$/);
});
/// Drags the first card along `path` as the list does, with cards of
/// `heights` 10 px apart: the order it ends in.
function drag(heights, path) {
  let order = heights.map((_, i) => i);
  const frames = () => {
    let top = 0;
    return order.map((i) => {
      const frame = { left: 0, right: 100, top, bottom: top + heights[i] };
      top += heights[i] + 10;
      return frame;
    });
  };
  const item = order[0];
  let anchor = path[0];
  for (const y of path) {
    const from = order.indexOf(item);
    const to = R.targetAt(frames(), { x: 50, y }, from);
    if (to === null || !R.movedToward(to, from, y, anchor)) continue;
    anchor = y;
    order.splice(from, 1);
    order.splice(to, 0, item);
  }
  return order;
}
const steps = (from, to, step = 5) =>
  Array.from({ length: Math.abs(to - from) / step + 1 }, (_, i) =>
    from + Math.sign(to - from) * i * step,
  );
test("A card dragged past a neighbour can be dragged back to its place", () => {
  const down = steps(10, 140);
  assert.deepEqual(drag([100, 100, 100], down), [1, 0, 2]);
  assert.deepEqual(drag([100, 100, 100], [...down, ...steps(140, 10)]), [0, 1, 2]);
  // Past two cards and back again.
  const far = steps(10, 250);
  assert.deepEqual(drag([100, 100, 100], far), [1, 2, 0]);
  assert.deepEqual(drag([100, 100, 100], [...far, ...steps(250, 10)]), [0, 1, 2]);
});
test("A tall neighbour does not swap back while the pointer keeps going", () => {
  // After a short card swaps below a tall one, the pointer is still over the
  // tall card, which must not swap straight back.
  const down = steps(10, 200);
  assert.deepEqual(drag([100, 300, 100], down), [1, 0, 2]);
  // Moving back up returns it.
  assert.deepEqual(drag([100, 300, 100], [...down, ...steps(200, 10)]), [0, 1, 2]);
});
