//! This module contains functions that will be used to populate the slots of
//! the `PyTypeObject`s.

use std::ffi::c_int;
use std::mem;
use std::ptr::{self, NonNull};

use pyo3::exceptions::{PySystemError, PyTypeError};
use pyo3::ffi::{
  Py_tp_free, PyObject, PyObject_CallFinalizerFromDealloc, PyObject_GC_UnTrack,
  PyType_GenericNew, PyType_GetSlot, PyTypeObject, destructor,
};
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyString, PyTuple, PyType};

use crate::data_ptr::{drop_type_data, set_type_data, type_data};
use crate::ext::*;
use crate::no_exceptions;
use crate::typeobject::RuntimeTypeObject;

/// # Safety
/// Must be called in `tp_new` slot of type created with [`RuntimeTypeObject`] as type data
pub(crate) unsafe extern "C" fn new<T: Send + Sync + 'static>(
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
      "could not get __new__ fn for <{}>: {}",
      ty.name().unwrap_or_else(|_| PyString::new(py, "<unknown>")),
      core::any::type_name::<T>()
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

        if !set_type_data(obj, val) {
          // NOTE: the python object is leaked, freeing it would run the rust
          //       type's Drop on uninitialized memory
          return ptr::null_mut();
        }

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
pub(crate) unsafe extern "C" fn init<T: Send + Sync + 'static>(
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

  fn inner<T: Send + Sync + 'static>(
    slf: Borrowed<'_, '_, PyAny>,
    ty: Borrowed<'_, '_, PyType>,
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
    init_fn(t, ty.to_owned(), args, kwargs)
  }

  match inner::<T>(slf.as_borrowed(), ty.as_borrowed(), args, kwargs) {
    Ok(()) => 0,
    Err(err) => {
      err.restore(py);
      -1
    },
  }
}

/// # Safety
/// The `obj` must have been created with [`new`] and python must be in
/// attached state
pub(crate) unsafe extern "C" fn dealloc<T: Send + Sync + 'static>(
  obj: *mut PyObject,
) {
  // SAFETY: caller upholds rquirements
  let py = unsafe { Python::assume_attached() };
  no_exceptions(py, || {
    // SAFETY: python does not call dealloc with null ptr
    let obj = unsafe { Borrowed::from_ptr(py, obj) };
    let ty = obj.get_type();

    // SAFETY: called with ptr received from python
    if unsafe { PyObject_CallFinalizerFromDealloc(obj.as_ptr()) < 0 } {
      return;
    }

    if ty.is_gc() {
      // SAFETY: called with ptr received from python
      unsafe { PyObject_GC_UnTrack(obj.as_ptr().cast()) };
    }

    // SAFETY: the type builder ensures this is the correct `T` and it'll never
    //         be used again becuase we deallocate it below
    unsafe { drop_type_data::<T>(obj) };
    // TODO drop any type data(s) from base classes

    // SAFETY: calling python api with valid `PyObject` ptr
    unsafe {
      let tp_free: destructor =
        mem::transmute(PyType_GetSlot(ty.as_type_ptr(), Py_tp_free));
      tp_free(obj.as_ptr());
    }
  });
}
