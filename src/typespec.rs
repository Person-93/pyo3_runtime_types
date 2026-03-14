//! This module contains a safe-ish wrapper around [`PyType_Spec`]
#![expect(clippy::disallowed_types, reason = "implementing replacement")]

use std::ffi::{CString, c_int, c_uint, c_void};
use std::ptr;

use pyo3::ffi::{PyType_Slot, PyType_Spec};

pub(crate) struct TypeSpec {
  #[expect(unused, reason = "spec holds a pointer to it")]
  name: CString,
  slots: Vec<PyType_Slot>,
  spec: PyType_Spec,
}

impl TypeSpec {
  pub(crate) fn new(
    name: CString,
    basicsize: c_int,
    itemsize: c_int,
    flags: c_uint,
  ) -> Self {
    Self {
      slots: Vec::with_capacity(2),
      spec: PyType_Spec {
        name: name.as_ptr(),
        basicsize,
        itemsize,
        flags,
        slots: ptr::null_mut(),
      },
      name,
    }
  }

  pub(crate) fn add_flags(&mut self, flags: u64) {
    self.spec.flags |= flags as c_uint;
  }

  pub(crate) fn push_slot(&mut self, slot: c_int, pfunc: *mut c_void) {
    self.slots.push(PyType_Slot { slot, pfunc });
  }

  pub(crate) fn finish(&mut self) -> &mut PyType_Spec {
    self
      .slots
      .push(PyType_Slot { slot: 0, pfunc: ptr::null_mut() });
    self.spec.slots = self.slots.as_mut_ptr();
    &mut self.spec
  }
}
