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
      toggle: (name, on = !classes().has(name)) =>
        on ? this.classList.add(name) : this.classList.remove(name),
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
  addEventListener(name, handler) {
    this.listeners[name] = handler;
  }
  /// What a click on it runs.
  click() {
    this.onclick?.();
    this.listeners.click?.();
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
module.exports = { Node, el, icon, button };
