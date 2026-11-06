//! Small, dynamically loaded GIO adapter for *no-overwrite* remote writes.
//!
//! The `gio copy`/`gio move` command-line tools may silently replace existing
//! destinations. The underlying `g_file_copy`/`g_file_move` APIs explicitly
//! reject existing names without `G_FILE_COPY_OVERWRITE`, which we never pass.
//! Runtime loading keeps the binary usable without optional GVfs backends.

use libloading::Library;
use std::{
    ffi::{CStr, CString, c_char, c_void},
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant},
};

type Ptr = *mut c_void;

#[repr(C)]
struct GError {
    domain: u32,
    code: i32,
    message: *mut c_char,
}

type CopyMove = unsafe extern "C" fn(
    Ptr,
    Ptr,
    u32,
    Ptr,
    Option<unsafe extern "C" fn(i64, i64, Ptr)>,
    Ptr,
    *mut *mut GError,
) -> i32;

struct Api {
    _gio: Library,
    _gobject: Library,
    _glib: Library,
    file_from_uri: unsafe extern "C" fn(*const c_char) -> Ptr,
    file_from_path: unsafe extern "C" fn(*const c_char) -> Ptr,
    file_copy: CopyMove,
    file_move: CopyMove,
    cancellable_new: unsafe extern "C" fn() -> Ptr,
    cancellable_cancel: unsafe extern "C" fn(Ptr),
    object_unref: unsafe extern "C" fn(Ptr),
    error_free: unsafe extern "C" fn(*mut GError),
}

impl Api {
    fn load() -> Result<Self, String> {
        let gio = unsafe { Library::new("libgio-2.0.so.0") }
            .map_err(|error| format!("Native GIO unavailable: {error}"))?;
        let gobject = unsafe { Library::new("libgobject-2.0.so.0") }
            .map_err(|error| format!("GObject unavailable: {error}"))?;
        let glib = unsafe { Library::new("libglib-2.0.so.0") }
            .map_err(|error| format!("GLib unavailable: {error}"))?;
        macro_rules! symbol {
            ($lib:ident, $name:literal) => {{
                // SAFETY: each function has the documented stable GLib C ABI;
                // all three libraries outlive the copied function pointers.
                unsafe { $lib.get(concat!($name, "\0").as_bytes()) }
                    .map(|symbol: libloading::Symbol<'_, _>| *symbol)
                    .map_err(|error| format!("Missing GIO symbol {}: {error}", $name))?
            }};
        }
        Ok(Self {
            file_from_uri: symbol!(gio, "g_file_new_for_uri"),
            file_from_path: symbol!(gio, "g_file_new_for_path"),
            file_copy: symbol!(gio, "g_file_copy"),
            file_move: symbol!(gio, "g_file_move"),
            cancellable_new: symbol!(gio, "g_cancellable_new"),
            cancellable_cancel: symbol!(gio, "g_cancellable_cancel"),
            object_unref: symbol!(gobject, "g_object_unref"),
            error_free: symbol!(glib, "g_error_free"),
            _gio: gio,
            _gobject: gobject,
            _glib: glib,
        })
    }

    fn perform(
        &self,
        source: &str,
        destination: &str,
        operation: CopyMove,
        flags: u32,
        cancelled: &AtomicBool,
        deadline: Instant,
    ) -> Result<(), String> {
        let source = CString::new(source).map_err(|_| "Invalid source URI")?;
        let destination = CString::new(destination).map_err(|_| "Invalid destination URI")?;
        self.perform_raw(
            unsafe { (self.file_from_uri)(source.as_ptr()) },
            unsafe { (self.file_from_uri)(destination.as_ptr()) },
            operation,
            flags,
            cancelled,
            deadline,
        )
    }

    fn perform_local_copy(
        &self,
        path: &Path,
        destination: &str,
        cancelled: &AtomicBool,
        deadline: Instant,
    ) -> Result<(), String> {
        use std::os::unix::ffi::OsStrExt;
        let path =
            CString::new(path.as_os_str().as_bytes()).map_err(|_| "Invalid source pathname")?;
        let destination = CString::new(destination).map_err(|_| "Invalid destination URI")?;
        // NOFOLLOW_SYMLINKS=4; never request OVERWRITE=1.
        self.perform_raw(
            unsafe { (self.file_from_path)(path.as_ptr()) },
            unsafe { (self.file_from_uri)(destination.as_ptr()) },
            self.file_copy,
            4,
            cancelled,
            deadline,
        )
    }

    fn perform_raw(
        &self,
        source: Ptr,
        destination: Ptr,
        operation: CopyMove,
        flags: u32,
        cancelled: &AtomicBool,
        deadline: Instant,
    ) -> Result<(), String> {
        struct Objects<'a>(&'a Api, Vec<Ptr>);
        impl Drop for Objects<'_> {
            fn drop(&mut self) {
                for ptr in self.1.drain(..) {
                    if !ptr.is_null() {
                        unsafe { (self.0.object_unref)(ptr) };
                    }
                }
            }
        }
        let mut objects = Objects(self, vec![source, destination]);
        if source.is_null() || destination.is_null() {
            return Err("GIO could not initialize source or destination".into());
        }
        if cancelled.load(Ordering::Relaxed) || Instant::now() >= deadline {
            return Err("GIO transfer cancelled before starting".into());
        }
        let cancellable = unsafe { (self.cancellable_new)() };
        if cancellable.is_null() {
            return Err("GIO could not allocate a cancellable transfer".into());
        }
        objects.1.push(cancellable);
        let finished = AtomicBool::new(false);
        let mut failure = std::ptr::null_mut();
        // GIO's GCancellable is explicitly thread-safe. The watcher always
        // joins before Objects releases it; GUI threads are never blocked.
        let success = thread::scope(|scope| {
            let handle = cancellable as usize;
            let done = &finished;
            let watcher = scope.spawn(move || {
                while !done.load(Ordering::Acquire) {
                    if cancelled.load(Ordering::Relaxed) || Instant::now() >= deadline {
                        unsafe { (self.cancellable_cancel)(handle as Ptr) };
                        break;
                    }
                    thread::sleep(Duration::from_millis(35));
                }
            });
            let result = unsafe {
                operation(
                    source,
                    destination,
                    flags,
                    cancellable,
                    None,
                    std::ptr::null_mut(),
                    &mut failure,
                )
            };
            finished.store(true, Ordering::Release);
            watcher.join().expect("GIO cancellation watcher panicked");
            result != 0
        });
        let message = if failure.is_null() {
            "GIO operation failed without error detail".into()
        } else {
            let message = unsafe { CStr::from_ptr((*failure).message) }
                .to_string_lossy()
                .into_owned();
            unsafe { (self.error_free)(failure) };
            message
        };
        if success {
            Ok(())
        } else {
            Err(format!("Native GIO refused operation: {message}"))
        }
    }
}

pub fn stage_copy(
    source: &Path,
    stage: &str,
    cancelled: &AtomicBool,
    deadline: Instant,
) -> Result<(), String> {
    Api::load()?.perform_local_copy(source, stage, cancelled, deadline)
}

pub fn publish_no_replace(
    stage: &str,
    destination: &str,
    cancelled: &AtomicBool,
    deadline: Instant,
) -> Result<(), String> {
    // G_FILE_COPY_NO_FALLBACK_FOR_MOVE=16. GIO must refuse copying a
    // destination if the backend cannot perform a native rename.
    let api = Api::load()?;
    api.perform(stage, destination, api.file_move, 16, cancelled, deadline)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    #[test]
    fn native_staging_refuses_preexisting_name_without_touching_bytes() {
        let root = std::env::temp_dir().join(format!(
            "ricefield-gio-no-overwrite-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        let source = root.join("source");
        let staging = root.join("staging");
        fs::write(&source, b"source bytes").unwrap();
        fs::write(&staging, b"original stage bytes").unwrap();
        assert!(
            stage_copy(
                &source,
                &format!("file://{}", staging.display()),
                &AtomicBool::new(false),
                Instant::now() + Duration::from_secs(10)
            )
            .is_err()
        );
        assert_eq!(fs::read(staging).unwrap(), b"original stage bytes");
        assert_eq!(fs::read(source).unwrap(), b"source bytes");
        fs::remove_dir_all(root).unwrap();
    }
}
