use std::borrow::Cow;
use std::ffi::{CString, c_ulong, c_void};
use std::marker::PhantomData;
use std::mem;

use pyo3::PyTypeInfo as _;
use pyo3::exceptions::PySystemError;
use pyo3::ffi::{
  Py_TPFLAGS_DEFAULT, Py_TPFLAGS_HEAPTYPE, Py_TPFLAGS_TYPE_SUBCLASS,
  Py_tp_dealloc, Py_tp_init, Py_tp_new, destructor, initproc, newfunc,
};
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyTuple, PyType};

use self::type_erased::MovingData;
use self::typeobject::RuntimeTypeObject;
use self::typespec::TypeSpec;

mod data_ptr;
mod tp;
mod type_erased;
mod typeobject;
mod typespec;

pub struct PyTypeBuilder<'py, 'n, T: 'static> {
  flags: c_ulong,
  module: Option<Bound<'py, PyModule>>,
  name: Cow<'n, str>,
  metaclass: Option<MetaclassWithData<'py>>,
  bases: Vec<Bound<'py, PyType>>,
  new_fn: Option<Box<NewFn<T>>>,
  init_fn: Option<Box<InitFn<T>>>,
}

impl<'n> PyTypeBuilder<'_, 'n, ()> {
  pub fn new_empty(name: impl Into<Cow<'n, str>>) -> Self {
    Self::new_without_new_fn(name)
  }
}

impl<'py, 'n, T: 'static> PyTypeBuilder<'py, 'n, T> {
  pub fn new(name: impl Into<Cow<'n, str>>, new_fn: Box<NewFn<T>>) -> Self {
    // SAFETY: new_fn is set right after this call
    let mut this = unsafe { PyTypeBuilder::new_without_new_fn_unsafe(name) };
    this.new_fn(new_fn);
    this
  }

  /// # Panic
  /// Panics if `T` is not a ZST or if it imeplemnts [`Drop`]
  pub fn new_without_new_fn(name: impl Into<Cow<'n, str>>) -> Self {
    assert_eq!(
      mem::size_of::<T>(),
      0,
      "new_without_new_fn can only be called for ZST"
    );
    assert!(
      !mem::needs_drop::<T>(),
      "new_without_new_fn cannot be called for `Drop` types"
    );
    // SAFETY: ZST does not need to be initialized
    unsafe { Self::new_without_new_fn_unsafe(name) }
  }

  /// # Safety
  /// Caller must ensure that no type object is used while the type data is not initialized
  ///
  /// There are two ways to accomplish this
  /// - set the new_fn by calling [`Self::new_fn`] before [`Self::build`]
  /// - manually set the type data for every instance of the new python type
  pub unsafe fn new_without_new_fn_unsafe(
    name: impl Into<Cow<'n, str>>,
  ) -> Self {
    PyTypeBuilder {
      flags: (Py_TPFLAGS_DEFAULT | Py_TPFLAGS_HEAPTYPE),
      module: None,
      name: name.into(),
      metaclass: None,
      bases: Vec::new(),
      new_fn: None,
      init_fn: None,
    }
  }

  pub fn bases(
    &mut self,
    bases: impl IntoIterator<Item = Bound<'py, PyType>>,
  ) -> PyResult<&mut Self> {
    let bases = bases.into_iter();
    self.bases.reserve_exact(bases.size_hint().0);
    for base in bases {
      if base.is_subclass(&PyType::type_object(base.py()))? {
        return Err(PySystemError::new_err(
          "pyo3_runtime_type may not use the type builder to extend `type`, \
            use the `new_metaclass` function if you intend to make a metaclass",
        ));
      }
      self.bases.push(base);
    }
    Ok(self)
  }

  pub fn module(&mut self, module: Bound<'py, PyModule>) -> &mut Self {
    self.module = Some(module);
    self
  }

  pub fn new_fn(&mut self, new_fn: Box<NewFn<T>>) -> &mut Self {
    self.new_fn = Some(new_fn);
    self
  }

  pub fn init_fn(&mut self, init_fn: Box<InitFn<T>>) -> &mut Self {
    self.init_fn = Some(init_fn);
    self
  }

  pub fn build(self, py: Python<'py>) -> PyResult<Bound<'py, PyType>> {
    let spec = self.spec()?;

    let mut bases = self.bases;
    let bases = match bases.len() {
      0 => PyAny::type_object(py).into_any(),
      1 => bases.remove(0).into_any(),
      _ => PyTuple::new(py, &bases)?.into_any(),
    };

    let rtt = RuntimeTypeObject::new(self.new_fn, self.init_fn);
    let module = self.module.as_ref().map(|m| m.as_borrowed());

    // SAFETY: we just created a valid `spec` and all the pointers it
    //         contains point to things still in scope
    unsafe { rtt.make_type(self.metaclass, spec, bases.as_borrowed(), module) }
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

    if self.new_fn.is_some() {
      spec.push_slot(Py_tp_new, tp::new::<T> as newfunc as *mut c_void);
    }
    if mem::needs_drop::<T>() {
      spec.push_slot(
        Py_tp_dealloc,
        tp::dealloc::<T> as destructor as *mut c_void,
      );
    }
    if self.init_fn.is_some() {
      spec.push_slot(Py_tp_init, tp::init::<T> as initproc as *mut c_void);
    }

    Ok(spec)
  }
}

pub type NewFn<T> = dyn for<'py> Fn(
  Bound<'py, PyType>,
  Bound<'py, PyTuple>,
  Option<Bound<'py, PyDict>>,
) -> PyResult<T>;

pub type InitFn<T> = dyn for<'py> Fn(
  &T,
  Bound<'py, PyType>,
  Bound<'py, PyTuple>,
  Option<Bound<'py, PyDict>>,
) -> PyResult<()>;

pub struct Metaclass<T: 'static> {
  py_type: Py<PyType>,
  _marker: PhantomData<fn() -> T>,
}

struct MetaclassWithData<'py> {
  py_type: Bound<'py, PyType>,
  data: Option<MovingData>,
}

impl<T: 'static> Metaclass<T> {
  pub fn new<'a>(
    py: Python<'_>,
    name: impl Into<Cow<'a, str>>,
  ) -> PyResult<Self> {
    let mut builder = PyTypeBuilder::new_empty(name);
    builder.bases.push(RuntimeTypeObject::type_object(py));
    builder.flags |= Py_TPFLAGS_TYPE_SUBCLASS | Py_TPFLAGS_HEAPTYPE;
    let ty = builder.build(py)?;
    Ok(Self {
      py_type: ty.unbind(),
      _marker: PhantomData,
    })
  }

  pub fn builder<'a, 'py, U: 'static>(
    &self,
    py: Python<'py>,
    name: impl Into<Cow<'a, str>>,
    data: T,
    new_fn: Box<NewFn<U>>,
  ) -> PyTypeBuilder<'py, 'a, U> {
    let mut builder = PyTypeBuilder::new(name, new_fn);
    builder.metaclass = Some(MetaclassWithData {
      py_type: self.py_type.bind(py).clone(),
      data: (mem::size_of::<T>() > 0).then(|| MovingData::new(data)),
    });
    builder
  }

  /// # Safety
  /// The type object must not be instantiated
  pub unsafe fn as_type_obj<'py>(&self, py: Python<'py>) -> Bound<'py, PyType> {
    self.py_type.bind(py).clone()
  }
}

/// Run arbitrary code while preserving the exception state. Any exceptions
/// raised in the closure will be written to Python's unraisable hook.
fn no_exceptions<R>(py: Python<'_>, f: impl FnOnce() -> R) -> R {
  let exc = PyErr::take(py);
  let r = f();
  if let Some(new_exc) = PyErr::take(py) {
    new_exc.write_unraisable(py, None);
  }
  if let Some(exc) = exc {
    exc.restore(py);
  }
  r
}
