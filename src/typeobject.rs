//! This module contains the metaclass for the python types we create.

use std::ffi::c_ulong;
use std::ptr::NonNull;
use std::{mem, ptr};

use pyo3::exceptions::PyTypeError;
use pyo3::ffi::{
  Py_TPFLAGS_BASETYPE, Py_TPFLAGS_DEFAULT, Py_TPFLAGS_DISALLOW_INSTANTIATION,
  Py_TPFLAGS_TYPE_SUBCLASS, PyObject, PyObject_HEAD_INIT, PyType_FromMetaclass,
  PyType_Ready, PyTypeObject, PyVarObject, destructor,
};
use pyo3::prelude::*;
use pyo3::py_format;
use pyo3::type_object::PyTypeInfo;
use pyo3::types::PyType;

use crate::data_ptr::type_data_ptr;
use crate::typespec::TypeSpec;
use crate::{InitFn, MetaclassWithData, NewFn};

pub(crate) struct RuntimeTypeObject {
  new_fn: [*mut (); 2],
  new_fn_drop: *mut (),
  init_fn: [*mut (); 2],
  init_fn_drop: *mut (),
}

// SAFETY: `type_object_raw` always returns the same pointer
unsafe impl PyTypeInfo for RuntimeTypeObject {
  const NAME: &str = "pyo3_runtime_type";
  const MODULE: Option<&str> = None;

  fn type_object_raw(_py: Python<'_>) -> *mut PyTypeObject {
    &raw mut RUNTIME_TYPE_TYPE
  }
}

impl<'a, 'py> FromPyObject<'a, 'py> for &'a RuntimeTypeObject {
  type Error = PyErr;

  fn extract(obj: Borrowed<'a, 'py, PyAny>) -> Result<Self, Self::Error> {
    if obj.is_instance_of::<RuntimeTypeObject>() {
      let with_base = obj.as_ptr().cast::<RuntimeTypeWithBase>();
      // SAFETY: we just checked if it's the right type
      unsafe { Ok(&*ptr::addr_of!((*with_base).runtime_type)) }
    } else {
      let py = obj.py();
      Err(PyTypeError::new_err(
        py_format!(
          py,
          "expected type to be an instance of {} metaclass",
          RuntimeTypeObject::type_object(py).name()?
        )?
        .unbind(),
      ))
    }
  }
}

impl Drop for RuntimeTypeObject {
  fn drop(&mut self) {
    // SAFETY: all of these pointers are set in the constructor
    unsafe {
      let new_fn_drop =
        mem::transmute::<*mut (), fn([*mut (); 2])>(self.new_fn_drop);
      new_fn_drop(self.new_fn);

      let init_fn_drop =
        mem::transmute::<*mut (), fn([*mut (); 2])>(self.init_fn_drop);
      init_fn_drop(self.init_fn);
    }
  }
}

impl RuntimeTypeObject {
  pub(crate) fn new<T>(
    new_fn: Option<Box<NewFn<T>>>,
    init_fn: Option<Box<InitFn<T>>>,
  ) -> Self {
    fn new_fn_drop<T>(new_fn: [*mut (); 2]) {
      // SAFETY: undoing transmute below
      let _ = unsafe {
        mem::transmute::<[*mut (); 2], Option<Box<NewFn<T>>>>(new_fn)
      };
    }
    fn init_fn_drop<T>(init_fn: [*mut (); 2]) {
      // SAFETY: undoing transmute below
      let _ = unsafe {
        mem::transmute::<[*mut (); 2], Option<Box<InitFn<T>>>>(init_fn)
      };
    }

    // SAFETY: these function pointers will only be accessed from the getter functions
    unsafe {
      Self {
        new_fn: mem::transmute::<Option<Box<NewFn<T>>>, [*mut (); 2]>(new_fn),
        new_fn_drop: new_fn_drop::<T> as *mut (),
        init_fn: mem::transmute::<Option<Box<InitFn<T>>>, [*mut (); 2]>(
          init_fn,
        ),
        init_fn_drop: init_fn_drop::<T> as *mut (),
      }
    }
  }

  /// # Safety
  /// `spec` must be valid for the python API and `metaclass` must have been
  /// constructed with the rust type for the type's type-data
  pub(crate) unsafe fn make_type<'py>(
    self,
    metaclass: Option<MetaclassWithData>,
    mut spec: TypeSpec,
    bases: Borrowed<'_, 'py, PyAny>,
    module: Option<Borrowed<'_, 'py, PyModule>>,
  ) -> PyResult<Bound<'py, PyType>> {
    let py = bases.py();

    Self::ready(py)?;

    let (metaclass, metaclass_data) = metaclass.map_or_else(
      || (Self::type_object(py), None),
      |mc| (mc.py_type, mc.data),
    );

    // SAFETY: all the pointers refer to objects in this scope
    let Some(ty) = (unsafe {
      NonNull::new(PyType_FromMetaclass(
        metaclass.as_type_ptr(),
        module.map(Borrowed::as_ptr).unwrap_or_default(),
        spec.finish(),
        bases.as_ptr(),
      ))
    }) else {
      return Err(PyErr::fetch(py));
    };
    // SAFETY: python API returns a valid owned reference to a type object
    let ty = unsafe {
      Bound::from_owned_ptr(py, ty.as_ptr()).cast_into_unchecked::<PyType>()
    };
    RuntimeTypeWithBase::setup(ty.as_borrowed(), self);

    if let Some(metaclass_data) = metaclass_data {
      let tp_data = type_data_ptr::<()>(ty.as_any().as_borrowed()).unwrap();
      // SAFETY: caller ensures that the pointers are the same type
      unsafe { metaclass_data.move_it(tp_data) };
    }

    if let Some(module) = module {
      module.add(spec.name(), &ty)?;
    }

    Ok(ty)
  }

  fn ready(py: Python<'_>) -> PyResult<()> {
    // SAFETY: calling PyType_Ready with valid static type object
    if unsafe { PyType_Ready(&raw mut RUNTIME_TYPE_TYPE) } == 0 {
      Ok(())
    } else {
      Err(PyErr::fetch(py))
    }
  }

  /// # Safety
  /// `self` must have been constructed as `T`
  pub(crate) unsafe fn new_fn<T>(&self) -> Option<&NewFn<T>> {
    // SAFETY: new_fn is set in `new` and caller ensures that `T` is correct
    unsafe { &*(&raw const self.new_fn as *const Option<Box<NewFn<T>>>) }
      .as_deref()
  }

  /// # Safety
  /// `self` must have been constructed as `T`
  pub(crate) unsafe fn init_fn<T>(&self) -> Option<&InitFn<T>> {
    // SAFETY: init_fn is set in `new` and caller ensures that `T` is correct
    unsafe { &*(&raw const self.init_fn as *const Option<Box<InitFn<T>>>) }
      .as_deref()
  }
}

#[repr(C)]
struct RuntimeTypeWithBase {
  _ob_base: PyTypeObject,
  runtime_type: RuntimeTypeObject,
}

impl RuntimeTypeWithBase {
  fn setup(slf: Borrowed<'_, '_, PyType>, runtime_type: RuntimeTypeObject) {
    assert!(
      slf.is_instance_of::<RuntimeTypeObject>(),
      "called `RuntimeTypeWithBase::setup` with typeobject that isn't a RuntimeTypeObject"
    );
    let slf = slf.as_type_ptr().cast::<Self>();
    // SAFETY: we just asserted that `slf` was created with the correct type object
    unsafe {
      ptr::addr_of_mut!((*slf).runtime_type).write(runtime_type);
    }
  }

  /// # Safety
  /// `slf` must be a pointer to a [`RuntimeTypeWithBase`]
  unsafe extern "C" fn destroy(slf: *mut PyObject) {
    // SAFETY: caller upholds requirements
    unsafe {
      let slf = slf.cast::<Self>();
      let p = ptr::addr_of_mut!((*slf).runtime_type);
      ptr::drop_in_place(p);
    }
  }
}

static mut RUNTIME_TYPE_TYPE: PyTypeObject = PyTypeObject {
  tp_name: c"pyo3_runtime_type".as_ptr(),
  tp_base: &raw mut pyo3::ffi::PyType_Type,
  tp_finalize: Some(RuntimeTypeWithBase::destroy as destructor),
  tp_basicsize: mem::size_of::<RuntimeTypeWithBase>() as pyo3::ffi::Py_ssize_t,
  tp_itemsize: 1,
  #[cfg(not(Py_GIL_DISABLED))]
  tp_flags: runtime_type_flags() as _,
  #[cfg(Py_GIL_DISABLED)]
  tp_flags: std::sync::atomic::AtomicU64::new(runtime_type_flags()),
  ..empty_type_obj()
};

const fn runtime_type_flags() -> c_ulong {
  Py_TPFLAGS_DEFAULT
    | Py_TPFLAGS_TYPE_SUBCLASS
    | Py_TPFLAGS_DISALLOW_INSTANTIATION
    | Py_TPFLAGS_BASETYPE
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
