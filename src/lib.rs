use std::borrow::Cow;
use std::ffi::{CString, c_ulong, c_void};
use std::mem;

use pyo3::PyTypeInfo as _;
use pyo3::ffi::{
  Py_TPFLAGS_DEFAULT, Py_TPFLAGS_HEAPTYPE, Py_tp_finalize, Py_tp_init,
  Py_tp_new, destructor, initproc, newfunc,
};
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyTuple, PyType};

use self::typeobject::RuntimeTypeObject;
use self::typespec::TypeSpec;

mod data_ptr;
mod tp;
mod typeobject;
mod typespec;

pub struct PyTypeBuilder<'py, 'n, T> {
  flags: c_ulong,
  module: Option<Bound<'py, PyModule>>,
  name: Cow<'n, str>,
  bases: Vec<Bound<'py, PyType>>,
  new_fn: Option<NewFn<T>>,
  init_fn: Option<InitFn<T>>,
}

impl<'py, 'n, T> PyTypeBuilder<'py, 'n, T> {
  pub fn new(name: impl Into<Cow<'n, str>>, new_fn: NewFn<T>) -> Self {
    // SAFETY: new_fn is set right after this call
    let mut this = unsafe { PyTypeBuilder::new_without_new_fn(name) };
    this.new_fn(new_fn);
    this
  }

  /// # Safety
  /// Caller must ensure that no type object is used while the type data is not initialized
  ///
  /// There are two ways to accomplish this
  /// - set the new_fn by calling [`Self::new_fn`] before [`Self::build`]
  /// - manually set the type data for every instance of the new python type
  pub unsafe fn new_without_new_fn(name: impl Into<Cow<'n, str>>) -> Self {
    PyTypeBuilder {
      flags: (Py_TPFLAGS_DEFAULT | Py_TPFLAGS_HEAPTYPE),
      module: None,
      name: name.into(),
      bases: Vec::new(),
      new_fn: None,
      init_fn: None,
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

  pub fn new_fn(&mut self, new_fn: NewFn<T>) -> &mut Self {
    self.new_fn = Some(new_fn);
    self
  }

  pub fn init_fn(&mut self, init_fn: InitFn<T>) -> &mut Self {
    self.init_fn = Some(init_fn);
    self
  }

  pub fn build(self, py: Python<'py>) -> PyResult<Bound<'py, PyType>> {
    let spec = self.spec()?;

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

  fn spec(&self) -> PyResult<TypeSpec> {
    let name = match &self.module {
      Some(module) => {
        CString::new(format!("{}.{}", module.name()?.to_str()?, self.name))
          .unwrap()
      },
      None => CString::new(self.name.as_bytes()).unwrap(),
    };

    let mut spec = TypeSpec::new(
      name,
      -i32::try_from(mem::size_of::<T>()).unwrap(),
      0,
      self.flags as _,
    );

    spec.push_slot(Py_tp_new, tp::new::<T> as newfunc as *mut c_void);
    if mem::needs_drop::<T>() {
      spec.push_slot(
        Py_tp_finalize,
        tp::finalize::<T> as destructor as *mut c_void,
      );
    }
    if self.init_fn.is_some() {
      spec.push_slot(Py_tp_init, tp::init::<T> as initproc as *mut c_void);
    }

    Ok(spec)
  }
}

pub type NewFn<T> = Box<
  dyn for<'py> Fn(
    Bound<'py, PyType>,
    Bound<'py, PyTuple>,
    Option<Bound<'py, PyDict>>,
  ) -> PyResult<T>,
>;

pub type InitFn<T> = Box<
  dyn for<'py> Fn(
    &T,
    Bound<'py, PyType>,
    Bound<'py, PyTuple>,
    Option<Bound<'py, PyDict>>,
  ) -> PyResult<()>,
>;
