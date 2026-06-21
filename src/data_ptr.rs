use core::ptr::NonNull;

use pyo3::prelude::*;
use pyo3::types::PyString;

/// Gets the object's type data as `T`. Returns [`None`] if the type data can't
/// be retreieved from python.
/// # Safety
/// The object's type data must be a valid `T`
pub(crate) unsafe fn type_data<'a, T: Send + Sync + 'static>(
  obj: Borrowed<'a, '_, PyAny>,
) -> PyResult<&'a T> {
  type_data_ptr(obj).map_or_else(
    || Err(PyErr::fetch(obj.py())),
    // SAFETY: caller upholds requirements
    |p| unsafe { Ok(p.as_ref()) },
  )
}

/// # Safety
/// `obj`'s type data must be a valid `T` and it must not be used again
pub(crate) unsafe fn drop_type_data<T: Send + Sync + 'static>(
  obj: Borrowed<'_, '_, PyAny>,
) {
  if let Some(p) = type_data_ptr::<T>(obj) {
    // SAFETY: the pyobject's type data was created using the `new_fn`
    unsafe { p.drop_in_place() };
  }
}

/// Helper function to get a pointer to an object's type data
#[expect(clippy::disallowed_methods, reason = "implementing safe wrapper")]
pub(crate) fn type_data_ptr<T: Send + Sync + 'static>(
  obj: Borrowed<'_, '_, PyAny>,
) -> Option<NonNull<T>> {
  use pyo3::ffi::PyObject_GetTypeData;
  let ty = obj.get_type();
  // SAFETY: calling python API with pointers from pyo3
  let p = unsafe {
    NonNull::new(
      PyObject_GetTypeData(obj.as_ptr(), ty.as_type_ptr()).cast::<T>(),
    )
  }?;

  assert!(
    p.is_aligned(),
    "TypeData for <{}> is not properly aligned `{}`",
    ty.qualname()
      .unwrap_or_else(|_| PyString::new(obj.py(), "<unknown>")),
    core::any::type_name::<T>()
  );
  Some(p)
}
