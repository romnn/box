#[cxx::bridge]
mod ffi {
    unsafe extern "C++" {
        include!("playground-sys/src/bindings.hpp");

        type scheduler_unit;
        #[must_use]
        fn new_scheduler_unit() -> UniquePtr<scheduler_unit>;
    }
}

pub use ffi::*;
