use std::any::{TypeId, type_name};
use std::error::Error;
use std::fmt::Display;
use std::marker::PhantomData;
use std::ptr::{self, NonNull};

/// Type erased data that can be moved to a pointer.
///
/// The data will be dropped properly if it isn't moved.
pub(crate) struct MovingData {
  ptr: NonNull<()>, // TODO store small objects inline ?
  mover: unsafe fn(src: NonNull<()>, dst: NonNull<()>),
  dropper: unsafe fn(NonNull<()>),
}

impl MovingData {
  pub(crate) fn new<T: 'static>(data: T) -> Self {
    Self {
      ptr: NonNull::new(Box::into_raw(Box::new(Some(data))))
        .unwrap()
        .cast(),
      // SAFETY: caller will uphold requirements
      mover: |src, dst| unsafe {
        let data: &mut Option<T> = src.cast().as_mut();
        let data = data.take().unwrap();
        dst.cast().write(data);
      },
      dropper: |ptr| {
        // SAFETY: ptr is set above via `Box::into_raw` of the correct type
        let _: Box<Option<T>> = unsafe { Box::from_raw(ptr.cast().as_ptr()) };
      },
    }
  }

  /// Writes the data into `dst`.
  ///
  /// # Safety
  /// The `dst` pointer must be a valid write target for a `T` that `self`
  /// was constructed with.
  pub(crate) unsafe fn move_it(self, dst: NonNull<()>) {
    // SAFETY: caller upholds requirements
    unsafe { (self.mover)(self.ptr, dst) };
  }
}

impl Drop for MovingData {
  fn drop(&mut self) {
    // SAFETY: `Self::new` ensures the pointers match
    unsafe { (self.dropper)(self.ptr) };
  }
}

#[repr(C)]
pub(crate) struct ErasedTraitObjects<const N: usize> {
  objects: [[*mut (); 2]; N],
  dropper: unsafe fn([*mut (); 2]),
  type_id: TypeId,
}

impl<const N: usize> ErasedTraitObjects<N> {
  pub(crate) fn new<T: ?Sized + Send + Sync + 'static>(
    objects: [Option<Box<T>>; N],
  ) -> Self {
    let mut slf = Self {
      objects: [[ptr::null_mut(); 2]; N],
      // SAFETY: `obj` is written to below
      dropper: |mut obj| unsafe {
        (&raw mut obj as *mut Box<T>).drop_in_place();
      },
      type_id: TypeId::of::<T>(),
    };

    for (idx, obj) in objects.into_iter().enumerate() {
      if let Some(obj) = obj {
        // SAFETY: `slf.objects` items are the size of a wide pointer
        unsafe {
          (&raw mut slf.objects[idx] as *mut Box<T>).write(obj);
        }
      }
    }

    slf
  }

  pub(crate) fn typed<T: ?Sized + Send + Sync + 'static>(
    &self,
  ) -> Result<&TraitObjects<N, T>, ErasedTypeError> {
    if self.type_id == TypeId::of::<T>() {
      // SAFETY: `TraitObjects` is repr-compatible with `Self`
      Ok(unsafe { NonNull::from_ref(self).cast().as_ref() })
    } else {
      Err(ErasedTypeError { expected: type_name::<T>() })
    }
  }
}

impl<const N: usize> Drop for ErasedTraitObjects<N> {
  fn drop(&mut self) {
    for obj in self.objects {
      if obj != [ptr::null_mut(); 2] {
        // SAFETY: the constructor ensures it's the correct type and we just
        //         checked for null
        unsafe { (self.dropper)(obj) };
      }
    }
  }
}

#[repr(C)]
pub(crate) struct TraitObjects<
  const N: usize,
  T: ?Sized + Send + Sync + 'static,
> {
  objects: [[*mut (); 2]; N],
  dropper: fn([*mut (); 2]),
  _marker: PhantomData<fn() -> T>,
}

impl<const N: usize, T: ?Sized + Send + Sync + 'static> TraitObjects<N, T> {
  /// Returns the trait object at `idx` if it exists.
  ///
  /// # Panics
  /// Panics if `idx` is out of bounds.
  pub(crate) fn get(&self, idx: usize) -> Option<&T> {
    // SAFETY: `self.objects` is written to in constructor of `ErasedTraitObjects`
    unsafe {
      NonNull::from_ref(&self.objects[idx])
        .cast::<Option<Box<T>>>()
        .as_ref()
    }
    .as_deref()
  }
}

#[derive(Debug)]
pub(crate) struct ErasedTypeError {
  expected: &'static str,
}

impl Display for ErasedTypeError {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    write!(f, "expected `{}`", self.expected)
  }
}

impl Error for ErasedTypeError {}

#[cfg(test)]
mod tests {
  use std::sync::atomic::{AtomicBool, Ordering};

  use super::*;

  #[test]
  fn erased_trait_object_can_be_used_and_dropped() {
    S::clear();
    let obj: Box<dyn Display + Send + Sync + 'static> = Box::new(S);
    let original_ptr: *const dyn Display = &*obj;

    let erased = ErasedTraitObjects::new([Some(obj)]);
    let objects = erased.typed::<dyn Display + Send + Sync>().unwrap();
    let actual = objects.get(0).unwrap();
    let erased_ptr: *const dyn Display = actual;

    assert_eq!(original_ptr, erased_ptr);
    assert_eq!(S::DISPLAY_VALUE, actual.to_string());
    drop(erased);
    S::assert_dropped();
  }

  struct S;
  impl S {
    thread_local! { static DROP_FLAG: AtomicBool = const { AtomicBool::new(false) } }
    const DISPLAY_VALUE: &str = "hello";

    fn clear() {
      Self::DROP_FLAG.with(|b| b.store(false, Ordering::SeqCst));
    }
    fn assert_dropped() {
      assert!(Self::DROP_FLAG.with(|b| b.load(Ordering::SeqCst)));
    }
  }
  impl Display for S {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
      f.write_str(Self::DISPLAY_VALUE)
    }
  }
  impl Drop for S {
    fn drop(&mut self) {
      Self::DROP_FLAG.with(|b| b.store(true, Ordering::SeqCst));
    }
  }
}
