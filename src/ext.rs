//! Miscallaneous extension traits to [`pyo3`] types

use pyo3::PyTypeInfo;
use pyo3::ffi::PyType_IS_GC;
use pyo3::prelude::*;

use crate::typeobject::RuntimeTypeObject;

pub trait BorrowedExt<'a> {
  /// Get the runtime data associated with this type
  fn rt_data<T: Send + Sync + 'static>(self) -> PyResult<&'a T>;
}

impl<'a, 'py: 'a> BorrowedExt<'a> for Borrowed<'a, 'py, PyAny> {
  fn rt_data<T: Send + Sync + 'static>(self) -> PyResult<&'a T> {
    let ty = self.get_type();
    let rtt: &RuntimeTypeObject = ty.extract()?;
    rtt.get_data(self)
  }
}

pub trait BoundExt {
  /// Get the runtime data associated with this type
  fn rt_data<T: Send + Sync + 'static>(&self) -> PyResult<&T>;
}

impl<P: PyTypeInfo> BoundExt for Bound<'_, P> {
  fn rt_data<T: Send + Sync + 'static>(&self) -> PyResult<&T> {
    self.as_any().as_borrowed().rt_data()
  }
}

pub(crate) trait PyTypeExt<'a>: PyTypeMethods<'a> {
  fn is_gc(&self) -> bool {
    // SAFETY: as_type_ptr returns a pointer to a valid PyTypeObject
    (unsafe { PyType_IS_GC(self.as_type_ptr()) }) != 0
  }
}

impl<'a, T: PyTypeMethods<'a>> PyTypeExt<'a> for T {}
