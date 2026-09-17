//! The tree being built. Builders ([`crate::vocab`]) and [`crate::El`]
//! modifiers reach it through here; it is set only for the duration of a
//! [`crate::Ui::rebuild`].
//!
//! On the target this is one global: rebuilding two `Ui`s concurrently (from
//! two cores) is refused loudly. With the `std` feature it is thread-local.

use crate::tree::Tree;

/// Type-erased `*mut Tree<'_>`.
type Ptr = *mut ();

#[cfg(feature = "std")]
mod slot {
    extern crate std;
    use super::Ptr;
    std::thread_local! {
        static CURRENT: core::cell::Cell<Ptr> = const { core::cell::Cell::new(core::ptr::null_mut()) };
    }
    pub fn swap(new: Ptr) -> Ptr {
        CURRENT.with(|c| c.replace(new))
    }
    pub fn claim(new: Ptr) -> bool {
        CURRENT.with(|c| {
            if c.get().is_null() {
                c.set(new);
                true
            } else {
                false
            }
        })
    }
}

#[cfg(not(feature = "std"))]
mod slot {
    use core::sync::atomic::{AtomicPtr, Ordering};

    use super::Ptr;
    static CURRENT: AtomicPtr<()> = AtomicPtr::new(core::ptr::null_mut());
    pub fn swap(new: Ptr) -> Ptr {
        CURRENT.swap(new, Ordering::AcqRel)
    }
    pub fn claim(new: Ptr) -> bool {
        CURRENT
            .compare_exchange(core::ptr::null_mut(), new, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }
}

/// Make `tree` current while `f` runs.
pub(crate) fn scope<R>(tree: &mut Tree<'_>, f: impl FnOnce() -> R) -> R {
    struct Clear;
    impl Drop for Clear {
        fn drop(&mut self) {
            slot::swap(core::ptr::null_mut());
        }
    }
    assert!(
        slot::claim(tree as *mut Tree<'_> as Ptr),
        "scanwright: Ui::rebuild is already running (nested, or on another core)"
    );
    let _clear = Clear;
    f()
}

/// Borrow the current tree for the duration of `f`. `f` must not build
/// elements (it never needs to: children are built before their parent).
pub(crate) fn with_tree<R>(f: impl FnOnce(&mut Tree<'_>) -> R) -> R {
    // Take the pointer out while it is borrowed, so a re-entrant call fails
    // loudly instead of aliasing the `&mut`.
    let ptr = slot::swap(core::ptr::null_mut());
    assert!(!ptr.is_null(), "scanwright: elements can only be built inside Ui::rebuild");
    struct Restore(Ptr);
    impl Drop for Restore {
        fn drop(&mut self) {
            slot::swap(self.0);
        }
    }
    let _restore = Restore(ptr);
    // Safety: `scope` stored a pointer to a live `&mut Tree` that it does not
    // touch while `f` (the build closure) runs, and the swap above guarantees
    // this is the only reference derived from it right now.
    let tree = unsafe { &mut *(ptr as *mut Tree<'_>) };
    f(tree)
}
