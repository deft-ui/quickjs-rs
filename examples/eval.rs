use quick_js::console::{ConsoleBackend, Level};
use quick_js::{Context, JsValue};

pub struct Console {

}

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

pub fn main() {
    let context = Context::builder().console(Console::new()).build().unwrap();

    let value = context.eval("() => 123").unwrap();
    println!("function result {:?}", value.call_as_function(Vec::new()).unwrap());

    let r = context.call_js_function(value, Vec::<JsValue>::new()).unwrap();
    println!("result={:?}", r);
    context
        .add_callback("myCallback", |a: i32, b: i32| a + b * b)
        .unwrap();

    let value = context
        .eval(
            r#"
       var x = myCallback(10, 20);
       x;
"#,
        )
        .unwrap();
    println!("js: callback = {:?}", value);
}
