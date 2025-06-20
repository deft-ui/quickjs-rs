use std::{collections::HashMap, os::raw::c_char};
use std::os::raw::{c_int, c_void};
use std::ptr::null_mut;

use libquickjs_sys as q;

use crate::{JsValue, RawJSValue, ResourceValue, ValueError};

use super::{droppable_value::DroppableValue, JsClass, make_cstring, Resource, ResourceObject};

use super::{
    TAG_BOOL, TAG_EXCEPTION, TAG_FLOAT64, TAG_INT, TAG_NULL, TAG_OBJECT, TAG_STRING, TAG_UNDEFINED,
};

#[cfg(feature = "bigint")]
use {
    super::TAG_BIG_INT,
    crate::value::bigint::{BigInt, BigIntOrI64},
};
use libquickjs_sys::{JS_GetClassID, JS_GetOpaque, JS_GetOpaque2, JS_NewClass, JS_NewClassID, JS_NewObjectClass, JS_SetOpaque, JSClassDef, JSRuntime, JSValue, JS_VALUE_GET_TAG};

#[cfg(feature = "chrono")]
fn js_date_constructor(context: *mut q::JSContext) -> q::JSValue {
    let global = unsafe { q::JS_GetGlobalObject(context) };
    assert!(q::JS_IsObject(global));

    let date_constructor = unsafe {
        q::JS_GetPropertyStr(
            context,
            global,
            std::ffi::CStr::from_bytes_with_nul(b"Date\0")
                .unwrap()
                .as_ptr(),
        )
    };
    assert!(q::JS_IsObject(date_constructor));
    unsafe { q::JS_FreeValue(context, global) };
    date_constructor
}

#[cfg(feature = "bigint")]
fn js_create_bigint_function(context: *mut q::JSContext) -> q::JSValue {
    let global = unsafe { q::JS_GetGlobalObject(context) };
    assert_eq!(global.tag, TAG_OBJECT);

    let bigint_function = unsafe {
        q::JS_GetPropertyStr(
            context,
            global,
            std::ffi::CStr::from_bytes_with_nul(b"BigInt\0")
                .unwrap()
                .as_ptr(),
        )
    };
    assert_eq!(bigint_function.tag, TAG_OBJECT);
    unsafe { q::JS_FreeValue(context, global) };
    bigint_function
}

/// Serialize a Rust value into a quickjs runtime value.
//TODO pub(super)?
pub fn serialize_value(
    context: *mut q::JSContext,
    value: JsValue,
) -> Result<q::JSValue, ValueError> {
    let v = match value {
        JsValue::Undefined => q::JS_UNDEFINED,
        JsValue::Null => q::JS_NULL,
        JsValue::Bool(flag) => q::JS_MKVAL(q::JS_TAG_BOOL, if flag { 1 } else { 0 }),
        JsValue::Int(val) => q::JS_MKVAL(q::JS_TAG_INT, val),
        JsValue::Float(val) => q::__JS_NewFloat64(val),
        JsValue::String(val) => {
            let qval = unsafe {
                q::JS_NewStringLen(context, val.as_ptr() as *const c_char, val.len() as _)
            };

            if q::JS_IsException(qval) {
                return Err(ValueError::Internal(
                    "Could not create string in runtime".into(),
                ));
            }

            qval
        }
        JsValue::Array(values) => {
            // Allocate a new array in the runtime.
            let arr = unsafe { q::JS_NewArray(context) };
            if q::JS_IsException(arr) {
                return Err(ValueError::Internal(
                    "Could not create array in runtime".into(),
                ));
            }

            for (index, value) in values.into_iter().enumerate() {
                let qvalue = match serialize_value(context, value) {
                    Ok(qval) => qval,
                    Err(e) => {
                        // Make sure to free the array if a individual element
                        // fails.

                        unsafe {
                            q::JS_FreeValue(context, arr);
                        }

                        return Err(e);
                    }
                };

                let ret = unsafe {
                    q::JS_DefinePropertyValueUint32(
                        context,
                        arr,
                        index as u32,
                        qvalue,
                        q::JS_PROP_C_W_E as i32,
                    )
                };
                if ret < 0 {
                    // Make sure to free the array if a individual
                    // element fails.
                    unsafe {
                        q::JS_FreeValue(context, arr);
                    }
                    return Err(ValueError::Internal(
                        "Could not append element to array".into(),
                    ));
                }
            }
            arr
        }
        JsValue::Object(map) => {
            let obj = unsafe { q::JS_NewObject(context) };
            if q::JS_IsException(obj) {
                return Err(ValueError::Internal("Could not create object".into()));
            }

            for (key, value) in map {
                let ckey = make_cstring(key)?;

                let qvalue = serialize_value(context, value).map_err(|e| {
                    // Free the object if a property failed.
                    unsafe {
                        q::JS_FreeValue(context, obj);
                    }
                    e
                })?;

                let ret = unsafe {
                    q::JS_DefinePropertyValueStr(
                        context,
                        obj,
                        ckey.as_ptr(),
                        qvalue,
                        q::JS_PROP_C_W_E as i32,
                    )
                };
                if ret < 0 {
                    // Free the object if a property failed.
                    unsafe {
                        q::JS_FreeValue(context, obj);
                    }
                    return Err(ValueError::Internal(
                        "Could not add add property to object".into(),
                    ));
                }
            }

            obj
        }
        JsValue::Raw(raw) => {
            unsafe {
                raw.create_js_value()
            }
        }
        JsValue::Exception(raw) => {
            unsafe {
                raw.create_js_value()
            }
        }
        JsValue::Resource(raw) => {
            create_resource(context, raw)
        }
        #[cfg(feature = "chrono")]
        JsValue::Date(datetime) => {
            let date_constructor = js_date_constructor(context);

            let f = datetime.timestamp_millis() as f64;

            let timestamp = q::JS_NewFloat64(f);

            let mut args = vec![timestamp];

            let value = unsafe {
                q::JS_CallConstructor(
                    context,
                    date_constructor,
                    args.len() as i32,
                    args.as_mut_ptr(),
                )
            };
            unsafe {
                q::JS_FreeValue(context, date_constructor);
            }

            if !q::JS_IsObject(value) {
                return Err(ValueError::Internal(
                    "Could not construct Date object".into(),
                ));
            }
            value
        }
        #[cfg(feature = "bigint")]
        JsValue::BigInt(int) => match int.inner {
            BigIntOrI64::Int(int) => unsafe { q::JS_NewBigInt64(context, int) },
            BigIntOrI64::BigInt(bigint) => {
                let bigint_string = bigint.to_str_radix(10);
                let s = unsafe {
                    q::JS_NewStringLen(
                        context,
                        bigint_string.as_ptr() as *const c_char,
                        bigint_string.len() as q::size_t,
                    )
                };
                let s = DroppableValue::new(s, |&mut s| unsafe {
                    q::JS_FreeValue(context, s);
                });
                if (*s).tag != TAG_STRING {
                    return Err(ValueError::Internal(
                        "Could not construct String object needed to create BigInt object".into(),
                    ));
                }

                let mut args = vec![*s];

                let bigint_function = js_create_bigint_function(context);
                let bigint_function =
                    DroppableValue::new(bigint_function, |&mut bigint_function| unsafe {
                        q::JS_FreeValue(context, bigint_function);
                    });
                let js_bigint = unsafe {
                    q::JS_Call(
                        context,
                        *bigint_function,
                        q::JSValue {
                            u: q::JSValueUnion { int32: 0 },
                            tag: TAG_NULL,
                        },
                        1,
                        args.as_mut_ptr(),
                    )
                };

                if js_bigint.tag != TAG_BIG_INT {
                    return Err(ValueError::Internal(
                        "Could not construct BigInt object".into(),
                    ));
                }

                js_bigint
            }
        },
        JsValue::__NonExhaustive => unreachable!(),
    };
    Ok(v)
}

pub fn create_resource(context: *mut q::JSContext, resource: ResourceValue) -> JSValue {
    unsafe  {
        let class_id = Resource::class_id();
        if class_id.id.get() == 0 {
            let runtime = q::JS_GetRuntime(context);
            let mut cls_id = 0;
            JS_NewClassID(runtime, &mut cls_id);
            class_id.id.set(cls_id);
            extern fn finalizer(rt: *mut JSRuntime, val: JSValue) {
                //println!("finalizer calling");
                unsafe {
                    let cls_id = JS_GetClassID(val);
                    let opaque = JS_GetOpaque(val, cls_id) as *mut ResourceObject;
                    let _ = Box::from_raw(opaque);
                }
                //println!("finalizer called");
            }
            let cls_def = JSClassDef {
                class_name: Resource::NAME.as_ptr() as *const std::ffi::c_char,
                finalizer: Some(finalizer),
                gc_mark: None,
                call: None,
                exotic: null_mut(),
            };
            JS_NewClass(runtime, cls_id, &cls_def);
        }

        let class_id = class_id.id.get();
        let res = JS_NewObjectClass(context, class_id as c_int);
        let opaque = Box::into_raw(Box::new(ResourceObject {
            data: resource,
        }));
        JS_SetOpaque(res, opaque as *mut c_void);
        res
    }

}

fn deserialize_array(
    context: *mut q::JSContext,
    raw_value: &q::JSValue,
) -> Result<JsValue, ValueError> {
    assert!(q::JS_IsObject(*raw_value));

    let length_name = make_cstring("length")?;

    let len_raw = unsafe { q::JS_GetPropertyStr(context, *raw_value, length_name.as_ptr()) };

    let len_res = deserialize_value(context, &len_raw);
    unsafe { q::JS_FreeValue(context, len_raw) };
    let len = match len_res? {
        JsValue::Int(x) => x,
        _ => {
            return Err(ValueError::Internal(
                "Could not determine array length".into(),
            ));
        }
    };

    let mut values = Vec::new();
    for index in 0..(len as usize) {
        let value_raw = unsafe { q::JS_GetPropertyUint32(context, *raw_value, index as u32) };
        if q::JS_IsException(value_raw) {
            return Err(ValueError::Internal("Could not build array".into()));
        }
        let value_res = deserialize_value(context, &value_raw);
        unsafe { q::JS_FreeValue(context, value_raw) };

        let value = value_res?;
        values.push(value);
    }

    Ok(JsValue::Array(values))
}

pub fn deserialize_object(context: *mut q::JSContext, obj: &q::JSValue) -> Result<HashMap<String, JsValue>, ValueError> {
    assert_eq!(JS_VALUE_GET_TAG(*obj), q::JS_TAG_OBJECT);

    let mut properties: *mut q::JSPropertyEnum = std::ptr::null_mut();
    let mut count: u32 = 0;

    let flags = (q::JS_GPN_STRING_MASK | q::JS_GPN_SYMBOL_MASK | q::JS_GPN_ENUM_ONLY) as i32;
    let ret =
        unsafe { q::JS_GetOwnPropertyNames(context, &mut properties, &mut count, *obj, flags) };
    if ret != 0 {
        return Err(ValueError::Internal(
            "Could not get object properties".into(),
        ));
    }

    // TODO: refactor into a more Rust-idiomatic iterator wrapper.
    let properties = DroppableValue::new(properties, |&mut properties| {
        for index in 0..count {
            let prop = unsafe { properties.offset(index as isize) };
            unsafe {
                q::JS_FreeAtom(context, (*prop).atom);
            }
        }
        unsafe {
            q::js_free(context, properties as *mut std::ffi::c_void);
        }
    });

    let mut map = HashMap::new();
    for index in 0..count {
        let prop = unsafe { (*properties).offset(index as isize) };
        let raw_value = unsafe { q::JS_GetProperty(context, *obj, (*prop).atom) };
        if q::JS_IsException(raw_value) {
            return Err(ValueError::Internal("Could not get object property".into()));
        }

        let value_res = deserialize_value(context, &raw_value);
        unsafe {
            q::JS_FreeValue(context, raw_value);
        }
        let value = value_res?;

        let key_value = unsafe { q::JS_AtomToString(context, (*prop).atom) };
        if q::JS_IsException(key_value) {
            return Err(ValueError::Internal(
                "Could not get object property name".into(),
            ));
        }

        let key_res = deserialize_value(context, &key_value);
        unsafe {
            q::JS_FreeValue(context, key_value);
        }
        let key = match key_res? {
            JsValue::String(s) => s,
            _ => {
                return Err(ValueError::Internal("Could not get property name".into()));
            }
        };
        map.insert(key, value);
    }

    // Ok(JsValue::Object(map))
    Ok(map)
}

pub fn deserialize_value(
    context: *mut q::JSContext,
    value: &q::JSValue,
) -> Result<JsValue, ValueError> {
    let r = value;

    match q::JS_VALUE_GET_TAG(*r) {
        // Int.
        q::JS_TAG_INT => {
            let val = unsafe { q::JS_VALUE_GET_INT(*r) };
            Ok(JsValue::Int(val))
        }
        // Bool.
        q::JS_TAG_BOOL => {
            let val = unsafe { q::JS_VALUE_GET_BOOL(*r) };
            Ok(JsValue::Bool(val))
        }
        // Null.
        q::JS_TAG_NULL => Ok(JsValue::Null),
        // Undefined.
        q::JS_TAG_UNDEFINED => Ok(JsValue::Undefined),
        // Float.
        q::JS_TAG_FLOAT64 => {
            let val = unsafe { q::JS_VALUE_GET_FLOAT64(*r) };
            Ok(JsValue::Float(val))
        }
        // String.
        q::JS_TAG_STRING => {
            let ptr = unsafe { q::JS_ToCStringLen2(context, std::ptr::null_mut(), *r, false) };

            if ptr.is_null() {
                return Err(ValueError::Internal(
                    "Could not convert string: got a null pointer".into(),
                ));
            }

            let cstr = unsafe { std::ffi::CStr::from_ptr(ptr) };

            let s = cstr
                .to_str()
                .map_err(ValueError::InvalidString)?
                .to_string();

            // Free the c string.
            unsafe { q::JS_FreeCString(context, ptr) };

            Ok(JsValue::String(s))
        }
        // Object.
        q::JS_TAG_OBJECT => {
            let is_func = unsafe { q::JS_IsFunction(context, *r)};
            if is_func {
                //TODO remove
                let raw_js_value = RawJSValue::new(context, value);
                return Ok(JsValue::Raw(raw_js_value));
            }
            let is_array = unsafe { q::JS_IsArray(context, *r) } > 0;
            if is_array {
                deserialize_array(context, r)
            } else {
                let is_resource = unsafe {
                    Resource::class_id().id.get() > 0 && q::JS_GetClassID(*r) == Resource::class_id().id.get()
                };
                if is_resource {
                    unsafe {
                        let cls_id = JS_GetClassID(*value);
                        let cls_obj = JS_GetOpaque2(context, *value, cls_id) as *mut ResourceObject;
                        let res = (*cls_obj).data.resource.clone();
                        return Ok(JsValue::Resource(ResourceValue {
                            resource: res
                        }))
                    }
                }
                #[cfg(feature = "chrono")]
                {
                    use chrono::offset::TimeZone;

                    let date_constructor = js_date_constructor(context);
                    let is_date = unsafe { q::JS_IsInstanceOf(context, *r, date_constructor) > 0 };

                    if is_date {
                        let getter = unsafe {
                            q::JS_GetPropertyStr(
                                context,
                                *r,
                                std::ffi::CStr::from_bytes_with_nul(b"getTime\0")
                                    .unwrap()
                                    .as_ptr(),
                            )
                        };
                        assert_eq!(q::JS_VALUE_GET_TAG(getter), q::JS_TAG_OBJECT);

                        let timestamp_raw =
                            unsafe { q::JS_Call(context, getter, *r, 0, std::ptr::null_mut()) };

                        unsafe {
                            q::JS_FreeValue(context, getter);
                            q::JS_FreeValue(context, date_constructor);
                        };

                        let res = if q::JS_IsFloat64(timestamp_raw) {
                            let f = unsafe { q::JS_VALUE_GET_FLOAT64(timestamp_raw) } as i64;
                            let datetime = chrono::Utc.timestamp_millis(f);
                            Ok(JsValue::Date(datetime))
                        } else if q::JS_IsInt(timestamp_raw) {
                            let f = unsafe { q::JS_VALUE_GET_INT(timestamp_raw) } as i64;
                            let datetime = chrono::Utc.timestamp_millis(f);
                            Ok(JsValue::Date(datetime))
                        } else {
                            Err(ValueError::Internal(
                                "Could not convert 'Date' instance to timestamp".into(),
                            ))
                        };
                        return res;
                    } else {
                        unsafe { q::JS_FreeValue(context, date_constructor) };
                    }
                }
                let raw_js_value = RawJSValue::new(context, value);
                return Ok(JsValue::Raw(raw_js_value));
            }
        }
        // BigInt
        #[cfg(feature = "bigint")]
        TAG_BIG_INT => {
            let mut int: i64 = 0;
            let ret = unsafe { q::JS_ToBigInt64(context, &mut int, *r) };
            if ret == 0 {
                Ok(JsValue::BigInt(BigInt {
                    inner: BigIntOrI64::Int(int),
                }))
            } else {
                let ptr = unsafe { q::JS_ToCStringLen2(context, std::ptr::null_mut(), *r, 0) };

                if ptr.is_null() {
                    return Err(ValueError::Internal(
                        "Could not convert BigInt to string: got a null pointer".into(),
                    ));
                }

                let cstr = unsafe { std::ffi::CStr::from_ptr(ptr) };
                let bigint = num_bigint::BigInt::parse_bytes(cstr.to_bytes(), 10).unwrap();

                // Free the c string.
                unsafe { q::JS_FreeCString(context, ptr) };

                Ok(JsValue::BigInt(BigInt {
                    inner: BigIntOrI64::BigInt(bigint),
                }))
            }
        }
        q::JS_TAG_EXCEPTION => {
            let raw_js_value = RawJSValue::new(context, value);
            Ok(JsValue::Exception(raw_js_value))
        }
        t => {
            if q::JS_IsFloat64(*value) {
                Ok(JsValue::Float(unsafe {
                    q::JS_VALUE_GET_FLOAT64(*value)
                }))
            } else {
                // println!("unknown tag: {}", t);
                let raw_js_value = RawJSValue::new(context, value);
                Ok(JsValue::Raw(raw_js_value))
            }
        }
    }
}
