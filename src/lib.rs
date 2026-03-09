use std::borrow::Cow;
use std::ffi::{CString, c_ulong, c_void};
use std::mem;
use std::ptr;

use pyo3::PyTypeInfo as _;
use pyo3::ffi::{
  Py_TPFLAGS_DEFAULT, Py_TPFLAGS_HEAPTYPE, Py_tp_finalize, Py_tp_init,
  Py_tp_new, PyType_Slot, PyType_Spec, destructor, initproc, newfunc,
};
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyTuple, PyType};

use self::typeobject::RuntimeTypeObject;

mod tp;
mod typeobject;

pub struct Builder<'py, 'n, T> {
  new_fn: NewFn<T>,
  flags: c_ulong,
  init_fn: Option<InitFn<T>>,
  name: Cow<'n, str>,
  module: Option<Bound<'py, PyModule>>,
  bases: Vec<Bound<'py, PyType>>,
}

impl<'py, 'n, T> Builder<'py, 'n, T> {
  pub fn new(name: impl Into<Cow<'n, str>>, new_fn: NewFn<T>) -> Self {
    Builder {
      new_fn,
      flags: (Py_TPFLAGS_DEFAULT | Py_TPFLAGS_HEAPTYPE),
      init_fn: None,
      name: name.into(),
      module: None,
      bases: Vec::new(),
    }
  }

  pub fn bases(
    &mut self,
    bases: impl IntoIterator<Item = Bound<'py, PyType>>,
  ) -> &mut Self {
    self.bases.extend(bases);
    self
  }

  pub fn module(&mut self, module: Bound<'py, PyModule>) -> &mut Self {
    self.module = Some(module);
    self
  }

  pub fn init_fn(&mut self, init_fn: InitFn<T>) -> &mut Self {
    self.init_fn = Some(init_fn);
    self
  }

  pub fn build(&self, py: Python<'py>) -> PyResult<Bound<'py, PyType>> {
    let name = match &self.module {
      Some(module) => {
        CString::new(format!("{}.{}", module.name()?.to_str()?, self.name))
          .unwrap()
      },
      None => CString::new(self.name.as_bytes()).unwrap(),
    };
    let mut slots = self.slots();
    let spec = PyType_Spec {
      name: name.as_ptr(),
      basicsize: -i32::try_from(mem::size_of::<T>()).unwrap(),
      itemsize: 0,
      flags: self.flags as _,
      slots: slots.as_mut_ptr(),
    };

    let bases = if self.bases.is_empty() {
      PyAny::type_object(py).into_any()
    } else {
      PyTuple::new(py, &self.bases)?.into_any()
    };

    let rtt = RuntimeTypeObject::new(self.new_fn, self.init_fn);
    let module = self.module.as_ref().map(|m| m.as_borrowed());

    // SAFETY: we just created a valid `spec` and all the pointers it
    //         contains point to things still in scope
    unsafe { rtt.make_type(spec, bases.as_borrowed(), module) }
  }

  fn slots(&self) -> Vec<PyType_Slot> {
    let mut slots = vec![PyType_Slot {
      slot: Py_tp_new,
      pfunc: tp::new::<T> as newfunc as *mut c_void,
    }];
    if mem::needs_drop::<T>() {
      slots.push(PyType_Slot {
        slot: Py_tp_finalize,
        pfunc: tp::finalize::<T> as destructor as *mut c_void,
      });
    }
    if self.init_fn.is_some() {
      slots.push(PyType_Slot {
        slot: Py_tp_init,
        pfunc: tp::init::<T> as initproc as *mut c_void,
      });
    }

    slots.push(PyType_Slot { slot: 0, pfunc: ptr::null_mut() });
    slots
  }
}

pub type NewFn<T> = for<'py> fn(
  Bound<'py, PyType>,
  Bound<'py, PyTuple>,
  Option<Bound<'py, PyDict>>,
) -> PyResult<T>;

pub type InitFn<T> = for<'py> fn(
  &T,
  ty: Bound<'py, PyType>,
  args: Bound<'py, PyTuple>,
  kwargs: Option<Bound<'py, PyDict>>,
) -> PyResult<()>;

#[cfg(test)]
#[expect(clippy::allow_attributes, reason = "tests")]
#[allow(clippy::unnecessary_wraps, clippy::unused_self, reason = "tests")]
mod tests {
  use super::*;

  use std::sync::atomic::{AtomicBool, Ordering};

  #[test]
  fn obj_created_inited_and_destroyed() {
    thread_local! {
      static INIT: AtomicBool = const { AtomicBool::new(false) };
      static DESTROY: AtomicBool = const { AtomicBool::new(false) };
    }

    Python::initialize();
    Python::attach(|py| {
      let ty = Builder::new("S", |_, _, _| Ok(S::default()))
        .init_fn(|slf, _, _, _| slf.__init__())
        .build(py)
        .unwrap();
      let obj = ty.call0().unwrap();
      drop(obj);
      PyModule::import(py, "gc")
        .unwrap()
        .getattr("collect")
        .unwrap()
        .call0()
        .unwrap();
    });

    assert!(INIT.with(|b| b.load(Ordering::SeqCst)));
    assert!(DESTROY.with(|b| b.load(Ordering::SeqCst)));

    #[derive(Default)]
    struct S {
      _n: i32, // prevent it from being a ZST
    }

    impl S {
      fn __init__(&self) -> PyResult<()> {
        INIT.with(|b| b.store(true, Ordering::SeqCst));
        Ok(())
      }
    }

    impl Drop for S {
      fn drop(&mut self) {
        DESTROY.with(|b| b.store(true, Ordering::SeqCst));
      }
    }
  }
}
