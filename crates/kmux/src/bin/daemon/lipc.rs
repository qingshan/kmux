//! Kindle-only LIPC ABI and service lifetime.
use super::{state::*, Op, SERVICE_NAME};
use kmuxd::{read_string_prop, write_string_prop, LIPC_ERROR_INVALID_ARG, LIPC_OK};
use std::ffi::{c_char, c_int, c_void, CString};
use std::sync::atomic::Ordering;

type LipcHandle = *mut c_void;
type LipcCallback = extern "C" fn(LipcHandle, *const c_char, *mut c_void, *mut c_void) -> c_int;

#[link(name = "lipc")]
extern "C" {
    fn LipcOpenEx(service: *const c_char, code: *mut c_int) -> LipcHandle;
    fn LipcClose(handle: LipcHandle) -> c_int;
    fn LipcRegisterStringProperty(
        lipc: LipcHandle,
        prop: *const c_char,
        getter: Option<LipcCallback>,
        setter: Option<LipcCallback>,
        data: *mut c_void,
    ) -> c_int;
}

pub(crate) struct Service {
    handle: LipcHandle,
}
impl Service {
    pub(crate) fn open() -> Result<Self, c_int> {
        let service = CString::new(SERVICE_NAME).expect("static, no NUL");
        let mut code: c_int = -1;
        let handle = unsafe { LipcOpenEx(service.as_ptr(), &mut code) };
        if code != LIPC_OK {
            return Err(code);
        }
        Ok(Self { handle })
    }
    pub(crate) fn register_properties(&self) {
        let props: [(&str, Option<LipcCallback>, Option<LipcCallback>); 4] = [
            ("cmd", Some(cmd_getter), Some(cmd_setter)),
            ("status", Some(status_getter), None),
            ("exit", Some(exit_getter), Some(exit_setter)),
            ("info", Some(info_getter), None),
        ];
        for (prop, getter, setter) in props {
            let prop = CString::new(prop).expect("static, no NUL");
            unsafe {
                LipcRegisterStringProperty(
                    self.handle,
                    prop.as_ptr(),
                    getter,
                    setter,
                    std::ptr::null_mut(),
                )
            };
        }
    }
}
impl Drop for Service {
    fn drop(&mut self) {
        unsafe { LipcClose(self.handle) };
    }
}

extern "C" fn cmd_getter(
    _h: LipcHandle,
    _p: *const c_char,
    value: *mut c_void,
    data: *mut c_void,
) -> c_int {
    let out = last_cmd()
        .clone()
        .unwrap_or_else(|| "No command yet.".to_string());
    unsafe { write_string_prop(value, data, &out) }
}

extern "C" fn cmd_setter(
    _h: LipcHandle,
    _p: *const c_char,
    value: *mut c_void,
    _d: *mut c_void,
) -> c_int {
    let Some(cmd) = (unsafe { read_string_prop(value) }) else {
        return LIPC_ERROR_INVALID_ARG;
    };
    *last_cmd() = Some(cmd.clone());
    match serde_json::from_str::<Op>(&cmd) {
        Ok(op) => {
            if let Some(tx) = OP_TX.get() {
                let _ = tx.send(op);
            }
        }
        Err(e) => set_status(|s| {
            s.last_error = Some(format!("invalid command: {}", e));
        }),
    }
    LIPC_OK
}

extern "C" fn status_getter(
    _h: LipcHandle,
    _p: *const c_char,
    value: *mut c_void,
    data: *mut c_void,
) -> c_int {
    let json = serde_json::to_string(&*status_mutex().lock().unwrap_or_else(|p| p.into_inner()))
        .unwrap_or_else(|_| "{}".to_string());
    unsafe { write_string_prop(value, data, &json) }
}

extern "C" fn exit_getter(
    _h: LipcHandle,
    _p: *const c_char,
    value: *mut c_void,
    data: *mut c_void,
) -> c_int {
    unsafe { write_string_prop(value, data, "Write into this property to exit kmuxd") }
}

extern "C" fn exit_setter(
    _h: LipcHandle,
    _p: *const c_char,
    _value: *mut c_void,
    _d: *mut c_void,
) -> c_int {
    KEEP_RUNNING.store(false, Ordering::SeqCst);
    LIPC_OK
}

extern "C" fn info_getter(
    _h: LipcHandle,
    _p: *const c_char,
    value: *mut c_void,
    data: *mut c_void,
) -> c_int {
    let msg = format!(
        "Build Info: Branch: {}, Commit: {}, Built On: {}",
        env!("GIT_BRANCH"),
        env!("GIT_COMMIT"),
        env!("BUILD_TIME")
    );
    unsafe { write_string_prop(value, data, &msg) }
}
