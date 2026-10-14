const { test } = require("node:test");
const assert = require("node:assert/strict");
const M = require("../ui/morph.js");

test("The morph curve settles without overshoot", () => {
  assert.equal(M.ease(0), 0);
  assert.ok(Math.abs(M.ease(1) - 1) < 1e-12);
  let last = 0;
  for (let i = 1; i <= 100; i++) {
    const e = M.ease(i / 100);
    assert.ok(e >= last && e <= 1 + 1e-12, `${i}: ${e}`);
    last = e;
  }
});

// Three full cards 8 px apart folding into rows that touch.
const items = [
  { from: { left: 0, top: 0, width: 400, height: 150 }, to: { left: 4, top: 4, width: 392, height: 26 } },
  { from: { left: 0, top: 158, width: 400, height: 250 }, to: { left: 4, top: 30, width: 392, height: 52 } },
  { from: { left: 0, top: 416, width: 400, height: 150 }, to: { left: 4, top: 82, width: 392, height: 26 } },
];
const timing = { stagger: 22, duration: 340 };
test("The cards start and end where they were and will be", () => {
  const start = M.stack(items, 0, timing);
  const end = M.stack(items, 2 * 22 + 340, timing);
  items.forEach(({ from, to }, i) => {
    for (const k of ["left", "top", "width", "height"]) {
      assert.ok(Math.abs(start[i].rect[k] - from[k]) < 1e-9, `start ${i} ${k}`);
      assert.ok(Math.abs(end[i].rect[k] - to[k]) < 1e-9, `end ${i} ${k}`);
    }
  });
});
test("Folding in turn, the cards stay a stack with no gap opening or overlap", () => {
  for (let t = 0; t <= 400; t += 4) {
    const frames = M.stack(items, t, timing);
    for (let i = 1; i < frames.length; i++) {
      const above = frames[i - 1].rect;
      const gap = frames[i].rect.top - (above.top + above.height);
      assert.ok(gap >= -1e-9 && gap <= 8 + 1e-9, `t ${t}, card ${i}: gap ${gap}`);
    }
    // Each card folds only once the one above it has begun.
    for (let i = 1; i < frames.length; i++)
      assert.ok(frames[i].p <= frames[i - 1].p, `t ${t}, card ${i}`);
  }
});
