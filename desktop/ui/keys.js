/* The panel's keys: the menu bar's ←/→, Escape back to the accounts page,
   turning an edited card back or hiding the panel, Return turning the
   hovered card over, and the ⌘ shortcuts. An open menu has its own keys
   (menus.js). Used by the UI and node:test.

   `create(host)` takes what the keys act on, read when used:
     document               the page
     mac                    whether ⌘ (else Ctrl) is the modifier
     menus                  menus.js's menus
     page, pool, collapsed, reordering   what the panel shows
     controller             the shown pool's Accounts controller
     menuBarNavigation      the panel was opened from the menu bar's keys
     invoke(command, args)  the core's commands
     showPage, showPool, editCard, cancelEditing, quit, click(id)   app.js's */
(function (root, factory) {
  const node = typeof module === "object" && module.exports;
  const api = node
    ? factory(require("./presentation.js"), require("./card-view.js"))
    : factory(root.CopoolPresentation, root.CopoolCardView);
  if (node) module.exports = api;
  else root.CopoolKeys = api;
})(globalThis, function (P, V) {
  // Where ←/→ move the caret.
  const TEXT_ENTRY =
    'textarea, [contenteditable]:not([contenteditable="false"]), input:not([type="checkbox"]):not([type="radio"]):not([type="button"]):not([type="submit"]):not([type="reset"])';
  // Interactive controls that own Return when focused (e.g. via Tab).
  const INTERACTIVE_ELEMENT =
    'a[href], button, input, select, textarea, summary, [contenteditable]:not([contenteditable="false"]), [role="button"], [role="link"], [role="menuitem"], [role="tab"], [role="switch"], [role="checkbox"], [role="radio"], [role="combobox"], [role="option"], [role="textbox"], [tabindex]:not([tabindex="-1"])';
  // Toolbar shortcuts: Cmd (Ctrl elsewhere) with these keys.
  const TOOLBAR_SHORTCUTS = { i: "import-open", n: "add-account", r: "refresh-all" };
  function create(host) {
    const { document } = host;
    /// Whether Return should act on a focused UI element rather than a hovered card.
    function focusOwnsEnter(event) {
      if (event.target?.closest?.(INTERACTIVE_ELEMENT)) return true;
      const active = document.activeElement;
      return Boolean(
        active &&
          active !== document.body &&
          active !== document.documentElement &&
          active.closest?.(INTERACTIVE_ELEMENT),
      );
    }
    /// The hovered account card (or provider group) that Return can turn over to
    /// edit, or null when none is hovered, collapsed, already turned over, or has
    /// no settings side.
    function hoveredEditableCard() {
      if (host.page !== "accounts" || host.collapsed) return null;
      if (host.reordering) return null;
      const list = document.getElementById("accounts");
      if (!list) return null;
      const hovered = [...list.querySelectorAll("[data-flip-key]:is(:hover, .hover-hold)")];
      // Innermost first: a grouped account with its own back wins over its group,
      // while one without a back falls through to its group (as contextmenu does).
      for (let i = hovered.length - 1; i >= 0; i--) {
        const node = hovered[i];
        const key = node.dataset.flipKey;
        if (node.classList?.contains("account-back") || host.controller.isEditing(key)) {
          return null;
        }
        if (V.hasBack(host.controller.flipTarget(key), host.pool)) return key;
      }
      return null;
    }
    /// Coport's panel shortcuts. An open dialog keeps Escape and Return for
    /// cancel and submit, as the SwiftUI dialogs did.
    function onKeydown(event) {
      if (document.querySelector("dialog[open]")) return;
      if (host.menus.handleKey(event)) return;
      const step = P.menuBarStep(event.key, {
        navigating: host.menuBarNavigation,
        modified: event.metaKey || event.ctrlKey || event.altKey || event.shiftKey,
        inText: Boolean(event.target?.closest?.(TEXT_ENTRY)),
        handled: event.defaultPrevented || event.isComposing,
      });
      if (step) {
        event.preventDefault();
        host.invoke("navigate_menu_bar", { right: step === "right" }).catch(() => {});
        return;
      }
      const mac = host.mac;
      const mod = event.metaKey || event.ctrlKey;
      const back =
        !event.shiftKey &&
        (mac
          ? event.metaKey && !event.ctrlKey && !event.altKey && event.key === "["
          : event.altKey && !event.ctrlKey && !event.metaKey && event.key === "ArrowLeft");
      if (event.key === "Escape") {
        if (event.defaultPrevented || event.isComposing || event.keyCode === 229) return;
        event.preventDefault();
        // Holding Escape through a card flip must not dismiss the panel too.
        if (event.repeat) return;
        // Back from a subpage first; on the accounts page, turn an edited card
        // back without saving, else dismiss like a menu.
        if (host.page !== "accounts") host.showPage("accounts");
        else if (host.controller.editing || host.controller.turning) host.cancelEditing();
        else host.invoke("hide_panel").catch(() => {});
      } else if (
        event.key === "Enter" &&
        !event.defaultPrevented &&
        !event.isComposing &&
        event.keyCode !== 229 &&
        !event.repeat &&
        !event.metaKey &&
        !event.ctrlKey &&
        !event.altKey &&
        !event.shiftKey &&
        !focusOwnsEnter(event)
      ) {
        const key = hoveredEditableCard();
        if (key) {
          event.preventDefault();
          host.editCard(key);
        }
      } else if (back) {
        event.preventDefault();
        if (host.page !== "accounts") host.showPage("accounts");
      } else if (mod && event.key === "q") {
        event.preventDefault();
        host.quit();
      } else if (mod && !event.altKey && !event.shiftKey && ["1", "2", "3", "4"].includes(event.key)) {
        event.preventDefault();
        // The pool is shared, so the proxy page switches in place; Grok's
        // accounts have no proxy page.
        const shortcut = P.poolShortcut(host.page, event.key);
        if (shortcut.page) host.showPage(shortcut.page);
        host.showPool(shortcut.pool);
      } else if (mod && event.key === ",") {
        event.preventDefault();
        host.showPage("settings");
      } else if (mod && !event.altKey && !event.shiftKey && Object.hasOwn(TOOLBAR_SHORTCUTS, event.key)) {
        // Always handled, so Cmd+R never reloads the page; a disabled button
        // ignores its shortcut as it ignores clicks.
        event.preventDefault();
        if (host.page !== "accounts") host.showPage("accounts");
        host.click(TOOLBAR_SHORTCUTS[event.key]);
      }
    }
    return { onKeydown };
  }
  return { create, TEXT_ENTRY };
});
