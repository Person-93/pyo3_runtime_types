#![allow(
  unsafe_op_in_unsafe_fn,
  clippy::missing_assert_message,
  clippy::undocumented_unsafe_blocks,
  clippy::unnecessary_wraps,
  clippy::unused_self,
  reason = "tests"
)]

use std::iter;
use std::sync::atomic::{AtomicBool, Ordering};

use pyo3::PyTypeInfo as _;
use pyo3::exceptions::PySystemError;
use pyo3::ffi::PyObject_CallFinalizer;
use pyo3::prelude::*;

use pyo3::types::PyType;
use pyo3_runtime_types::{Metaclass, PyTypeBuilder};

#[test]
fn obj_created_inited_and_destroyed() {
  S::clear_flags();

  Python::initialize();
  Python::attach(|py| {
    let mut builder =
      PyTypeBuilder::new("S", Box::new(|_, _, _| Ok(S::default())));
    builder.init_fn(Box::new(|slf, _, _, _| slf.__init__()));
    let ty = builder.build(py).unwrap();
    let obj = ty.call0().unwrap();
    drop(obj);
    PyModule::import(py, "gc")
      .unwrap()
      .getattr("collect")
      .unwrap()
      .call0()
      .unwrap();
  });

  S::assert_inited_and_finalized();
}

#[test]
fn build_metaclass_exception() {
  Python::initialize();
  Python::attach(|py| {
    let mut builder = PyTypeBuilder::new("dummy", Box::new(|_, _, _| Ok(42)));
    let err = builder
      .bases(iter::once(PyType::type_object(py)))
      .err()
      .unwrap();
    assert!(err.matches(py, PySystemError::type_object(py)).unwrap());
  });
}

#[test]
fn new_metaclass() {
  struct Meta;
  thread_local! {
    static DESTROY: AtomicBool = const { AtomicBool::new(false) };
  }
  impl Drop for Meta {
    fn drop(&mut self) {
      DESTROY.with(|b| b.store(true, Ordering::SeqCst));
    }
  }

  S::clear_flags();

  Python::initialize();
  Python::attach(|py| {
    let meta = Metaclass::new(py, "Meta").unwrap();
    let mut builder =
      meta.builder(py, "S", Meta, Box::new(|_, _, _| Ok(S::default())));
    builder.init_fn(Box::new(|slf, _, _, _| slf.__init__()));
    let ty = builder.build(py).unwrap();
    let obj = ty.call0().unwrap();

    unsafe {
      finalize(obj);
      S::assert_inited_and_finalized();

      finalize(ty);
      assert!(DESTROY.with(|b| b.load(Ordering::SeqCst)));
    }
    drop(meta);
  });
}

unsafe fn finalize<T>(obj: Bound<'_, T>) {
  let p = obj.as_ptr();
  drop(obj);
  PyObject_CallFinalizer(p);
}

thread_local! {
  static INIT: AtomicBool = const { AtomicBool::new(false) };
  static DESTROY: AtomicBool = const { AtomicBool::new(false) };
}

#[derive(Default)]
struct S {
  _n: i32, // prevent it from being a ZST
}

impl S {
  fn clear_flags() {
    INIT.with(|b| b.store(false, Ordering::SeqCst));
    DESTROY.with(|b| b.store(false, Ordering::SeqCst));
  }

  #[track_caller]
  fn assert_inited_and_finalized() {
    assert!(INIT.with(|b| b.load(Ordering::SeqCst)));
    assert!(DESTROY.with(|b| b.load(Ordering::SeqCst)));
  }

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
