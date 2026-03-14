//! This module contains the metaclass for the python types we create.

use std::any::TypeId;
use std::ffi::c_ulong;
use std::mem;
use std::ptr::{self, NonNull};
use std::sync::OnceLock;

use pyo3::exceptions::PyTypeError;
use pyo3::ffi::{
  Py_DECREF, Py_TPFLAGS_BASETYPE, Py_TPFLAGS_DEFAULT,
  Py_TPFLAGS_DISALLOW_INSTANTIATION, Py_TPFLAGS_HEAPTYPE,
  Py_TPFLAGS_TYPE_SUBCLASS, PyObject, PyObject_HEAD_INIT, PyType_FromMetaclass,
  PyType_FromSpec, PyType_Ready, PyType_Type, PyTypeObject, PyVarObject,
  destructor,
};
use pyo3::prelude::*;
use pyo3::py_format;
use pyo3::sync::OnceLockExt as _;
use pyo3::type_object::PyTypeInfo;
use pyo3::types::PyType;

use crate::data_ptr::{type_data, type_data_ptr};
use crate::type_erased::ErasedTraitObjects;
use crate::typespec::TypeSpec;
use crate::{CallFn, InitFn, MetaclassWithData, NewFn};

pub(crate) struct RuntimeTypeObject {
  new_fn: ErasedTraitObjects<1>,
  init_fn: ErasedTraitObjects<1>,
  call_fn: ErasedTraitObjects<1>,
  type_id: TypeId,
}

// SAFETY: `type_object_raw` always returns the same pointer
unsafe impl PyTypeInfo for RuntimeTypeObject {
  const NAME: &str = "pyo3_runtime_type";
  const MODULE: Option<&str> = Some("__hidden__");

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

impl RuntimeTypeObject {
  pub(crate) fn new<T: Send + Sync + 'static>(
    new_fn: Option<Box<NewFn<T>>>,
    init_fn: Option<Box<InitFn<T>>>,
    call_fn: Option<Box<CallFn<T>>>,
  ) -> Self {
    Self {
      new_fn: ErasedTraitObjects::new([new_fn]),
      init_fn: ErasedTraitObjects::new([init_fn]),
      call_fn: ErasedTraitObjects::new([call_fn]),
      type_id: TypeId::of::<T>(),
    }
  }

  /// # Safety
  /// - The `spec` must be valid for the `T` that `self` was contructed with.
  /// - The `metaclass` must hold an instance of the `T` that `self` was constructed with.
  pub(crate) unsafe fn make_type<'py>(
    self,
    metaclass: Option<MetaclassWithData>,
    mut spec: TypeSpec,
    bases: Borrowed<'_, 'py, PyAny>,
    module: Borrowed<'_, 'py, PyModule>,
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
        module.as_ptr(),
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

  pub(crate) fn new_fn<T: Send + Sync + 'static>(&self) -> Option<&NewFn<T>> {
    self.new_fn.typed().unwrap().get(0)
  }

  pub(crate) fn init_fn<T: Send + Sync + 'static>(&self) -> Option<&InitFn<T>> {
    self.init_fn.typed().unwrap().get(0)
  }

  pub(crate) fn call_fn<T: Send + Sync + 'static>(&self) -> Option<&CallFn<T>> {
    self.call_fn.typed().unwrap().get(0)
  }

  pub(crate) fn get_data<'a, T: Send + Sync + 'static>(
    &self,
    obj: Borrowed<'a, '_, PyAny>,
  ) -> PyResult<&'a T> {
    if self.type_id == TypeId::of::<T>() {
      // SAFETY: we just confirmed that it's the correct type
      unsafe { type_data(obj) }
    } else {
      todo!()
    }
  }
}

fn subtype_dealloc(py: Python<'_>) -> destructor {
  static SUBTYPE_DEALLOC: OnceLock<destructor> = OnceLock::new();
  *SUBTYPE_DEALLOC.get_or_init_py_attached(py, || {
    let mut spec = TypeSpec::new(
      c"__hidden__.__dummy__".to_owned(),
      0,
      0,
      (Py_TPFLAGS_DEFAULT | Py_TPFLAGS_HEAPTYPE) as _,
    );

    // SAFETY: this is probably fine
    unsafe {
      let ty = PyType_FromSpec(spec.finish()).cast::<PyTypeObject>();
      let dealloc = ptr::addr_of!((*ty).tp_dealloc).read();
      Py_DECREF(ty.cast());
      dealloc.unwrap()
    }
  })
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
  /// `slf` must be a pointer to a [`RuntimeTypeWithBase`] and python must be
  ///  in attached state
  unsafe extern "C" fn destroy(slf: *mut PyObject) {
    #[cfg(test)]
    #[expect(clippy::disallowed_macros, reason = "tests")]
    {
      eprintln!("destroying RuntimeTypeWithBase");
    }

    // SAFETY: caller upholds requirements
    unsafe {
      let slf = slf.cast::<Self>();
      let p = ptr::addr_of_mut!((*slf).runtime_type);
      ptr::drop_in_place(p);
    }

    // SAFETY: caller upholds requirements
    let py = unsafe { Python::assume_attached() };

    // SAFETY: `slf` is known to be a `PyTypeObject`
    unsafe {
      // let base_dealloc = PyType_Type.tp_dealloc.unwrap();
      // base_dealloc(slf);
      (subtype_dealloc(py))(slf);
    }
  }
}

static mut RUNTIME_TYPE_TYPE: PyTypeObject = PyTypeObject {
  tp_name: c"__hidden__.pyo3_runtime_type".as_ptr(),
  tp_base: &raw mut PyType_Type,
  tp_dealloc: Some(RuntimeTypeWithBase::destroy as destructor),
  tp_basicsize: mem::size_of::<RuntimeTypeWithBase>() as pyo3::ffi::Py_ssize_t,
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
