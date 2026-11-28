// Plain nodes standing in for the DOM in the UI modules' tests: what app.js's
// el(tag, className, text), button(label, glyph, className, handler) and
// icon(name) build, with their children, attributes and handlers kept for
// the tests to read and run.
class Node {
  constructor(tag, className = "", text = "") {
    this.tag = tag;
    this.className = className || "";
    this.textContent = text ?? "";
    this.title = "";
    this.dataset = {};
    this.attributes = {};
    this.listeners = {};
    this.style = {};
    this.children = [];
    this.parent = null;
    const classes = () => new Set(this.className.split(" ").filter(Boolean));
    this.classList = {
      add: (...names) => (this.className = [...new Set([...classes(), ...names])].join(" ")),
      remove: (...names) => (this.className = [...classes()].filter((n) => !names.includes(n)).join(" ")),
      toggle: (name, on = !classes().has(name)) => {
        if (on) this.classList.add(name);
        else this.classList.remove(name);
        return Boolean(on);
      },
      contains: (name) => classes().has(name),
    };
  }
  append(...nodes) {
    for (const node of nodes) {
      if (node instanceof Node) node.parent = this;
      this.children.push(node);
    }
  }
  prepend(...nodes) {
    for (const node of nodes) if (node instanceof Node) node.parent = this;
    this.children.unshift(...nodes);
  }
  setAttribute(name, value) {
    this.attributes[name] = String(value);
  }
  getAttribute(name) {
    return this.attributes[name] ?? null;
  }
  /// Every handler is kept, run in the order added, as the DOM does.
  addEventListener(name, handler) {
    (this.listeners[name] ??= []).push(handler);
  }
  /// Runs its handlers of `name` with `event`.
  dispatch(name, event = {}) {
    for (const handler of this.listeners[name] ?? []) handler(event);
  }
  /// What a click on it runs: nothing while it is disabled, as a disabled
  /// button takes no click.
  click() {
    if (this.disabled) return;
    this.onclick?.();
    this.dispatch("click");
  }
  /// Its text and its children's, as textContent reads.
  get text() {
    return [this.textContent, ...this.children.map((c) => (c instanceof Node ? c.text : String(c)))].join("");
  }
  /// It and every node under it, in document order.
  *nodes() {
    yield this;
    for (const child of this.children) if (child instanceof Node) yield* child.nodes();
  }
  /// The first node under it, or it, matching `test`.
  find(test) {
    for (const node of this.nodes()) if (test(node)) return node;
    return null;
  }
  findAll(test) {
    return [...this.nodes()].filter(test);
  }
  /// The nodes under it matching a class (".name") or tag selector, the
  /// only ones the UI modules ask the fakes for; any other is an error
  /// rather than no match.
  querySelectorAll(selector) {
    const matches = /^\.([\w-]+)$/.test(selector)
      ? (node) => node.classList.contains(selector.slice(1))
      : /^[a-z]+$/.test(selector)
        ? (node) => node.tag === selector
        : null;
    if (!matches) throw new Error(`fake-dom does not match ${selector}`);
    return [...this.nodes()].slice(1).filter(matches);
  }
  querySelector(selector) {
    return this.querySelectorAll(selector)[0] ?? null;
  }
  /// The node under it with this data-back-field, as a card's back names its
  /// controls.
  field(name) {
    return this.find((node) => node.dataset.backField === name);
  }
}
const el = (tag, className, text) => new Node(tag, className, text);
const icon = (name) => Object.assign(new Node("svg", "icon"), { glyph: name });
function button(label, glyph, className, handler) {
  const node = el("button", className);
  node.glyph = glyph;
  if (glyph) node.append(icon(glyph));
  if (label) node.append(el("span", "", label));
  if (handler) node.addEventListener("click", handler);
  return node;
}
/// card-face.js over fake nodes, for a Codex card in English with nothing
/// under way and idle actions, which `extra` replaces (`host` is it).
function cardFace(extra = {}) {
  const tr = (key, ...args) => (args.length ? `${key}:${args.join(",")}` : key);
  const host = {
    el,
    button,
    icon,
    tr,
    locale: "en",
    pool: "codex",
    collapsed: false,
    resetDisplay: "exact",
    preferences: {},
    controller: { cardState: () => ({}), flipTarget: () => null, editing: null },
    switchingId: null,
    cardContext: () => ({ pool: host.pool, locale: host.locale, tr: host.tr, collapsed: host.collapsed }),
    providerTitle: () => "Team",
    accountProxy: () => "",
    cardBack: () => el("section", "account-back"),
    modelBrand: (model) => `${model} mark`,
    showResetPopover: () => {},
    toggleResetDisplay: () => {},
    getComputedStyle: () => ({}),
    reducedMotion: () => false,
    notice: () => {},
    signIn: () => {},
    editCard: () => {},
    refreshUsage: () => {},
    refreshProvider: () => {},
    greet: () => {},
    renewNow: () => {},
    switchAccount: () => {},
    confirmRemoval: () => {},
    ...extra,
  };
  const face = require("../ui/card-face.js").create(host);
  face.host = host;
  return face;
}
module.exports = { Node, el, icon, button, cardFace };
