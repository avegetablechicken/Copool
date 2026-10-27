//! The tray's macOS status item. tray-icon handles mouse clicks in a view over
//! its button, and accessibility clients press the button itself, which
//! toggles the panel.
//!
//! macOS 27 opens a status item's attached menu on any click, and only a
//! status item's expanded interface session joins keyboard navigation of the
//! menu bar (Control-F8). There the panel follows that session, as the system's
//! own items do: a left click or navigation begins it, and a click elsewhere,
//! another item or Escape ends it. Only a click focuses the panel, which would
//! otherwise end keyboard navigation. tray-icon releases before 0.25.1
//! keep the menu attached (see build.rs), so it is taken off the item and pops
//! up on a right click; an attached menu or a clicked button would begin a
//! session as well.
#[cfg(tray_icon_attached_menu)]
use block2::RcBlock;
use objc2::{
    define_class, msg_send, rc::Retained, runtime::AnyObject, runtime::NSObject,
    runtime::NSObjectProtocol, sel, DefinedClass, MainThreadMarker, MainThreadOnly,
};
#[cfg(tray_icon_attached_menu)]
use objc2::{extern_protocol, runtime::Bool};
use objc2_app_kit::NSStatusItem;
#[cfg(tray_icon_attached_menu)]
use objc2_app_kit::{NSEvent, NSEventMask, NSEventType, NSMenu};
#[cfg(tray_icon_attached_menu)]
use objc2_foundation::{NSPoint, NSProcessInfo};
use std::cell::RefCell;
#[cfg(tray_icon_attached_menu)]
use std::{cell::Cell, ptr::NonNull};
use tauri::{tray::TrayIcon, AppHandle};

pub struct Handlers {
    /// The button was pressed other than by the mouse.
    pub press: Box<dyn Fn() + Send>,
    /// The expanded interface session began: show the panel, focused when a
    /// click on the item began it.
    pub begin: Box<dyn Fn(bool) + Send>,
    /// The session ended: hide the panel.
    pub end: Box<dyn Fn() + Send>,
}

#[cfg(tray_icon_attached_menu)]
extern_protocol!(
    /// `NSStatusItemExpandedInterfaceDelegate` (macOS 27), not yet in objc2-app-kit.
    ///
    /// # Safety
    ///
    /// Implementors must implement the protocol's required methods.
    // The lint misreads the macro's expansion, not this documentation.
    #[allow(clippy::missing_safety_doc)]
    unsafe trait NSStatusItemExpandedInterfaceDelegate: NSObjectProtocol {}
);

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = Handlers]
    struct Delegate;

    unsafe impl NSObjectProtocol for Delegate {}

    #[cfg(tray_icon_attached_menu)]
    unsafe impl NSStatusItemExpandedInterfaceDelegate for Delegate {
        #[unsafe(method(statusItem:didBeginExpandedInterfaceSession:))]
        fn did_begin(&self, _item: &NSStatusItem, _session: &AnyObject) {
            let now = NSProcessInfo::processInfo().systemUptime();
            BEGAN.with(|began| began.set(now));
            let clicked = CLICKED.with(Cell::get) > now - CLICK_BEGINS;
            (self.ivars().begin)(clicked);
        }

        #[unsafe(method(statusItemDidEndExpandedInterfaceSession:animated:))]
        fn did_end(&self, _item: &NSStatusItem, _animated: Bool) {
            (self.ivars().end)();
        }
    }

    impl Delegate {
        #[unsafe(method(press:))]
        fn press(&self, _sender: Option<&AnyObject>) {
            (self.ivars().press)();
        }
    }
);

struct Installed {
    item: Retained<NSStatusItem>,
    /// The button and the item only reference it weakly.
    _delegate: Retained<Delegate>,
    /// The detached menu, when the panel follows sessions.
    #[cfg(tray_icon_attached_menu)]
    menu: Option<Retained<NSMenu>>,
    #[cfg(tray_icon_attached_menu)]
    _monitor: Option<Retained<AnyObject>>,
}

thread_local! {
    static INSTALLED: RefCell<Option<Installed>> = const { RefCell::new(None) };
    /// When the current session began, in system uptime like event timestamps.
    #[cfg(tray_icon_attached_menu)]
    static BEGAN: Cell<f64> = const { Cell::new(0.0) };
    /// When the item was last clicked without an open session.
    #[cfg(tray_icon_attached_menu)]
    static CLICKED: Cell<f64> = const { Cell::new(0.0) };
}

/// How soon after a click on the item a session counts as begun by it.
#[cfg(tray_icon_attached_menu)]
const CLICK_BEGINS: f64 = 0.5;

pub fn install(tray: &TrayIcon, handlers: Handlers) {
    let _ = tray.with_inner_tray_icon(move |inner| {
        let (Some(mtm), Some(item)) = (MainThreadMarker::new(), inner.ns_status_item()) else {
            return;
        };
        let Some(button) = item.button(mtm) else {
            return;
        };
        let delegate = mtm.alloc::<Delegate>().set_ivars(handlers);
        // SAFETY: NSObject's `init` takes no arguments.
        let delegate: Retained<Delegate> = unsafe { msg_send![super(delegate), init] };
        // SAFETY: `press:` takes the sender, and INSTALLED keeps the target alive.
        unsafe {
            button.setTarget(Some(&delegate));
            button.setAction(Some(sel!(press:)));
        }
        #[cfg(tray_icon_attached_menu)]
        let (menu, monitor) = if item.respondsToSelector(sel!(setExpandedInterfaceDelegate:)) {
            inner.set_show_menu_on_right_click(false);
            let menu = item.menu(mtm);
            item.setMenu(None);
            // SAFETY: the delegate implements the protocol, and INSTALLED keeps it alive.
            let () = unsafe { msg_send![&**item, setExpandedInterfaceDelegate: &*delegate] };
            (menu, monitor())
        } else {
            (None, None)
        };
        INSTALLED.with(|cell| {
            *cell.borrow_mut() = Some(Installed {
                item: item.clone(),
                _delegate: delegate,
                #[cfg(tray_icon_attached_menu)]
                menu,
                #[cfg(tray_icon_attached_menu)]
                _monitor: monitor,
            })
        });
    });
}

/// Pops the detached menu up under the button.
#[cfg(tray_icon_attached_menu)]
pub fn show_menu() {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let Some((menu, button)) = INSTALLED.with(|cell| {
        let installed = cell.borrow();
        let installed = installed.as_ref()?;
        Some((installed.menu.clone()?, installed.item.button(mtm)?))
    }) else {
        return;
    };
    let below = NSPoint::new(0.0, button.bounds().size.height + 5.0);
    button.highlight(true);
    menu.popUpMenuPositioningItem_atLocation_inView(None, below, Some(&button));
    button.highlight(false);
}

/// Ends the session after the panel was hidden some other way.
pub fn end_session(app: &AppHandle) {
    let _ = app.run_on_main_thread(|| {
        INSTALLED.with(|cell| {
            if let Some(session) = cell.borrow().as_ref().and_then(|i| session(&i.item)) {
                // SAFETY: `cancel` takes no arguments.
                let () = unsafe { msg_send![&*session, cancel] };
            }
        })
    });
}

fn session(item: &NSStatusItem) -> Option<Retained<AnyObject>> {
    if !item.respondsToSelector(sel!(expandedInterfaceSession)) {
        return None;
    }
    // SAFETY: the item responds to the getter, which returns an object or nil.
    unsafe { msg_send![item, expandedInterfaceSession] }
}

/// A click outside the app ends an open session, as for a menu; that includes
/// the item, whose own process never sees the click. The system reports the
/// mouse down that begins a session before beginning it, so a later session is
/// this click's.
#[cfg(tray_icon_attached_menu)]
fn monitor() -> Option<Retained<AnyObject>> {
    let handler = RcBlock::new(|event: NonNull<NSEvent>| {
        // SAFETY: AppKit passes a valid event.
        let event = unsafe { event.as_ref() };
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let point = NSEvent::mouseLocation();
        let Some((on_item, session)) = INSTALLED.with(|cell| {
            let installed = cell.borrow();
            let item = &installed.as_ref()?.item;
            let frame = item.button(mtm)?.window()?.frame();
            let on_item = point.x >= frame.origin.x
                && point.x <= frame.origin.x + frame.size.width
                && point.y >= frame.origin.y
                && point.y <= frame.origin.y + frame.size.height;
            Some((on_item, session(item)))
        }) else {
            return;
        };
        match session.filter(|_| BEGAN.with(Cell::get) < event.timestamp()) {
            // SAFETY: `cancel` takes no arguments.
            Some(session) => unsafe { msg_send![&*session, cancel] },
            None if on_item && event.r#type() == NSEventType::LeftMouseDown => {
                CLICKED.with(|clicked| clicked.set(event.timestamp()))
            }
            None => {}
        }
    });
    let mask =
        NSEventMask::LeftMouseDown | NSEventMask::RightMouseDown | NSEventMask::OtherMouseDown;
    NSEvent::addGlobalMonitorForEventsMatchingMask_handler(mask, &handler)
}
