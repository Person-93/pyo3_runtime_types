//! Miscallaneous extension traits to [`pyo3`] types

use pyo3::PyTypeInfo;
use pyo3::ffi::PyType_IS_GC;
use pyo3::prelude::*;

use crate::typeobject::RuntimeTypeObject;

pub trait BoundExt {
  /// Get the runtime data associated with this type
  fn rt_data<T: Send + Sync + 'static>(&self) -> PyResult<&T>;
}

impl<P: PyTypeInfo> BoundExt for Bound<'_, P> {
  fn rt_data<T: Send + Sync + 'static>(&self) -> PyResult<&T> {
    let ty = self.as_any().get_type();
    let rtt: &RuntimeTypeObject = ty.extract()?;
    rtt.get_data(self.as_any().as_borrowed())
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
