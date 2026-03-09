//! This module contains functions that will be used to populate the slots of
//! the `PyTypeObject`s.

use std::ffi::c_int;
use std::ptr::{self, NonNull};

use pyo3::exceptions::{PySystemError, PyTypeError};
use pyo3::ffi::{PyObject, PyType_GenericNew, PyTypeObject};
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyString, PyTuple, PyType};

use crate::typeobject::RuntimeTypeObject;

/// # Safety
/// Must be called in `tp_new` slot of type created with [`RuntimeTypeObject`] as type data
pub(crate) unsafe extern "C" fn new<T>(
  ty: *mut PyTypeObject,
  args: *mut PyObject,
  kwargs: *mut PyObject,
) -> *mut PyObject {
  // SAFETY: caller ensures this function is only called by python runtime
  let py = unsafe { Python::assume_attached() };
  // SAFETY: python doesn't give null type object
  let ty = unsafe { PyType::from_borrowed_type_ptr(py, ty) };
  // SAFETY: python always gives args as non-null PyTuple
  let args: Bound<'_, PyTuple> =
    unsafe { Bound::from_borrowed_ptr(py, args).cast_into_unchecked() };
  // SAFETY: python gives kwargs as PyDict and we check for null
  let kwargs: Option<Bound<'_, PyDict>> = unsafe {
    Bound::from_borrowed_ptr_or_opt(py, kwargs).map(|b| b.cast_into_unchecked())
  };
  let rtt: &RuntimeTypeObject = match ty.extract() {
    Ok(rtt) => rtt,
    Err(err) => {
      err.restore(py);
      return ptr::null_mut();
    },
  };

  // SAFETY: `RuntimeTypeObject::new` stores this fn's ptr with the correct `T`
  let Some(new_fn) = (unsafe { rtt.new_fn::<T>() }) else {
    PyTypeError::new_err(format!(
      "{} can only be instantiated from native code",
      ty.name().unwrap_or_else(|_| PyString::new(py, "<unknown>"))
    ))
    .restore(py);
    return ptr::null_mut();
  };

  match new_fn(ty.clone(), args.clone(), kwargs.clone()) {
    Ok(val) => {
      // SAFETY: forwarding args from python and writing to a properly aligned pointer
      unsafe {
        let Some(obj) = NonNull::new(PyType_GenericNew(
          ty.as_type_ptr(),
          args.as_ptr(),
          kwargs.map(|d| d.as_ptr()).unwrap_or_default(),
        )) else {
          return ptr::null_mut();
        };
        let obj = Borrowed::from_ptr(py, obj.as_ptr());

        let Some(p) = type_data_ptr::<T>(obj) else {
          return ptr::null_mut();
        };
        p.write(val);

        obj.as_ptr()
      }
    },
    Err(err) => {
      err.restore(py);
      ptr::null_mut()
    },
  }
}

/// # Safety
/// `slf` must have been created with [`new`]
pub(crate) unsafe extern "C" fn init<T>(
  slf: *mut PyObject,
  args: *mut PyObject,
  kwargs: *mut PyObject,
) -> c_int {
  // SAFETY: caller ensures this function is only called by python runtime
  let py = unsafe { Python::assume_attached() };
  // SAFETY: python doesn't give null self
  let slf = unsafe { Bound::from_borrowed_ptr(py, slf) };
  let ty = slf.get_type();
  // SAFETY: python always gives args as non-null PyTuple
  let args: Bound<'_, PyTuple> =
    unsafe { Bound::from_borrowed_ptr(py, args).cast_into_unchecked() };
  // SAFETY: python gives kwargs as PyDict and we check for null
  let kwargs: Option<Bound<'_, PyDict>> = unsafe {
    Bound::from_borrowed_ptr_or_opt(py, kwargs).map(|b| b.cast_into_unchecked())
  };

  fn inner<T>(
    slf: Borrowed<'_, '_, PyAny>,
    ty: Bound<'_, PyType>,
    args: Bound<'_, PyTuple>,
    kwargs: Option<Bound<'_, PyDict>>,
  ) -> PyResult<()> {
    let rtt: &RuntimeTypeObject = ty.extract()?;
    // SAFETY: `RuntimeTypeObject::new` stores this fn's ptr with the correct `T`
    let init_fn = unsafe { rtt.init_fn::<T>() }.ok_or_else(|| {
      PySystemError::new_err(format!(
        "could not get init fn for <{}>: {}",
        ty.qualname()
          .unwrap_or_else(|_| PyString::new(ty.py(), "<unknown>")),
        core::any::type_name::<T>()
      ))
    })?;
    // SAFETY: python will only call this function after `tp_new` runs
    let t = unsafe { type_data(slf.as_borrowed()) }?;
    init_fn(t, ty, args, kwargs)
  }

  match inner::<T>(slf.as_borrowed(), ty, args, kwargs) {
    Ok(()) => 0,
    Err(err) => {
      err.restore(py);
      -1
    },
  }
}

/// # Safety
/// The `obj` must have been created with [`new`]
pub(crate) unsafe extern "C" fn finalize<T>(obj: *mut PyObject) {
  // SAFETY: this function will only be called by python
  let py = unsafe { Python::assume_attached() };
  // SAFETY: python just gave this to us and we only use it in the body of this fn
  let obj = unsafe { Borrowed::from_ptr(py, obj) };
  if let Some(p) = type_data_ptr::<T>(obj) {
    // SAFETY: the pyobject's type data was created using the `new_fn`
    unsafe { p.drop_in_place() };
  }
}

/// Gets the object's type data as `T`. Returns [`None`] if the type data can't
/// be retreieved from python.
/// # Safety
/// The object's type data must be a valid `T`
unsafe fn type_data<'a, T>(obj: Borrowed<'a, '_, PyAny>) -> PyResult<&'a T> {
  type_data_ptr(obj).map_or_else(
    || Err(PyErr::fetch(obj.py())),
    // SAFETY: caller upholds requirements
    |p| unsafe { Ok(p.as_ref()) },
  )
}

/// Helper function to get a pointer to an object's type data
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
