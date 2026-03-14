#[cfg(not(Py_3_10))]
compile_error!("min python version: 3.10");

use std::ffi::{CString, c_int, c_void};
use std::marker::PhantomData;
use std::mem;

use pyo3::PyTypeInfo as _;
use pyo3::exceptions::PySystemError;
use pyo3::ffi::{
  Py_TPFLAGS_DEFAULT, Py_TPFLAGS_HAVE_GC, Py_TPFLAGS_HEAPTYPE,
  Py_TPFLAGS_TYPE_SUBCLASS, Py_tp_call, Py_tp_clear, Py_tp_dealloc, Py_tp_init,
  Py_tp_new, Py_tp_traverse, PyTypeObject, destructor, initproc, inquiry,
  newfunc, ternaryfunc, traverseproc,
};
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyTuple, PyType};

use self::type_erased::MovingData;
use self::typeobject::RuntimeTypeObject;
use self::typespec::TypeSpec;

mod data_ptr;
mod ext;
mod tp;
mod type_erased;
mod typeobject;
mod typespec;

pub mod prelude {
  pub use crate::ext::*;
}

pub struct PyTypeBuilder<'py, T: Send + Sync + 'static> {
  module: Bound<'py, PyModule>,
  add_to_module: bool,
  metaclass: Option<MetaclassWithData<'py>>,
  bases: Vec<Bound<'py, PyType>>,
  new_fn: Option<Box<NewFn<T>>>,
  init_fn: Option<Box<InitFn<T>>>,
  call_fn: Option<Box<CallFn<T>>>,
  spec: TypeSpec,
}

impl<'py> PyTypeBuilder<'py, ()> {
  pub fn new_empty(name: &str, module: Bound<'py, PyModule>) -> PyResult<Self> {
    Self::new_without_new_fn(name, module)
  }
}

impl<'py, T: Send + Sync + 'static> PyTypeBuilder<'py, T> {
  pub fn new(
    name: &str,
    module: Bound<'py, PyModule>,
    new_fn: Box<NewFn<T>>,
  ) -> PyResult<Self> {
    // SAFETY: new_fn is set right after this call
    let mut this =
      unsafe { PyTypeBuilder::new_without_new_fn_unsafe(name, module) }?;
    this.new_fn(new_fn);
    Ok(this)
  }

  /// # Panic
  /// Panics if `T` is not a ZST or if it imeplemnts [`Drop`]
  pub fn new_without_new_fn(
    name: &str,
    module: Bound<'py, PyModule>,
  ) -> PyResult<Self> {
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
    unsafe { Self::new_without_new_fn_unsafe(name, module) }
  }

  /// # Safety
  /// Caller must ensure that no type object is used while the type data is not initialized
  ///
  /// There are two ways to accomplish this
  /// - set the new_fn by calling [`Self::new_fn`] before [`Self::build`]
  /// - manually set the type data for every instance of the new python type
  pub unsafe fn new_without_new_fn_unsafe(
    // name: impl Into<Cow<'n, str>>,
    name: &str,
    module: Bound<'py, PyModule>,
  ) -> PyResult<Self> {
    let name = format!("{}.{name}", module.name()?.to_str()?);
    let name = CString::new(name).unwrap();

    Ok(PyTypeBuilder {
      module,
      add_to_module: true,
      spec: TypeSpec::new(
        name,
        -c_int::try_from(mem::size_of::<T>()).unwrap(),
        0,
        (Py_TPFLAGS_DEFAULT | Py_TPFLAGS_HEAPTYPE | Py_TPFLAGS_HAVE_GC) as _,
      ),
      metaclass: None,
      bases: Vec::new(),
      new_fn: None,
      init_fn: None,
      call_fn: None,
    })
  }

  pub fn hide_from_module(&mut self, hide: bool) -> &mut Self {
    self.add_to_module = !hide;
    self
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
            use the `Metaclass` rust type if you intend to make a metaclass",
        ));
      }
      self.bases.push(base);
    }
    Ok(self)
  }

  pub fn new_fn(&mut self, new_fn: Box<NewFn<T>>) -> &mut Self {
    self.new_fn = Some(new_fn);
    self
  }

  pub fn init_fn(&mut self, init_fn: Box<InitFn<T>>) -> &mut Self {
    self.init_fn = Some(init_fn);
    self
  }

  pub fn call_fn(&mut self, call_fn: Box<CallFn<T>>) -> &mut Self {
    self.call_fn = Some(call_fn);
    self
  }

  pub fn build(mut self, py: Python<'py>) -> PyResult<Bound<'py, PyType>> {
    self.build_spec();

    let mut bases = self.bases;
    let bases = match bases.len() {
      0 => PyAny::type_object(py).into_any(),
      1 => bases.remove(0).into_any(),
      _ => PyTuple::new(py, &bases)?.into_any(),
    };

    let rtt = RuntimeTypeObject::new(self.new_fn, self.init_fn, self.call_fn);

    // SAFETY: we just created a valid `spec` and all the pointers it
    //         contains point to things still in scope
    let ty = unsafe {
      rtt.make_type(
        self.metaclass,
        self.spec,
        bases.as_borrowed(),
        self.module.as_borrowed(),
      )
    }?;

    if self.add_to_module {
      self.module.add(ty.name()?, &ty)?;
    }

    Ok(ty)
  }

  fn build_spec(&mut self) {
    if self.new_fn.is_some() {
      self
        .spec
        .push_slot(Py_tp_new, tp::new::<T> as newfunc as *mut c_void);
    }
    if self.init_fn.is_some() {
      self
        .spec
        .push_slot(Py_tp_init, tp::init::<T> as initproc as *mut c_void);
    }
    if mem::needs_drop::<T>() {
      self.spec.push_slot(
        Py_tp_dealloc,
        tp::dealloc::<T> as destructor as *mut c_void,
      );
    }

    if self.call_fn.is_some() {
      self
        .spec
        .push_slot(Py_tp_call, tp::call::<T> as ternaryfunc as *mut c_void);
    }

    self
      .spec
      .push_slot(Py_tp_traverse, tp::traverse as traverseproc as *mut c_void);
    self
      .spec
      .push_slot(Py_tp_clear, tp::clear as inquiry as *mut c_void);
  }
}

pub type NewFn<T> = dyn for<'py> Fn(
    Bound<'py, PyType>,
    Bound<'py, PyTuple>,
    Option<Bound<'py, PyDict>>,
  ) -> PyResult<T>
  + Send
  + Sync
  + 'static;

pub type InitFn<T> = dyn for<'py> Fn(
    &T,
    Bound<'py, PyType>,
    Bound<'py, PyTuple>,
    Option<Bound<'py, PyDict>>,
  ) -> PyResult<()>
  + Send
  + Sync
  + 'static;

pub type CallFn<T> = dyn for<'py> Fn(
    &T,
    Bound<'py, PyType>,
    Bound<'py, PyTuple>,
    Option<Bound<'py, PyDict>>,
  ) -> PyResult<Bound<'py, PyAny>>
  + Send
  + Sync
  + 'static;

pub struct Metaclass<T: Send + Sync + 'static> {
  py_type: Py<PyType>,
  _marker: PhantomData<fn() -> T>,
}

struct MetaclassWithData<'py> {
  py_type: Bound<'py, PyType>,
  data: Option<MovingData>,
}

impl<T: Send + Sync + 'static> Metaclass<T> {
  pub fn new(
    name: &str,
    module: Bound<'_, PyModule>,
    add_to_module: bool,
  ) -> PyResult<Self> {
    let py = module.py();

    // SAFETY: the `builder` method takes an instance of `T` and uses that
    let mut builder =
      unsafe { PyTypeBuilder::<T>::new_without_new_fn_unsafe(name, module) }?;
    builder.bases.push(RuntimeTypeObject::type_object(py));
    builder
      .spec
      .add_flags(Py_TPFLAGS_TYPE_SUBCLASS | Py_TPFLAGS_HEAPTYPE);
    builder.add_to_module = add_to_module;
    let ty = builder.build(py)?;
    Ok(Self {
      py_type: ty.unbind(),
      _marker: PhantomData,
    })
  }

  pub fn builder<'py, U: Send + Sync + 'static>(
    &self,
    name: &str,
    module: Bound<'py, PyModule>,
    data: T,
    new_fn: Box<NewFn<U>>,
  ) -> PyResult<PyTypeBuilder<'py, U>> {
    let py = module.py();

    let mut builder = PyTypeBuilder::new(name, module, new_fn)?;
    builder.metaclass = Some(MetaclassWithData {
      py_type: self.py_type.bind(py).clone(),
      data: (mem::size_of::<T>() > 0).then(|| MovingData::new(data)),
    });
    Ok(builder)
  }

  /// # Safety
  /// The type object must not be instantiated
  pub unsafe fn as_type_obj<'py>(&self, py: Python<'py>) -> Bound<'py, PyType> {
    self.py_type.bind(py).clone()
  }

  /// # Safety
  /// The type object must not be instantiated
  pub unsafe fn as_type_obj_borrowed<'py>(
    &self,
    py: Python<'py>,
  ) -> Borrowed<'_, 'py, PyType> {
    self.py_type.bind_borrowed(py)
  }

  /// # Safety
  /// The type object must not be instantiated
  pub unsafe fn as_type_ptr(&self) -> *mut PyTypeObject {
    self.py_type.as_ptr().cast()
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
