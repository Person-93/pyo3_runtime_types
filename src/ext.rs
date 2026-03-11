//! Miscallaneous extension traits to [`pyo3`] types

use pyo3::ffi::PyType_IS_GC;
use pyo3::prelude::*;

pub(crate) trait PyTypeExt<'a>: PyTypeMethods<'a> {
  fn is_gc(&self) -> bool {
    // SAFETY: as_type_ptr returns a pointer to a valid PyTypeObject
    (unsafe { PyType_IS_GC(self.as_type_ptr()) }) != 0
  }
}

impl<'a, T: PyTypeMethods<'a>> PyTypeExt<'a> for T {}
