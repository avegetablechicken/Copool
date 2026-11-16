const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const vm = require("node:vm");
const source = fs.readFileSync(require("node:path").join(__dirname, "../ui/app.js"), "utf8");

async function dialog(times) {
  const nodes = new Map();
  const $ = (id) => {
    if (!nodes.has(id)) nodes.set(id, {
      value: "", disabled: false, focus() {}, showModal() {}, close() {},
      reportValidity() { return /^\d{2}:\d{2}$/.test(this.value); },
    });
    return nodes.get(id);
  };
  let menu;
  const saves = [];
  const scope = { $, pool: "codex", openMenu: null, tr: (key) => key,
    canSayHi: () => true, showMenu: (_, items) => { menu = items; },
    invoke: async (command, args) => {
      if (command === "greeting_schedule") return { auto: true, times: [...times] };
      saves.push({ auto: args.auto, times: Array.from(args.times) });
    }, notice: (error) => { throw error; },
  };
  vm.createContext(scope);
  vm.runInContext(source.slice(source.indexOf("async function showSayHiMenu("), source.indexOf("/// The plan tag.")), scope);
  await scope.showSayHiMenu({ isConnected: true }, { id: "test", label: "test" });
  return { $, saves, choices: menu[2].children };
}

test("Add time recognizes saved times and can remove them without changing Auto", async () => {
  const { $, saves, choices } = await dialog(["08:00", "12:30"]);
  choices.at(-1).run();
  assert.equal($("say-hi-time").value, "12:30");
  assert.equal($("say-hi-time-remove").hidden, false);
  $("say-hi-time").value = "09:00";
  $("say-hi-time").oninput();
  assert.equal($("say-hi-time-remove").hidden, true);
  $("say-hi-time").value = "08:00";
  $("say-hi-time").oninput();
  assert.equal($("say-hi-time-remove").hidden, false);
  $("say-hi-time-remove").onclick();
  await new Promise(setImmediate);
  assert.deepEqual(saves, [{ auto: true, times: ["12:30"] }]);
});

test("Editing can delete the last saved time even if the time input was cleared", async () => {
  const { $, saves, choices } = await dialog(["08:00"]);
  choices[0].run();
  $("say-hi-time").value = "";
  $("say-hi-time").oninput();
  assert.equal($("say-hi-time-remove").hidden, false);
  $("say-hi-time-remove").onclick();
  await new Promise(setImmediate);
  assert.deepEqual(saves, [{ auto: true, times: [] }]);
});

test("Time menu rows can remove several saved times without opening the editor", async () => {
  const { $, saves, choices } = await dialog(["08:00", "12:30"]);
  let opened = false;
  $("say-hi-time-dialog").showModal = () => { opened = true; };
  await choices[0].remove();
  await choices[1].remove();
  assert.equal(opened, false);
  assert.deepEqual(saves, [
    { auto: true, times: ["12:30"] },
    { auto: true, times: [] },
  ]);
  assert.equal(choices.at(-1).remove, undefined, "Add time is never removable");
});
