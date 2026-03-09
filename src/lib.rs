use std::borrow::Cow;
use std::ffi::{CString, c_int, c_ulong, c_void};
use std::mem;
use std::ptr::{self, NonNull};

use pyo3::PyTypeInfo as _;
use pyo3::exceptions::PySystemError;
use pyo3::ffi::{
  Py_TPFLAGS_DEFAULT, Py_TPFLAGS_DISALLOW_INSTANTIATION, Py_TPFLAGS_HEAPTYPE,
  Py_TPFLAGS_TYPE_SUBCLASS, Py_tp_finalize, Py_tp_init, Py_tp_new, PyObject,
  PyObject_HEAD_INIT, PyType_FromMetaclass, PyType_GenericNew, PyType_Ready,
  PyType_Slot, PyType_Spec, PyTypeObject, PyVarObject, destructor, initproc,
  newfunc,
};
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyString, PyTuple, PyType};

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
    let mut spec = PyType_Spec {
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

    // SAFETY: pointer refers to a valid type object
    unsafe {
      if PyType_Ready(&raw mut META_CLASS_TYPE) != 0 {
        return Err(PyErr::fetch(py));
      }
    }
    // SAFETY: all the pointers refer to objects in this scope
    let Some(ty) = (unsafe {
      NonNull::new(PyType_FromMetaclass(
        &raw mut META_CLASS_TYPE,
        self.module.as_ref().map(Bound::as_ptr).unwrap_or_default(),
        &raw mut spec,
        bases.as_ptr(),
      ))
    }) else {
      return Err(PyErr::fetch(py));
    };
    // SAFETY: `ty` was just created using `META_CLASS_TYPE`
    unsafe {
      MetaClass::setup(ty.cast(), self.new_fn, self.init_fn);
    }
    // SAFETY: python API returns a valid owned reference to a type object
    let ty = unsafe {
      Bound::from_owned_ptr(py, ty.as_ptr()).cast_into_unchecked::<PyType>()
    };

    if let Some(module) = &self.module {
      module.add(&self.name, &ty)?;
    }

    Ok(ty)
  }

  fn slots(&self) -> Vec<PyType_Slot> {
    let mut slots = vec![PyType_Slot {
      slot: Py_tp_new,
      pfunc: tp_new::<T> as newfunc as *mut c_void,
    }];
    if mem::needs_drop::<T>() {
      slots.push(PyType_Slot {
        slot: Py_tp_finalize,
        pfunc: tp_finalize::<T> as destructor as *mut c_void,
      });
    }
    if self.init_fn.is_some() {
      slots.push(PyType_Slot {
        slot: Py_tp_init,
        pfunc: tp_init::<T> as initproc as *mut c_void,
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

#[repr(C)]
struct MetaClass {
  _ob_base: PyTypeObject,
  new_fn: NonNull<()>,
  init_fn: Option<NonNull<()>>,
}

const _: () = assert!(
  !mem::needs_drop::<MetaClass>(),
  "MetaClass's Drop will never be called"
);

impl MetaClass {
  /// # Safety
  /// `slf` must be a valid type object at the head of a [`MetaClass`]
  unsafe fn setup<T>(
    slf: NonNull<PyTypeObject>,
    new_fn: NewFn<T>,
    init_fn: Option<InitFn<T>>,
  ) {
    let slf = slf.cast::<Self>();
    // SAFETY: caller upholds requirements
    unsafe {
      ptr::addr_of_mut!((*slf.as_ptr()).new_fn)
        .write(NonNull::new_unchecked(new_fn as *mut ()));
      ptr::addr_of_mut!((*slf.as_ptr()).init_fn)
        .write(init_fn.map(|f| NonNull::new_unchecked(f as *mut ())));
    }
  }

  /// # Safety
  /// The `ty` must have been created as an instance of [`MetaClass`]
  unsafe fn get<'a>(ty: Borrowed<'a, '_, PyType>) -> &'a Self {
    // SAFETY: caller ensures the pointer was written to with a valid instance
    unsafe { &*ty.as_type_ptr().cast() }
  }

  /// # Safety
  /// `self` must have been setup with `T`
  unsafe fn new_fn<T>(&self) -> NewFn<T> {
    // SAFETY: new_fn is set in `setup` and caller ensures that `T` is correct
    unsafe { mem::transmute(self.new_fn) }
  }

  /// # Safety
  /// `self` must have been setup with `T`
  unsafe fn init_fn<T>(&self) -> Option<InitFn<T>> {
    // SAFETY: init_fn is set in `setup` and caller ensures that `T` is correct
    self
      .init_fn
      .map(|init_fn| unsafe { mem::transmute(init_fn) })
  }
}

/// # Safety
/// Must be called in `tp_new` slot of type created with `MetaClass` as type data
unsafe extern "C" fn tp_new<T>(
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
  // SAFETY: caller upholds requirements
  let metaclass = unsafe { MetaClass::get(ty.as_borrowed()) };

  // SAFETY: `Metaclass::setup` stores this fn's ptr with the correct `T`
  let new_fn = unsafe { metaclass.new_fn::<T>() };

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

unsafe extern "C" fn tp_init<T>(
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
    // SAFETY: caller upholds requirements
    let metaclass = unsafe { MetaClass::get(ty.as_borrowed()) };
    // SAFETY: `Metaclass::setup` stores this fn's ptr with the correct `T`
    let init_fn = unsafe { metaclass.init_fn::<T>() }.ok_or_else(|| {
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
/// The `obj` must have been created with [`tp_new`]
unsafe extern "C" fn tp_finalize<T>(obj: *mut PyObject) {
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

static mut META_CLASS_TYPE: PyTypeObject = PyTypeObject {
  tp_name: c"pyo3_runtime_types_metaclass".as_ptr(),
  tp_base: &raw mut pyo3::ffi::PyType_Type,
  tp_basicsize: mem::size_of::<MetaClass>() as pyo3::ffi::Py_ssize_t,
  #[cfg(not(Py_GIL_DISABLED))]
  tp_flags: metaclass_flags() as _,
  #[cfg(Py_GIL_DISABLED)]
  tp_flags: std::sync::atomic::AtomicU64::new(metaclass_flags()),
  tp_dictoffset: -1,
  ..empty_type_obj()
};

const fn metaclass_flags() -> c_ulong {
  Py_TPFLAGS_DEFAULT
    | Py_TPFLAGS_TYPE_SUBCLASS
    | Py_TPFLAGS_DISALLOW_INSTANTIATION
}

const fn empty_type_obj() -> PyTypeObject {
  PyTypeObject {
    ob_base: PyVarObject {
      ob_base: PyObject_HEAD_INIT,
      #[cfg(not(GraalPy))]
      ob_size: 0,
      #[cfg(GraalPy)]
      _ob_size_graalpy: 0,
    },
    tp_name: ptr::null_mut(),
    tp_basicsize: 0,
    tp_itemsize: 0,
    tp_dealloc: None,
    #[cfg(not(Py_3_8))]
    tp_print: None,
    #[cfg(Py_3_8)]
    tp_vectorcall_offset: 0,
    tp_getattr: None,
    tp_setattr: None,
    tp_as_async: ptr::null_mut(),
    tp_repr: None,
    tp_as_number: ptr::null_mut(),
    tp_as_sequence: ptr::null_mut(),
    tp_as_mapping: ptr::null_mut(),
    tp_hash: None,
    tp_call: None,
    tp_str: None,
    tp_getattro: None,
    tp_setattro: None,
    tp_as_buffer: ptr::null_mut(),
    #[cfg(not(Py_GIL_DISABLED))]
    tp_flags: Py_TPFLAGS_DEFAULT as _,
    #[cfg(Py_GIL_DISABLED)]
    tp_flags: std::sync::atomic::AtomicU64::new(Py_TPFLAGS_DEFAULT),
    tp_doc: ptr::null_mut(),
    tp_traverse: None,
    tp_clear: None,
    tp_richcompare: None,
    tp_weaklistoffset: 0,
    tp_iter: None,
    tp_iternext: None,
    tp_methods: ptr::null_mut(),
    tp_members: ptr::null_mut(),
    tp_getset: ptr::null_mut(),
    tp_base: ptr::null_mut(),
    tp_dict: ptr::null_mut(),
    tp_descr_get: None,
    tp_descr_set: None,
    tp_dictoffset: 0,
    tp_init: None,
    tp_alloc: None,
    tp_new: None,
    tp_free: None,
    tp_is_gc: None,
    tp_bases: ptr::null_mut(),
    tp_mro: ptr::null_mut(),
    tp_cache: ptr::null_mut(),
    tp_subclasses: ptr::null_mut(),
    tp_weaklist: ptr::null_mut(),
    tp_del: None,
    tp_version_tag: 0,
    tp_finalize: None,
    #[cfg(Py_3_8)]
    tp_vectorcall: None,
    #[cfg(Py_3_12)]
    tp_watched: 0,
    #[cfg(all(not(PyPy), Py_3_8, not(Py_3_9)))]
    tp_print: None,
    #[cfg(py_sys_config = "COUNT_ALLOCS")]
    tp_allocs: 0,
    #[cfg(py_sys_config = "COUNT_ALLOCS")]
    tp_frees: 0,
    #[cfg(py_sys_config = "COUNT_ALLOCS")]
    tp_maxalloc: 0,
    #[cfg(py_sys_config = "COUNT_ALLOCS")]
    tp_prev: ptr::null_mut(),
    #[cfg(py_sys_config = "COUNT_ALLOCS")]
    tp_next: ptr::null_mut(),
  }
}

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
