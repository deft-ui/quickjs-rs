#[cfg(feature = "bigint")]
pub(crate) mod bigint;

use std::convert::{TryFrom, TryInto};
use std::{collections::HashMap, error, fmt};
use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;
use libquickjs_sys as q;

#[cfg(feature = "bigint")]
pub use bigint::BigInt;
use libquickjs_sys::{JS_Call, JS_FreeValue, JS_NewPromiseCapability, JSContext, JSValue};
use crate::{Context, ExecutionError};
use crate::bindings::convert::{deserialize_object, deserialize_value, serialize_value};
use crate::bindings::{make_cstring, TAG_EXCEPTION};
use crate::bindings::value::JsTag;
use crate::ValueError::UnexpectedType;

/// Raw js value
#[derive(PartialEq, Debug)]
pub struct RawJSValue {
    /// the js context
    ctx: *mut JSContext,
    /// The raw js value
    js_value: *mut JSValue,
}

impl RawJSValue {

    /// Create a raw js value
    pub fn new(ctx: *mut JSContext, value: &JSValue) -> Self {
        unsafe {
            libquickjs_sys::JS_DupValue(ctx, *value);
        }
        let ptr = Box::into_raw(Box::new(*value));
        Self {
            ctx,
            js_value: ptr,
        }
    }

    /// Create JSValue
    pub fn create_js_value(&self) -> JSValue {
        unsafe {
            let v = *self.js_value;
            libquickjs_sys::JS_DupValue(self.ctx, v);
            v
        }
    }

}

impl Clone for RawJSValue {
    fn clone(&self) -> Self {
        unsafe {
            Self::new(self.ctx, &*self.js_value)
        }
    }
}

impl Drop for RawJSValue {
    fn drop(&mut self) {
        unsafe {
            let v = unsafe { Box::from_raw(self.js_value) };
            libquickjs_sys::JS_FreeValue(self.ctx, *v.as_ref());
            //TODO free js_value?
            // Box::from_raw(self.js_value);
        }
    }
}

#[derive(Debug, Clone)]
pub struct ResourceValue {
    pub resource: Rc<RefCell<dyn Any>>
}


impl ResourceValue {

    pub fn with<T: Any,R, F: FnOnce(&mut T) -> R>(&self, callback: F) -> Option<R> {
        let mut b = self.resource.borrow_mut();
        if let Some(e) = b.downcast_mut::<T>() {
            Some(callback(e))
        } else {
            None
        }
    }

}


/// A value that can be (de)serialized to/from the quickjs runtime.
#[derive(Clone, Debug)]
#[allow(missing_docs)]
pub enum JsValue {
    Undefined,
    Null,
    Bool(bool),
    Int(i32),
    Float(f64),
    String(String),
    Array(Vec<JsValue>),
    Object(HashMap<String, JsValue>),
    Resource(ResourceValue),
    Raw(RawJSValue),
    Exception(RawJSValue),
    /// chrono::Datetime<Utc> / JS Date integration.
    /// Only available with the optional `chrono` feature.
    #[cfg(feature = "chrono")]
    Date(chrono::DateTime<chrono::Utc>),
    /// num_bigint::BigInt / JS BigInt integration
    /// Only available with the optional `bigint` feature
    #[cfg(feature = "bigint")]
    BigInt(crate::BigInt),
    #[doc(hidden)]
    __NonExhaustive,
}

impl JsValue {
    pub fn create_object(context: *mut JSContext, map: HashMap<String, JsValue>) -> Result<Self, ValueError> {
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
            unsafe {
                q::JS_FreeValue(context, qvalue);
            }
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

        Ok(JsValue::Raw(RawJSValue {
            ctx: context,
            js_value: Box::into_raw(Box::new(obj)),
        }))
    }

    pub fn value_type(&self) -> &'static str {
        match self {
            JsValue::Undefined => "undefined",
            JsValue::Null => "null",
            JsValue::Bool(_) => "boolean",
            JsValue::Int(_) => "int",
            JsValue::Float(_) => "float",
            JsValue::String(_) => "string",
            JsValue::Array(_) => "array",
            JsValue::Object(_) => "object",
            JsValue::Resource(_) => "resource",
            JsValue::Raw(_) => "raw",
            JsValue::Exception(_) => "exception",
            #[cfg(feature = "chrono")]
            JsValue::Date(_) => "date",
            #[cfg(feature = "bigint")]
            JsValue::BigInt(_) => "bigint",
            JsValue::__NonExhaustive => "unknown",
        }
    }

    /// Cast value to a str.
    ///
    /// Returns `Some(&str)` if value is a `JsValue::String`, None otherwise.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            JsValue::String(ref s) => Some(s.as_str()),
            _ => None,
        }
    }

    /// Convert to `String`.
    pub fn into_string(self) -> Option<String> {
        match self {
            JsValue::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn new_resource<T: Any>(value: T) -> Self {
        Self::Resource(ResourceValue {
            resource: Rc::new(RefCell::new(value)),
        })
    }

    pub fn as_resource<T: Any,R, F: FnOnce(&mut T) -> R>(&self, callback: F) -> Option<R> {
        if let JsValue::Resource(res) = self {
            res.with(|t| {
                callback(t)
            })
        } else {
            None
        }
    }

    pub fn get_properties(&self) -> Option<HashMap<String, JsValue>> {
        if let JsValue::Raw(raw) = self {
            if let Ok(r) = deserialize_object(raw.ctx, unsafe {&*raw.js_value}) {
                Some(r)
            } else {
                None
            }
        } else {
            None
        }
    }

    pub fn call_as_function(
        &self,
        args: Vec<JsValue>,
    ) -> Result<JsValue, ExecutionError> {
        if let JsValue::Raw(raw) = self {
            // args.in
            let mut qargs = Vec::with_capacity(args.len());
            for arg in args {
                qargs.push(serialize_value(raw.ctx, arg.clone())?);
            }
            let qres_raw = unsafe {
                JS_Call(
                    raw.ctx,
                    *raw.js_value,
                    q::JS_NULL,
                    qargs.len() as i32,
                    qargs.as_mut_ptr(),
                )
            };
            let r = deserialize_value(raw.ctx, &qres_raw);
            unsafe {
                for q in qargs {
                    JS_FreeValue(raw.ctx, q);
                }
                JS_FreeValue(raw.ctx, qres_raw);
            }
            Ok(r?)
        } else {
            Err(ExecutionError::Conversion(UnexpectedType))
        }
    }

}

macro_rules! value_impl_from {
    (
        (
            $(  $t1:ty => $var1:ident, )*
        )
        (
            $( $t2:ty => |$exprname:ident| $expr:expr => $var2:ident, )*
        )
    ) => {
        $(
            impl From<$t1> for JsValue {
                fn from(value: $t1) -> Self {
                    JsValue::$var1(value)
                }
            }

            impl std::convert::TryFrom<JsValue> for $t1 {
                type Error = ValueError;

                fn try_from(value: JsValue) -> Result<Self, Self::Error> {
                    match value {
                        JsValue::$var1(inner) => Ok(inner),
                        _ => Err(ValueError::UnexpectedType)
                    }

                }
            }
        )*
        $(
            impl From<$t2> for JsValue {
                fn from(value: $t2) -> Self {
                    let $exprname = value;
                    let inner = $expr;
                    JsValue::$var2(inner)
                }
            }
        )*
    }
}

/// Js promise
pub struct JsPromise {
    context: *mut JSContext,
    func: Vec<JSValue>,
    raw_js_value: RawJSValue,
    settled: bool,
}

impl JsPromise {

    /// Create a new JsPromise
    pub fn new(context: &mut Context) -> JsPromise {
        // let context = context;
        let mut func: Vec<JSValue> = Vec::with_capacity(2);
        let value = unsafe {
            JS_NewPromiseCapability(context.wrapper.context, func.as_mut_ptr())
        };
        unsafe {
            func.set_len(2);
        }
        let raw_js_value = RawJSValue::new(context.wrapper.context, &value);
        unsafe {
            JS_FreeValue(context.wrapper.context, value);
        }
        Self {
            func,
            raw_js_value,
            context: context.wrapper.context,
            settled: false,
        }
    }

    /// Resolve the promise
    pub fn resolve(&mut self, value: JsValue) {
        if !self.mark_settled() {
            return;
        }
        unsafe {
            let undef = crate::bindings::convert::serialize_value(self.context, JsValue::Undefined).unwrap();
            let mut val = crate::bindings::convert::serialize_value(self.context, value).unwrap();
            let res = JS_Call(self.context, self.func[0], undef, 1, &mut val as *mut JSValue);
            JS_FreeValue(self.context, val);
            JS_FreeValue(self.context, res);
            JS_FreeValue(self.context, self.func[0]);
            JS_FreeValue(self.context, self.func[1]);
        }
    }

    /// Reject the promise
    pub fn reject(&mut self, value: JsValue) {
        if !self.mark_settled() {
            return;
        }
        unsafe {
            let undef = crate::bindings::convert::serialize_value(self.context, JsValue::Undefined).unwrap();
            let mut val = crate::bindings::convert::serialize_value(self.context, value).unwrap();
            let res = JS_Call(self.context, self.func[1], undef, 1, &mut val as *mut JSValue);
            JS_FreeValue(self.context, val);
            JS_FreeValue(self.context, res);
            JS_FreeValue(self.context, self.func[0]);
            JS_FreeValue(self.context, self.func[1]);
        }
    }



    /// Js value
    pub fn js_value(&self) -> JsValue {
        JsValue::Raw(self.raw_js_value.clone())
        //self.value.clone()
    }

    fn mark_settled(&mut self) -> bool {
        if !self.settled {
            self.settled = true;
            true
        } else {
            false
        }
    }

}

value_impl_from! {
    (
        bool => Bool,
        i32 => Int,
        f64 => Float,
        String => String,
    )
    (
        i8 => |x| i32::from(x) => Int,
        i16 => |x| i32::from(x) => Int,
        u8 => |x| i32::from(x) => Int,
        u16 => |x| i32::from(x) => Int,
        u32 => |x| f64::from(x) => Float,
    )
}

#[cfg(feature = "bigint")]
value_impl_from! {
    ()
    (
        i64 => |x| x.into() => BigInt,
        u64 => |x| num_bigint::BigInt::from(x).into() => BigInt,
        i128 => |x| num_bigint::BigInt::from(x).into() => BigInt,
        u128 => |x| num_bigint::BigInt::from(x).into() => BigInt,
        num_bigint::BigInt => |x| x.into() => BigInt,
    )
}

#[cfg(feature = "bigint")]
impl std::convert::TryFrom<JsValue> for i64 {
    type Error = ValueError;

    fn try_from(value: JsValue) -> Result<Self, Self::Error> {
        match value {
            JsValue::Int(int) => Ok(int as i64),
            JsValue::BigInt(bigint) => bigint.as_i64().ok_or(ValueError::UnexpectedType),
            _ => Err(ValueError::UnexpectedType),
        }
    }
}

#[cfg(feature = "bigint")]
macro_rules! value_bigint_impl_tryfrom {
    (
        ($($t:ty => $to_type:ident, )*)
    ) => {
        $(
            impl std::convert::TryFrom<JsValue> for $t {
                type Error = ValueError;

                fn try_from(value: JsValue) -> Result<Self, Self::Error> {
                    use num_traits::ToPrimitive;

                    match value {
                        JsValue::Int(int) => Ok(int as $t),
                        JsValue::BigInt(bigint) => bigint
                            .into_bigint()
                            .$to_type()
                            .ok_or(ValueError::UnexpectedType),
                        _ => Err(ValueError::UnexpectedType),
                    }
                }
            }
        )*
    }
}

#[cfg(feature = "bigint")]
value_bigint_impl_tryfrom! {
    (
        u64 => to_u64,
        i128 => to_i128,
        u128 => to_u128,
    )
}

#[cfg(feature = "bigint")]
impl std::convert::TryFrom<JsValue> for num_bigint::BigInt {
    type Error = ValueError;

    fn try_from(value: JsValue) -> Result<Self, Self::Error> {
        match value {
            JsValue::Int(int) => Ok(num_bigint::BigInt::from(int)),
            JsValue::BigInt(bigint) => Ok(bigint.into_bigint()),
            _ => Err(ValueError::UnexpectedType),
        }
    }
}

impl<T> From<Vec<T>> for JsValue
where
    T: Into<JsValue>,
{
    fn from(values: Vec<T>) -> Self {
        let items = values.into_iter().map(|x| x.into()).collect();
        JsValue::Array(items)
    }
}

impl<T> TryFrom<JsValue> for Vec<T>
where
    T: TryFrom<JsValue>,
{
    type Error = ValueError;

    fn try_from(value: JsValue) -> Result<Self, Self::Error> {
        match value {
            JsValue::Array(items) => items
                .into_iter()
                .map(|item| item.try_into().map_err(|_| ValueError::UnexpectedType))
                .collect(),
            _ => Err(ValueError::UnexpectedType),
        }
    }
}

impl<'a> From<&'a str> for JsValue {
    fn from(val: &'a str) -> Self {
        JsValue::String(val.into())
    }
}

impl<T> From<Option<T>> for JsValue
where
    T: Into<JsValue>,
{
    fn from(opt: Option<T>) -> Self {
        if let Some(value) = opt {
            value.into()
        } else {
            JsValue::Null
        }
    }
}

/// Error during value conversion.
#[derive(PartialEq, Eq, Debug)]
pub enum ValueError {
    /// Invalid non-utf8 string.
    InvalidString(std::str::Utf8Error),
    /// Encountered string with \0 bytes.
    StringWithZeroBytes(std::ffi::NulError),
    /// Internal error.
    Internal(String),
    /// Received an unexpected type that could not be converted.
    UnexpectedType,
    #[doc(hidden)]
    __NonExhaustive,
}

// TODO: remove this once either the Never type get's stabilized or the compiler
// can properly handle Infallible.
impl From<std::convert::Infallible> for ValueError {
    fn from(_: std::convert::Infallible) -> Self {
        unreachable!()
    }
}

impl fmt::Display for ValueError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        use ValueError::*;
        match self {
            InvalidString(e) => write!(
                f,
                "Value conversion failed - invalid non-utf8 string: {}",
                e
            ),
            StringWithZeroBytes(_) => write!(f, "String contains \\0 bytes",),
            Internal(e) => write!(f, "Value conversion failed - internal error: {}", e),
            UnexpectedType => write!(f, "Could not convert - received unexpected type"),
            __NonExhaustive => unreachable!(),
        }
    }
}

impl error::Error for ValueError {}

#[cfg(test)]
mod tests {
    #[allow(unused_imports)]
    use super::*;

    #[cfg(feature = "bigint")]
    #[test]
    fn test_bigint_from_i64() {
        let int = 1234i64;
        let value = JsValue::from(int);
        if let JsValue::BigInt(value) = value {
            assert_eq!(value.as_i64(), Some(int));
        } else {
            panic!("Expected JsValue::BigInt");
        }
    }

    #[cfg(feature = "bigint")]
    #[test]
    fn test_bigint_from_bigint() {
        let bigint = num_bigint::BigInt::from(std::i128::MAX);
        let value = JsValue::from(bigint.clone());
        if let JsValue::BigInt(value) = value {
            assert_eq!(value.into_bigint(), bigint);
        } else {
            panic!("Expected JsValue::BigInt");
        }
    }

    #[cfg(feature = "bigint")]
    #[test]
    fn test_bigint_i64_bigint_eq() {
        let value_i64 = JsValue::BigInt(1234i64.into());
        let value_bigint = JsValue::BigInt(num_bigint::BigInt::from(1234i64).into());
        assert_eq!(value_i64, value_bigint);
    }
}
