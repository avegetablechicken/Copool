/* In-panel menus, as Coport's: they keep focus in the panel window, which a
   native menu would take away. One menu is open at a time, with at most one
   submenu, and a reset popover shares their dismissal. Used by the UI and
   node:test, which gives `create` a fake document and `el`. */
(function (root, factory) {
  const api = factory();
  if (typeof module === "object" && module.exports) module.exports = api;
  else root.CopoolMenus = api;
})(globalThis, function () {
  /// Parent and child share one navigation mode and one highlighted menu.
  /// Hover must not leave a previously focused row painted in either menu.
  function configureMenuNavigation(menu, group = [menu]) {
    const activate = () => {
      for (const member of group) {
        if (member === menu) member.classList.remove("inactive-menu");
        else member.classList.add("inactive-menu");
      }
    };
    const pointer = () => {
      for (const member of group) member.classList.add("pointer-navigation");
      activate();
    };
    menu.addEventListener("pointerover", pointer);
    menu.addEventListener("pointermove", pointer);
    menu.addEventListener("focusin", activate);
    menu.addEventListener("keydown", () => {
      for (const member of group) member.classList.remove("pointer-navigation");
      activate();
    });
  }
  /// Keeps the parent aligned with its button, fitting the menu pair into
  /// the space to its right instead of shifting the parent off its anchor.
  function menuPairLayout(panel, parentWidth, submenuWidth, preferredLeft) {
    const left = Math.max(panel.left + 8, Math.min(preferredLeft, panel.right - 8));
    const available = Math.max(0, panel.right - 8 - left);
    const child = Math.floor(Math.min(submenuWidth, available * 0.45));
    const parent = Math.floor(Math.min(parentWidth, available - child + 1));
    return { left, parentWidth: parent, submenuWidth: child };
  }
  /// The import menu's: both pools share the toolbar anchor and parent
  /// width, reserving only a maximum for the child, whose actual width
  /// follows its provider names.
  function importMenuLayout(panel, preferredLeft) {
    return menuPairLayout(panel, 200, 150, preferredLeft);
  }
  /// Preserves the menu's measured width, moving it inward instead of
  /// squeezing it.
  function menuLeft(panel, width, preferredLeft) {
    return Math.max(panel.left + 8, Math.min(preferredLeft, panel.right - width - 8));
  }
  /// The panel's menus in `document`, built with `el(tag, className, text)`;
  /// `timers` ({ setTimeout, clearTimeout }) for tests.
  function create({ document, el, timers = globalThis }) {
    let openMenu = null;
    let openSubmenu = null;
    let submenuDismissTimer = null;
    const panelBounds = () => document.querySelector(".panel").getBoundingClientRect();
    function cancelSubmenuDismiss() {
      if (submenuDismissTimer !== null) timers.clearTimeout(submenuDismissTimer);
      submenuDismissTimer = null;
    }
    /// Crossing another row toward the open submenu gives the pointer time
    /// to reach it before the submenu closes.
    function deferSubmenuDismiss() {
      if (submenuDismissTimer !== null || !openSubmenu) return;
      const pending = openSubmenu;
      submenuDismissTimer = timers.setTimeout(() => {
        submenuDismissTimer = null;
        if (openSubmenu === pending) closeSubmenu();
      }, 350);
    }
    /// Entering a menu or returning from its child focuses the row without
    /// opening the child until the user navigates to it.
    function focusMenuRow(row) {
      if (!row) return;
      const onfocus = row.onfocus;
      row.onfocus = null;
      row.focus();
      row.onfocus = onfocus;
    }
    function closeSubmenu(restoreFocus = false) {
      cancelSubmenuDismiss();
      if (!openSubmenu) return;
      const { menu, trigger } = openSubmenu;
      openSubmenu = null;
      menu.remove();
      trigger.setAttribute("aria-expanded", "false");
      if (restoreFocus) focusMenuRow(trigger);
    }
    function closeMenu(restoreFocus = false) {
      closeSubmenu();
      if (!openMenu) return;
      const { menu, trigger, at } = openMenu;
      openMenu = null;
      menu.remove();
      if (at) return;
      trigger.setAttribute("aria-expanded", "false");
      if (restoreFocus) trigger.focus();
    }
    /// Lines in a popover under `trigger`, or closes it when it is open.
    function showResetPopover(trigger, lines, wrapping = false) {
      if (openMenu?.trigger === trigger) {
        closeMenu(true);
        return;
      }
      closeMenu();
      const menu = el("div", `select-menu reset-popover${wrapping ? " wrapping" : ""}`);
      menu.setAttribute("role", "dialog");
      menu.setAttribute("aria-label", trigger.getAttribute("aria-label"));
      menu.tabIndex = -1;
      for (const line of lines) menu.append(el("div", "", line));
      document.body.append(menu);
      const bounds = trigger.getBoundingClientRect();
      const panel = panelBounds();
      menu.style.maxWidth = `${Math.max(0, panel.right - panel.left - 16)}px`;
      const below = bounds.bottom + 4;
      const top = below + menu.offsetHeight <= panel.bottom - 8
        ? below : Math.max(panel.top + 8, bounds.top - menu.offsetHeight - 4);
      menu.style.left = `${menuLeft(panel, menu.offsetWidth, bounds.left)}px`;
      menu.style.top = `${top}px`;
      menu.style.maxHeight = `${panel.bottom - top - 8}px`;
      trigger.setAttribute("aria-expanded", "true");
      // Share outside-click, Escape, page-change and panel-hide dismissal.
      openMenu = { menu, trigger };
      menu.focus();
    }
    /// Shows a menu below `trigger`, or at the point `at` for a context
    /// menu. Items may be separators, and the destructive one is red.
    function showMenu(trigger, items, at = null) {
      closeMenu();
      const menu = el("div", "select-menu action-menu");
      const submenus = [];
      const navigationGroup = [menu];
      configureMenuNavigation(menu, navigationGroup);
      if (trigger.id === "import-open") menu.classList.add("import-menu");
      menu.setAttribute("role", "menu");
      // Reserve the checkmark gutter only when an item is actually checked.
      const choices = items.some((item) => item.selected || item.checked);
      menu.classList.toggle("choices", choices);
      for (const item of items) {
        if (item.separator) {
          const line = el("div", "menu-separator");
          line.setAttribute("role", "separator");
          menu.append(line);
          continue;
        }
        const option = el("button", item.destructive ? "destructive" : "", item.label);
        if (item.icon) option.prepend(item.icon);
        option.type = "button";
        option.disabled = !!item.disabled;
        option.setAttribute("role", "selected" in item ? "menuitemradio" : "menuitem");
        if ("selected" in item) option.setAttribute("aria-checked", item.selected ? "true" : "false");
        if ("checked" in item) {
          option.setAttribute("role", "menuitemcheckbox");
          option.setAttribute("aria-checked", String(item.checked));
        }
        menu.append(option);
        if (item.children) {
          const submenu = el("div", "select-menu action-menu provider-submenu");
          navigationGroup.push(submenu);
          configureMenuNavigation(submenu, navigationGroup);
          if (trigger.id === "import-open") submenu.classList.add("import-menu");
          submenu.addEventListener("pointerenter", cancelSubmenuDismiss);
          submenu.addEventListener("focusin", cancelSubmenuDismiss);
          submenu.setAttribute("role", "menu");
          submenu.setAttribute("aria-label", item.label);
          option.setAttribute("aria-haspopup", "menu");
          option.setAttribute("aria-expanded", "false");
          const expand = () => {
            cancelSubmenuDismiss();
            if (openSubmenu?.trigger === option) return;
            closeSubmenu();
            document.body.append(submenu);
            const bounds = option.getBoundingClientRect();
            const parent = menu.getBoundingClientRect();
            const panel = panelBounds();
            const right = parent.right - 1;
            const left = right + submenu.offsetWidth <= panel.right - 8
              ? right
              : parent.left - submenu.offsetWidth + 1;
            // Align to the parent row in both pools; long lists scroll below it.
            const top = Math.max(panel.top + 8,
              Math.min(bounds.top - 4, panel.bottom - 32));
            submenu.style.left = `${menuLeft(panel, submenu.offsetWidth, left)}px`;
            submenu.style.top = `${top}px`;
            submenu.style.maxHeight = `${panel.bottom - top - 8}px`;
            openSubmenu = { menu: submenu, trigger: option };
            option.setAttribute("aria-expanded", "true");
          };
          option.onpointerenter = expand;
          option.onfocus = expand;
          option.onclick = () => {
            expand();
            submenu.querySelector("button:not(:disabled)")?.focus();
          };
          option.onkeydown = (event) => {
            if (event.key !== "ArrowRight") return;
            event.preventDefault();
            option.onclick();
          };
          for (const child of item.children) {
            const button = el("button", "", child.label);
            if (child.icon) button.prepend(child.icon);
            button.type = "button";
            button.setAttribute("role", "menuitem");
            button.disabled = !!child.disabled;
            button.onclick = () => {
              closeMenu(true);
              child.run();
            };
            submenu.append(button);
          }
          submenus.push(submenu);
          submenu.onkeydown = (event) => {
            if (!["ArrowLeft", "Escape"].includes(event.key)) return;
            event.preventDefault();
            event.stopPropagation();
            closeSubmenu(true);
          };
        } else {
          option.onclick = () => {
            closeMenu(true);
            item.run();
          };
        }
      }
      // Padding is the bridge between the two menus, not another choice.
      // Give diagonal pointer travel across adjacent rows time to reach the child.
      menu.addEventListener("pointerover", (event) => {
        const row = event.target.closest("button");
        if (!openSubmenu) return;
        if (!row || row === openSubmenu.trigger) cancelSubmenuDismiss();
        else deferSubmenuDismiss();
      });
      menu.addEventListener("focusin", (event) => {
        const row = event.target.closest("button");
        if (openSubmenu && row && row !== openSubmenu.trigger) closeSubmenu();
      });
      document.body.append(menu);
      // Below the trigger, or above it when the panel is too short; kept inside.
      const bounds = at
        ? { left: at.x, right: at.x, top: at.y, bottom: at.y }
        : trigger.getBoundingClientRect();
      const panel = panelBounds();
      // Freeze the intrinsic width before assigning left: positioning must not
      // change the measured width (especially for cards along the right edge).
      menu.style.maxWidth = `${Math.max(0, panel.right - panel.left - 16)}px`;
      menu.style.width = `${menu.offsetWidth}px`;
      let left = menuLeft(panel, menu.offsetWidth, bounds.left);
      if (submenus.length && trigger.id === "import-open") {
        const layout = importMenuLayout(panel, bounds.left);
        menu.style.minWidth = "0";
        menu.style.width = `${layout.parentWidth}px`;
        for (const submenu of submenus) {
          submenu.style.minWidth = "0";
          submenu.style.maxWidth = `${layout.submenuWidth}px`;
        }
        left = layout.left;
      }
      // Other card menus keep their normal width; expand() opens the child to
      // the left when the trigger is near the panel's right edge.
      const below = bounds.bottom + 4;
      const top =
        below + menu.offsetHeight <= panel.bottom - 8
          ? below
          : Math.max(panel.top + 8, bounds.top - menu.offsetHeight - 4);
      menu.style.left = `${Math.max(8, left)}px`;
      menu.style.top = `${top}px`;
      menu.style.maxHeight = `${panel.bottom - top - 8}px`;
      if (!at) trigger.setAttribute("aria-expanded", "true");
      openMenu = { menu, trigger, at };
      focusMenuRow(
        menu.querySelector("[aria-checked=true]") || menu.querySelector("button:not(:disabled)"),
      );
    }
    /// The panel's keys while a menu is open: Escape and Tab close it, the
    /// arrows, Home and End move along its rows. False when none is open.
    function handleKey(event) {
      if (!openMenu) return false;
      const activeMenu = openSubmenu?.menu.contains(document.activeElement)
        ? openSubmenu.menu : openMenu.menu;
      const items = [...activeMenu.querySelectorAll("button")].filter(
        (item) => !item.disabled && !item.closest("[hidden]"),
      );
      const index = items.indexOf(document.activeElement);
      if (event.key === "Escape" || event.key === "Tab") {
        event.preventDefault();
        closeMenu(true);
      } else if (["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)) {
        event.preventDefault();
        const next =
          event.key === "Home"
            ? 0
            : event.key === "End"
              ? items.length - 1
              : (index + (event.key === "ArrowDown" ? 1 : -1) + items.length) %
                items.length;
        items[next]?.focus();
      }
      return true;
    }
    /// A press outside the open menu, its submenu and its trigger closes it.
    function dismissOutside(target) {
      if (
        openMenu &&
        !openMenu.menu.contains(target) &&
        !openSubmenu?.menu.contains(target) &&
        (openMenu.at || !openMenu.trigger.contains(target))
      )
        closeMenu();
    }
    return {
      show: showMenu,
      close: closeMenu,
      showResetPopover,
      handleKey,
      dismissOutside,
      /// Whether a menu is open, and whether `trigger`'s is.
      get isOpen() {
        return openMenu !== null;
      },
      isOpenFor: (trigger) => openMenu?.trigger === trigger,
    };
  }
  return { create, configureMenuNavigation, menuPairLayout, menuLeft };
});
