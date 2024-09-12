use std::cell::RefCell;
use std::rc::Rc;
use quick_js::{Context, JsValue, ResourceValue};
use quick_js::console::{ConsoleBackend, Level};

pub struct Console {}

impl Console {
    pub fn new() -> Self {
        Self {}
    }
}

impl ConsoleBackend for Console {
    fn log(&self, level: Level, values: Vec<JsValue>) {
        println!("{}:{:?}", level, values);
    }
}

struct MyResource {
    text: String,
}

impl Drop for MyResource {
    fn drop(&mut self) {
        println!("my class dropped");
    }
}

pub fn main() {
    let context = Context::builder().console(Console::new()).build().unwrap();
    let resource = MyResource {
        text: "test".to_string(),
    };
    let js_value = JsValue::Resource(ResourceValue {
        resource: Rc::new(RefCell::new(resource)),
    });
    context.add_callback("print", |msg: JsValue| {
        println!("{:?}", msg);
        if let Some(txt) = msg.as_resource(|r: &mut MyResource| r.text.to_string()) {
            println!("callback text={}", txt);
        } else {
            println!("Not a resource");
        }
        0
    }).unwrap();
    context.set_global("rs", js_value).unwrap();
    context.eval(r"print(rs)").unwrap();
}