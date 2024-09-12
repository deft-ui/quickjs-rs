//! js module loader
use std::ffi::{c_char, CStr, CString};
use std::fs::File;
use std::io;
use std::io::{Error, Read};
use std::os::raw::c_int;
use std::path::PathBuf;
use std::ptr::null_mut;
use std::str::FromStr;
use libquickjs_sys::{JS_Eval, JS_EVAL_FLAG_COMPILE_ONLY, JS_EVAL_TYPE_MODULE, JS_FreeValue, JS_IsException, js_module_set_import_meta, JSContext, JSModuleDef};

/// js module loader callback
pub unsafe extern "C" fn quickjs_rs_module_loader(
        ctx: *mut JSContext,
        module_name: *const ::std::os::raw::c_char,
        opaque: *mut ::std::os::raw::c_void,
    ) -> *mut JSModuleDef {
    let module_name = CStr::from_ptr(module_name);
    println!("loading module:{:?}", module_name);
    let mut loader = &*(opaque as *mut _ as *mut Box<dyn JsModuleLoader>);
    let input =  match loader.load(module_name.to_str().unwrap()) {
        Ok(e) => e,
        Err(err) => {
            return null_mut()
        }
    };
    let code_len = input.len();
    let code = CString::new(input).unwrap();
    let func_val = JS_Eval(
        ctx,
        code.as_ptr() as *const c_char,
        code_len,
        module_name.as_ptr(),
        (JS_EVAL_TYPE_MODULE | JS_EVAL_FLAG_COMPILE_ONLY) as c_int
    );
    if JS_IsException(func_val) {
        return null_mut();
        // return Err(anyhow!("Failed to load module"));
    }
    js_module_set_import_meta(ctx, func_val, true as c_int, false as c_int);
    let ptr = func_val.u.ptr;
    JS_FreeValue(ctx, func_val);
    ptr as *mut JSModuleDef
}

/// js module loader trait
pub trait JsModuleLoader: 'static {
    /// load a module
    fn load(&self, module_name: &str) -> Result<String, io::Error>;
}

/// File system module loader
pub struct FsJsModuleLoader {
    base: PathBuf,
}

impl FsJsModuleLoader {

    /// create a new FsJsModuleLoader
    pub fn new(base: &str) -> Self {
        Self {
            base: PathBuf::from_str(base).unwrap()
        }
    }
}

impl JsModuleLoader for FsJsModuleLoader {
    fn load(&self, module_name: &str) -> Result<String, Error> {
        let path = self.base.join(module_name);
        let mut file = File::open(path)?;
        let mut content = String::new();
        file.read_to_string(&mut content)?;
        Ok(content)
    }
}