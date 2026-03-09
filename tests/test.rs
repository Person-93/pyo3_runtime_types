#![allow(clippy::unnecessary_wraps, clippy::unused_self, reason = "tests")]

use std::sync::atomic::{AtomicBool, Ordering};

use pyo3::prelude::*;

use pyo3_runtime_types::Builder;

#[test]
fn obj_created_inited_and_destroyed() {
  thread_local! {
    static INIT: AtomicBool = const { AtomicBool::new(false) };
    static DESTROY: AtomicBool = const { AtomicBool::new(false) };
  }

  Python::initialize();
  Python::attach(|py| {
    let mut builder = Builder::new("S", Box::new(|_, _, _| Ok(S::default())));
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
