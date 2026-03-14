#![allow(
  unsafe_op_in_unsafe_fn,
  clippy::disallowed_macros,
  clippy::missing_assert_message,
  clippy::undocumented_unsafe_blocks,
  clippy::unnecessary_wraps,
  clippy::unused_self,
  reason = "tests"
)]

use std::ffi::c_int;
use std::iter;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use pyo3::PyTypeInfo as _;
use pyo3::exceptions::PySystemError;
use pyo3::ffi::{
  PyGC_Collect, PyGC_Disable, PyGC_Enable, PyObject_GC_IsTracked,
};
use pyo3::prelude::*;
use pyo3::types::{PyCFunction, PyType, PyWeakrefReference};
use pyo3_runtime_types::{Metaclass, PyTypeBuilder};

#[test]
fn obj_created_inited_and_destroyed() {
  py_wrapper(|py, module| {
    let factory = Arc::new(Factory::default());
    let mut builder = PyTypeBuilder::new("S", module, {
      let factory = factory.clone();
      Box::new(move |_, _, _| Ok(factory.make()))
    })
    .unwrap();
    builder
      .init_fn(Box::new(|slf, _, _, _| slf.__init__()))
      .hide_from_module(true);
    let ty = builder.build(py).unwrap();
    let obj = ty.call0().unwrap();

    eprintln!("deleting obj");
    drop(obj);
    gc_collect_force(py);
    factory.assert_inited_and_finalized();

    eprintln!("deleting ty");
    drop(ty);
    gc_collect_force(py);
  });
}

#[test]
fn build_metaclass_exception() {
  py_wrapper(|py, module| {
    let mut builder =
      PyTypeBuilder::new("dummy", module, Box::new(|_, _, _| Ok(42))).unwrap();
    let err = builder
      .bases(iter::once(PyType::type_object(py)))
      .err()
      .unwrap();
    assert!(err.matches(py, PySystemError::type_object(py)).unwrap());
  });
}

#[test]
fn obj_from_metaclass_created_inited_and_destroyed() {
  py_wrapper(|py, module| {
    let meta = Metaclass::new("Meta", module.clone(), true).unwrap();
    let factory = Arc::new(Factory::default());
    let mut builder = meta
      .builder("S", module, Meta { flag: Default::default() }, {
        let factory = factory.clone();
        Box::new(move |_, _, _| Ok(factory.make()))
      })
      .unwrap();
    builder.init_fn(Box::new(|slf, _, _, _| slf.__init__()));
    let ty = builder.build(py).unwrap();
    let obj = ty.call0().unwrap();

    drop(obj);
    gc_collect_force(py);
    factory.assert_inited_and_finalized();
  });
}

#[test]
#[ignore = "need to figure out why the gc thinks the types aren't trash"]
fn types_are_gc_collected() {
  py_wrapper(|py, module| {
    eprintln!("module refcnt at start: {}", module.get_refcnt());
    let meta = Metaclass::new("Meta", module.clone(), false).unwrap();
    eprintln!("created meta: {}", module.get_refcnt());

    let meta_factory = MetaFactory::default();
    let factory = Arc::new(Factory::default());
    let mut builder = meta
      .builder("S", module.clone(), meta_factory.make(), {
        let factory = factory.clone();
        Box::new(move |_, _, _| Ok(factory.make()))
      })
      .unwrap();
    eprintln!("created builder (with clone): {}", module.get_refcnt());
    builder.hide_from_module(true);

    builder.init_fn(Box::new(|slf, _, _, _| slf.__init__()));
    let ty = builder.build(py).unwrap();
    eprintln!("created type: {}", module.get_refcnt());
    let obj = ty.call0().unwrap();
    eprintln!("created object: {}", module.get_refcnt());
    eprintln!();

    // destroy the object
    {
      eprintln!("deleting obj refcnt={}", obj.get_refcnt());
      drop(obj);
      gc_collect_force(py);
      factory.assert_inited_and_finalized();
      eprintln!("module refcnt={}", module.get_refcnt());
      eprintln!();
    }

    // destroy the type
    {
      eprintln!("deleting ty refcnt={}", ty.get_refcnt());
      let ty_gc_check = WeakrefDropCheck::new(&ty);
      assert!(unsafe { PyObject_GC_IsTracked(ty.as_ptr()) != 0 });
      drop(ty);
      gc_collect_force(py);
      // NOTE: if the rust drop ran but it wasn't GCed, that's a soundness issue
      ty_gc_check.assert_garbage_collected();
      meta_factory.assert_dropped();
      eprintln!("module refcnt={}", module.get_refcnt());
      eprintln!();
    }

    // destroy the metatype
    {
      eprintln!(
        "deleting meta refcnt={}",
        unsafe { meta.as_type_obj(py) }.get_refcnt()
      );
      let meta_gc_check = {
        let ty = unsafe { meta.as_type_obj_borrowed(py) };
        WeakrefDropCheck::new(ty.as_any())
      };
      drop(meta);
      meta_gc_check.assert_garbage_collected();
    }
  });
}

#[derive(Default)]
struct Factory {
  initialized: Arc<AtomicBool>,
  dropped: Arc<AtomicBool>,
}

#[derive(Default)]
struct MetaFactory {
  flag: Arc<AtomicBool>,
}
impl MetaFactory {
  fn make(&self) -> Meta {
    Meta { flag: self.flag.clone() }
  }

  #[track_caller]
  fn assert_dropped(&self) {
    let ty_was_dropped = self.flag.load(Ordering::SeqCst);
    assert!(ty_was_dropped);
  }
}

struct Meta {
  flag: Arc<AtomicBool>,
}
impl Drop for Meta {
  fn drop(&mut self) {
    self.flag.store(true, Ordering::SeqCst);
  }
}

impl Factory {
  fn make(&self) -> S {
    S {
      initialized: self.initialized.clone(),
      dropped: self.dropped.clone(),
    }
  }

  #[track_caller]
  fn assert_inited_and_finalized(&self) {
    let initialized = self.initialized.load(Ordering::SeqCst);
    assert!(initialized);
    let dropped = self.dropped.load(Ordering::SeqCst);
    assert!(dropped);
  }
}

struct S {
  initialized: Arc<AtomicBool>,
  dropped: Arc<AtomicBool>,
}

impl S {
  fn __init__(&self) -> PyResult<()> {
    self.initialized.store(true, Ordering::SeqCst);
    Ok(())
  }
}

impl Drop for S {
  fn drop(&mut self) {
    self.dropped.store(true, Ordering::SeqCst);
  }
}

fn gc_collect_force(py: Python<'_>) {
  let previous_state = gc_enable(py);
  gc_collect(py);
  if !previous_state.enabled() {
    gc_disable(py);
  }
}

fn gc_collect(py: Python<'_>) {
  let get_counts = PyModule::import(py, "gc")
    .unwrap()
    .getattr("get_count")
    .unwrap();
  let prev = get_counts.call0().unwrap();
  eprintln!("running python gc...");
  let count = unsafe { PyGC_Collect() };
  eprintln!(
    "collected + uncollectable = {count}, prev={}, curr={}",
    prev.repr().unwrap(),
    get_counts.call0().unwrap().repr().unwrap()
  );
}

/// Returns the previous state
fn gc_disable(_py: Python<'_>) -> GcState {
  GcState(unsafe { PyGC_Disable() })
}

/// Returns the previous state
fn gc_enable(_py: Python<'_>) -> GcState {
  GcState(unsafe { PyGC_Enable() })
}

#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
struct GcState(c_int);

impl GcState {
  fn enabled(self) -> bool {
    self.0 == 1
  }
}

fn py_wrapper<R>(
  f: impl for<'py> FnOnce(Python<'py>, Bound<'py, PyModule>) -> R,
) -> R {
  Python::initialize();
  Python::attach(|py| {
    PyModule::import(py, "warnings")
      .unwrap()
      .getattr("simplefilter")
      .unwrap()
      .call1(("error",))
      .unwrap();
    let module = PyModule::new(py, "test_module").unwrap();
    gc_collect_force(py);
    gc_disable(py);
    eprintln!("using module: {}\n", module.name().unwrap());
    f(py, module)
  })
}

/// Helper type to assert that a python object was garbage collected
struct WeakrefDropCheck<'py> {
  #[expect(
    dead_code,
    reason = "prevent it from being dropped before its closure runs"
  )]
  weak: Bound<'py, PyWeakrefReference>,
  flag: Arc<AtomicBool>,
}

impl<'py> WeakrefDropCheck<'py> {
  fn new(obj: &Bound<'py, PyAny>) -> Self {
    let flag: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));
    let weak = {
      let flag = flag.clone();
      PyWeakrefReference::new_with(
        obj,
        PyCFunction::new_closure(obj.py(), None, None, move |_, _| {
          flag.store(true, Ordering::SeqCst);
        })
        .unwrap(),
      )
      .unwrap()
    };
    Self { weak, flag }
  }

  #[track_caller]
  fn assert_garbage_collected(self) {
    let obj_was_gc_collected = self.flag.load(Ordering::SeqCst);
    assert!(obj_was_gc_collected);
  }
}
