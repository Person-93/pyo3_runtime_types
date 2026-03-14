//! Miscallaneous extension traits to [`pyo3`] types

use pyo3::PyTypeInfo;
use pyo3::ffi::PyType_IS_GC;
use pyo3::prelude::*;
use pyo3::types::PyType;

use crate::typeobject::RuntimeTypeObject;

pub trait BoundExt {
  /// Get the runtime data associated with this type
  fn rt_data<T: Send + Sync + 'static>(&self) -> PyResult<&T>;
}

impl<P: PyTypeInfo> BoundExt for Bound<'_, P> {
  fn rt_data<T: Send + Sync + 'static>(&self) -> PyResult<&T> {
    let ty = self.get_type_borrowed();
    let rtt: &RuntimeTypeObject = ty.extract()?;
    rtt.get_data(self.as_any().as_borrowed())
  }
}

pub(crate) trait BoundExtInernal<'py>: BoundExt {
  fn get_type_borrowed<'a>(&'a self) -> Borrowed<'a, 'py, PyType>;
}

impl<'py, P: PyTypeInfo> BoundExtInernal<'py> for Bound<'py, P> {
  fn get_type_borrowed<'a>(&'a self) -> Borrowed<'a, 'py, PyType> {
    // SAFETY: pyo3 guarantees that ptr returned by `get_type_ptr` points to a
    //         valid type object
    unsafe {
      Borrowed::from_ptr(self.py(), self.as_any().get_type_ptr().cast())
        .cast_unchecked()
    }
  }
}

#[allow(unused, clippy::allow_attributes, reason = "useful for tests")]
pub(crate) trait PyTypeExt<'a>: PyTypeMethods<'a> {
  fn is_gc(&self) -> bool {
    // SAFETY: as_type_ptr returns a pointer to a valid PyTypeObject
    (unsafe { PyType_IS_GC(self.as_type_ptr()) }) != 0
  }
}

impl<'a, T: PyTypeMethods<'a>> PyTypeExt<'a> for T {}
