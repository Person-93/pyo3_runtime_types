use std::ptr::NonNull;

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
