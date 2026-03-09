use core::ptr::NonNull;

use pyo3::prelude::*;
use pyo3::types::PyString;

/// Gets the object's type data as `T`. Returns [`None`] if the type data can't
/// be retreieved from python.
/// # Safety
/// The object's type data must be a valid `T`
pub(crate) unsafe fn type_data<'a, T>(
  obj: Borrowed<'a, '_, PyAny>,
) -> PyResult<&'a T> {
  type_data_ptr(obj).map_or_else(
    || Err(PyErr::fetch(obj.py())),
    // SAFETY: caller upholds requirements
    |p| unsafe { Ok(p.as_ref()) },
  )
}

/// Sets the object's type data. Returns true if it succeeds.
/// If it fails, a python exception will be set.
/// # Safety
/// `obj`'s type data must be able to hold a `T`
#[must_use]
pub(crate) unsafe fn set_type_data<T>(
  obj: Borrowed<'_, '_, PyAny>,
  val: T,
) -> bool {
  let Some(p) = type_data_ptr::<T>(obj) else {
    return false;
  };
  // SAFETY: caller upholds requirements
  unsafe { p.write(val) };
  true
}

/// # Safety
/// `obj`'s type data must be a valid `T` and it must not be used again
pub(crate) unsafe fn drop_type_data<T>(obj: Borrowed<'_, '_, PyAny>) {
  if let Some(p) = type_data_ptr::<T>(obj) {
    // SAFETY: the pyobject's type data was created using the `new_fn`
    unsafe { p.drop_in_place() };
  }
}

/// Helper function to get a pointer to an object's type data
#[expect(clippy::disallowed_methods, reason = "implementing safe wrapper")]
fn type_data_ptr<T>(obj: Borrowed<'_, '_, PyAny>) -> Option<NonNull<T>> {
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
