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
use std::sync::atomic::{AtomicBool, Ordering};

use pyo3::PyTypeInfo as _;
use pyo3::exceptions::PySystemError;
use pyo3::ffi::{PyGC_Collect, PyGC_Disable, PyGC_Enable};
use pyo3::prelude::*;

use pyo3::types::PyType;
use pyo3_runtime_types::{Metaclass, PyTypeBuilder};

#[test]
fn obj_created_inited_and_destroyed() {
  S::clear_flags();

  py_wrapper(|py| {
    let mut builder =
      PyTypeBuilder::new("S", Box::new(|_, _, _| Ok(S::default())));
    builder.init_fn(Box::new(|slf, _, _, _| slf.__init__()));
    let ty = builder.build(py).unwrap();
    let obj = ty.call0().unwrap();
    eprintln!("deleting obj");
    drop(obj);
    gc_collect_force(py);

    eprintln!("deleting ty");
    drop(ty);
    gc_collect_force(py);
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

  py_wrapper(|py| {
    let meta = Metaclass::new(py, "Meta").unwrap();
    let mut builder =
      meta.builder(py, "S", Meta, Box::new(|_, _, _| Ok(S::default())));
    builder.init_fn(Box::new(|slf, _, _, _| slf.__init__()));
    let ty = builder.build(py).unwrap();
    let obj = ty.call0().unwrap();

    eprintln!("deleting obj");
    drop(obj);
    gc_collect_force(py);
    S::assert_inited_and_finalized();

    eprintln!("deleting ty");
    drop(ty);
    gc_collect_force(py);
    gc_collect_force(py);
    assert!(DESTROY.with(|b| b.load(Ordering::SeqCst)));

    eprintln!("deleting meta");
    drop(meta);
    gc_collect_force(py);
  });
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

fn gc_collect_force(py: Python<'_>) {
  let previous_state = gc_enable(py);
  gc_collect(py);
  if !previous_state.enabled() {
    gc_disable(py);
  }
}

fn gc_collect(_py: Python<'_>) {
  eprintln!("running python gc...");
  let count = unsafe { PyGC_Collect() };
  eprintln!("collected + uncollectable = {count}");
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

fn py_wrapper<R>(f: impl for<'py> FnOnce(Python<'py>) -> R) -> R {
  Python::initialize();
  Python::attach(|py| {
    // gc_collect_force(py);
    gc_disable(py);
    f(py)
  })
}
