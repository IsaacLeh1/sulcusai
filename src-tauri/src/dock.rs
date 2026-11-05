// SPDX-License-Identifier: AGPL-3.0-only
//! The Dock icon's right-click entries on macOS (Pause completely, Resume,
//! Quit completely), the counterpart of the Windows jump list.
//!
//! Tauri's app delegate has no dock-menu hook, so `applicationDockMenu:` is
//! added to the delegate's class at run time and hands AppKit a menu kept
//! here.

use std::cell::RefCell;
use std::sync::OnceLock;

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Imp, Sel};
use objc2::{define_class, msg_send, sel, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{NSApplication, NSMenu, NSMenuItem};
use objc2_foundation::{NSObject, NSString};

use crate::instance::Ask;

type Pick = Box<dyn Fn(Ask) + Send + Sync>;

static PICK: OnceLock<Pick> = OnceLock::new();

thread_local! {
    static MENU: RefCell<Option<Retained<NSMenu>>> = const { RefCell::new(None) };
    static TARGET: RefCell<Option<Retained<Target>>> = const { RefCell::new(None) };
    static ADDED: RefCell<bool> = const { RefCell::new(false) };
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "SulcusAIDockTarget"]
    struct Target;

    impl Target {
        #[unsafe(method(pick:))]
        fn pick(&self, item: &NSMenuItem) {
            if let (Some(f), Some(ask)) = (PICK.get(), Ask::ALL.get(item.tag() as usize)) {
                f(*ask);
            }
        }
    }
);

unsafe extern "C-unwind" fn dock_menu(_this: *mut AnyObject, _sel: Sel, _app: *mut AnyObject) -> *mut NSMenu {
    MENU.with(|m| m.borrow().as_ref().map_or(std::ptr::null_mut(), |m| Retained::as_ptr(m).cast_mut()))
}

/// Sets the entries (`on`) or clears them. Must run on the main thread,
/// after the app delegate is in place; `pick` gets the chosen entry.
pub fn set(on: bool, titles: [&str; 3], pick: impl Fn(Ask) + Send + Sync + 'static) -> Result<(), String> {
    let mtm = MainThreadMarker::new().ok_or("The Dock menu can only be set on the main thread.")?;
    let _ = PICK.set(Box::new(pick));
    if !on {
        MENU.with(|m| *m.borrow_mut() = None);
        return Ok(());
    }
    let app = NSApplication::sharedApplication(mtm);
    let delegate = app.delegate().ok_or("The app has no delegate yet.")?;
    let added = ADDED.with(|a| *a.borrow());
    if !added {
        let delegate: &AnyObject = delegate.as_ref();
        let class: &AnyClass = delegate.class();
        // SAFETY: the method's types ("@@:@": object return, self, _cmd, one
        // object) match `dock_menu`; adding fails harmlessly if it exists.
        unsafe {
            let imp: Imp = std::mem::transmute::<unsafe extern "C-unwind" fn(*mut AnyObject, Sel, *mut AnyObject) -> *mut NSMenu, Imp>(dock_menu);
            objc2::ffi::class_addMethod((class as *const AnyClass).cast_mut(), sel!(applicationDockMenu:), imp, c"@@:@".as_ptr());
        }
        ADDED.with(|a| *a.borrow_mut() = true);
    }
    let target: Retained<Target> = TARGET.with(|t| {
        t.borrow_mut()
            // SAFETY: plain NSObject init of a class with no ivars.
            .get_or_insert_with(|| unsafe { msg_send![Target::alloc(mtm), init] })
            .clone()
    });
    let menu = NSMenu::new(mtm);
    for (i, title) in titles.iter().enumerate() {
        // SAFETY: the action matches `Target::pick`, and the target lives in
        // TARGET for as long as the app runs.
        unsafe {
            let item = NSMenuItem::initWithTitle_action_keyEquivalent(NSMenuItem::alloc(mtm), &NSString::from_str(title), Some(sel!(pick:)), &NSString::from_str(""));
            item.setTag(i as isize);
            item.setTarget(Some(&target));
            menu.addItem(&item);
        }
    }
    MENU.with(|m| *m.borrow_mut() = Some(menu));
    Ok(())
}
