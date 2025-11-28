use core::{
    cell::UnsafeCell,
    marker::PhantomData,
    mem::ManuallyDrop,
    ops::{Deref, DerefMut},
    ptr::NonNull,
    sync::atomic::{AtomicIsize, Ordering},
};

use crate::error::SvsmError;

/// Errors related to reentrancy.
#[derive(Debug, Clone, Copy)]
pub enum ReentrancyError {
    /// Attempted to perform a reentrant read while a write was being performed
    ReentrantRead,
    /// Attempted to perform a reentrant write while a write was being performed
    ReentrantWrite,
}

impl From<ReentrancyError> for SvsmError {
    fn from(value: ReentrancyError) -> Self {
        Self::Reentrancy(value)
    }
}

/// A reentrancy-safe version of [`RefCell`](core::cell::RefCell).
/// The type tolerates reentrancy within the same CPU at any point
/// while guaranteeing memory safety.
///
/// NOTE! The type is **not** thread-safe.
///
/// The type allows either multiple readers or a single writer, but
/// not both, just like `RefCell`.
#[derive(Debug)]
pub struct PerCpuCell<T> {
    value: UnsafeCell<T>,
    borrow: AtomicIsize,
}

impl<T> PerCpuCell<T> {
    /// Create a new `PerCpuCell` with the given value.
    pub const fn new(value: T) -> Self {
        Self {
            value: UnsafeCell::new(value),
            borrow: AtomicIsize::new(0),
        }
    }

    /// Returns a raw pointer to the underlying data in this cell.
    pub fn as_ptr(&self) -> *mut T {
        self.value.get()
    }

    /// A reentrancy-safe version of
    /// [`RefCell::borrow()`](core::cell::RefCell::borrow).
    pub fn borrow(&self) -> PerCpuRef<'_, T> {
        self.try_borrow().unwrap()
    }

    /// A reentrancy-safe version of
    /// [`RefCell::try_borrow()`](core::cell::RefCell::try_borrow).
    pub fn try_borrow(&self) -> Result<PerCpuRef<'_, T>, SvsmError> {
        PerCpuRef::new(self)
            .ok_or(ReentrancyError::ReentrantRead)
            .map_err(Into::into)
    }

    /// A reentrancy-safe version of
    /// [`RefCell::borrow_mut()`](core::cell::RefCell::borrow_mut).
    pub fn borrow_mut(&self) -> PerCpuRefMut<'_, T> {
        self.try_borrow_mut().unwrap()
    }

    /// A reentrancy-safe version of
    /// [`RefCell::try_borrow_mut()`](core::cell::RefCell::try_borrow_mut).
    pub fn try_borrow_mut(&self) -> Result<PerCpuRefMut<'_, T>, SvsmError> {
        PerCpuRefMut::new(self)
            .ok_or(ReentrancyError::ReentrantWrite)
            .map_err(Into::into)
    }

    /// A reentrancy-safe version of
    /// [`RefCell::replace()`](core::cell::RefCell::replace).
    pub fn replace(&self, val: T) -> T {
        // Unlike `RefCell::replace`, we need to take a guard for reentrancy safety.
        let mut guard = self.borrow_mut();
        core::mem::replace(&mut guard, val)
    }

    /// A shorthand to copy out the value of the cell without calling
    /// [`borrow()`](Self::borrow).
    pub fn get(&self) -> T
    where
        T: Copy,
    {
        *self.borrow()
    }

    /// A shorthand to copy out the value of the cell without calling
    /// [`try_borrow()`](Self::try_borrow).
    pub fn try_get(&self) -> Result<T, SvsmError>
    where
        T: Copy,
    {
        Ok(*self.try_borrow()?)
    }
}

/// A reentrancy-safe version of [`Ref`](core::cell::Ref).
#[derive(Debug)]
pub struct PerCpuRef<'a, T> {
    borrow: &'a AtomicIsize,
    ptr: NonNull<T>,
}

impl<'a, T> PerCpuRef<'a, T> {
    fn new(cell: &'a PerCpuCell<T>) -> Option<Self> {
        let borrow = &cell.borrow;

        // There must be no writers and zero or more readers
        let val = borrow.load(Ordering::Relaxed);
        if val < 0 {
            return None;
        }
        borrow.store(val + 1, Ordering::Relaxed);

        // SAFETY: PerCpuCell is always initialized with a non-null value
        // inside the UnsafeCell.
        let ptr = unsafe { NonNull::new_unchecked(cell.value.get()) };
        Some(Self { borrow, ptr })
    }

    /// A reentrancy-safe version of [`Ref::map()`](core::cell::Ref::map).
    pub fn map<U, F>(orig: Self, f: F) -> PerCpuRef<'a, U>
    where
        F: FnOnce(&T) -> &U,
    {
        let orig = ManuallyDrop::new(orig);
        let new = f(&*orig);
        PerCpuRef {
            ptr: NonNull::from(new),
            borrow: orig.borrow,
        }
    }

    /// A reentrancy-safe version of [`Ref::filter_map()`](core::cell::Ref::filter_map).
    pub fn filter_map<U, F>(orig: Self, f: F) -> Result<PerCpuRef<'a, U>, Self>
    where
        F: FnOnce(&T) -> Option<&U>,
    {
        let orig = ManuallyDrop::new(orig);
        match f(&*orig) {
            Some(new) => Ok(PerCpuRef {
                ptr: NonNull::from(new),
                borrow: orig.borrow,
            }),
            None => Err(ManuallyDrop::into_inner(orig)),
        }
    }
}

impl<T> Drop for PerCpuRef<'_, T> {
    fn drop(&mut self) {
        self.borrow.fetch_sub(1, Ordering::Relaxed);
    }
}

impl<T> Deref for PerCpuRef<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        // SAFETY: the pointer is valid by construction. This type can
        // only exist if there are only readers of the pointer, so we
        // cannot violate Rust's memory model.
        unsafe { self.ptr.as_ref() }
    }
}

/// A reentrancy-safe version of [`RefMut`](core::cell::RefMut).
#[derive(Debug)]
pub struct PerCpuRefMut<'a, T> {
    borrow: &'a AtomicIsize,
    ptr: NonNull<T>,
    _phantom: PhantomData<&'a mut T>,
}

impl<'a, T> PerCpuRefMut<'a, T> {
    fn new(cell: &'a PerCpuCell<T>) -> Option<Self> {
        let borrow = &cell.borrow;

        // There must be exactly zero readers and writers
        let val = borrow.load(Ordering::Relaxed);
        if val != 0 {
            return None;
        }
        borrow.store(val - 1, Ordering::Relaxed);

        // SAFETY: PerCpuCell is always initialized with a non-null value
        // inside the UnsafeCell.
        let ptr = unsafe { NonNull::new_unchecked(cell.value.get()) };
        Some(Self {
            borrow,
            ptr,
            _phantom: PhantomData,
        })
    }

    /// A reentrancy-safe version of [`RefMut::map()`](core::cell::RefMut::map).
    pub fn map<U, F>(orig: Self, f: F) -> PerCpuRefMut<'a, U>
    where
        F: FnOnce(&mut T) -> &mut U,
    {
        // Do not run drop() on `orig`
        let mut orig = ManuallyDrop::new(orig);
        let new = f(&mut *orig);
        PerCpuRefMut {
            ptr: NonNull::from(new),
            borrow: orig.borrow,
            _phantom: PhantomData,
        }
    }

    /// A reentrancy-safe version of [`RefMut::filter_map()`](core::cell::RefMut::filter_map).
    pub fn filter_map<U, F>(orig: Self, f: F) -> Result<PerCpuRefMut<'a, U>, Self>
    where
        F: FnOnce(&mut T) -> Option<&mut U>,
    {
        // Do not run drop() on `orig`
        let mut orig = ManuallyDrop::new(orig);
        match f(&mut *orig) {
            Some(new) => Ok(PerCpuRefMut {
                ptr: NonNull::from(new),
                borrow: orig.borrow,
                _phantom: PhantomData,
            }),
            None => Err(ManuallyDrop::into_inner(orig)),
        }
    }

    /// A reentrancy-safe version of [`RefMut::filter_map()`](core::cell::RefMut::filter_map).
    pub fn map_split<U, V, F>(orig: Self, f: F) -> (PerCpuRefMut<'a, U>, PerCpuRefMut<'a, V>)
    where
        F: FnOnce(&mut T) -> (&mut U, &mut V),
    {
        // Do not run drop() on `orig`
        let mut orig = ManuallyDrop::new(orig);

        // Bind borrow to a variable so that we can pass `&mut orig` below.
        let borrow = orig.borrow;
        // The borrow count must already be negative for `orig` to be valid, so
        // decrease it once more. This correct because the 2 new borrows point
        // to non-overlapping regions of memory. This is a guarantee of the borrow
        // checker for safe references, which is what F takes and returns.
        borrow.fetch_sub(1, Ordering::Relaxed);
        let (a, b) = f(&mut *orig);
        (
            PerCpuRefMut {
                ptr: NonNull::from(a),
                borrow,
                _phantom: PhantomData,
            },
            PerCpuRefMut {
                ptr: NonNull::from(b),
                borrow,
                _phantom: PhantomData,
            },
        )
    }
}

impl<T> Drop for PerCpuRefMut<'_, T> {
    fn drop(&mut self) {
        self.borrow.fetch_add(1, Ordering::Relaxed);
    }
}

impl<T> Deref for PerCpuRefMut<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        // SAFETY: the pointer is valid by construction. This type can
        // only exist if there are no other readers or writers, so we
        // cannot violate Rust's memory model.
        unsafe { self.ptr.as_ref() }
    }
}

impl<T> DerefMut for PerCpuRefMut<'_, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        // SAFETY: the pointer is valid by construction. This type can
        // only exist if there are no other readers or writers, so we
        // cannot violate Rust's memory model.
        unsafe { self.ptr.as_mut() }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_basic() {
        let cell = PerCpuCell::new(0u32);

        let read1 = cell.borrow();
        let read2 = cell.borrow();
        cell.try_borrow_mut().unwrap_err();
        assert_eq!(*read1, 0);
        assert_eq!(*read2, 0);
        drop(read1);
        drop(read2);
        *cell.borrow_mut() = 1;
        assert_eq!(*cell.borrow(), 1);
    }

    #[derive(Default, Debug, Clone, Copy)]
    struct Foo {
        bar: u32,
        baz: u32,
    }

    #[test]
    fn test_map_mut() {
        let cell = PerCpuCell::new(Foo::default());

        let mut write = cell.borrow_mut();
        cell.try_borrow().unwrap_err();

        assert_eq!(write.bar, 0);
        write.bar = 1;
        assert_eq!(write.bar, 1);

        let mut bar = PerCpuRefMut::map(write, |foo| &mut foo.bar);
        assert_eq!(*bar, 1);
        cell.try_borrow().unwrap_err();

        *bar = 2;
        drop(bar);

        assert_eq!(cell.get().bar, 2);
    }

    #[test]
    fn test_map_split_mut() {
        let cell = PerCpuCell::new(Foo::default());

        // Get a writer and check that readers are disallowed
        let mut write = cell.borrow_mut();
        cell.try_borrow().unwrap_err();

        // Set some initial state
        assert_eq!(write.bar, 0);
        write.bar = 1;
        assert_eq!(write.bar, 1);

        // Split the writer
        let (mut bar, mut baz) = PerCpuRefMut::map_split(write, |foo| (&mut foo.bar, &mut foo.baz));

        // No readers allowed while writers are alive
        cell.try_borrow().unwrap_err();

        // Check previous state and make some writes
        assert_eq!(*bar, 1);
        assert_eq!(*baz, 0);
        *bar = 2;
        *baz = 3;
        assert_eq!(*bar, 2);
        assert_eq!(*baz, 3);

        // No readers allowed until *all* writers go away
        drop(bar);
        cell.try_borrow().unwrap_err();
        drop(baz);

        // Writes should be visible
        let read = cell.borrow();
        assert_eq!(read.bar, 2);
        assert_eq!(read.baz, 3);
    }
}
