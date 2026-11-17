//! The manager's and the worker's global allocator, [`ZeroOnFree`]: every block they free is zeroed first, so no copy of a
//! password outlives the memory that held it. The PAM answer is copied several times on its way (the frame, the `CString`
//! handed to PAM), and the libraries that make the copies do not clear them.

pub use cupertino_login_ipc::zeroing::ZeroOnFree;
