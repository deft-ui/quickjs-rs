use deft_quick_js::exception::HostPromiseRejectionTracker;
use deft_quick_js::{Context, JsValue};

pub struct TestHostPromiseRejectionTrack {
    pub reason: JsValue,
}

impl TestHostPromiseRejectionTrack {
    pub fn new() -> Self {
        Self {
            reason: JsValue::Null,
        }
    }
}

impl HostPromiseRejectionTracker for TestHostPromiseRejectionTrack {
    fn track_promise_rejection(&mut self, _promise: JsValue, reason: JsValue, _is_handled: bool) {
        println!("promise rejection caught: {:?}", reason);
        self.reason = reason;
    }
}

fn main() {
    let mut context = Context::builder().build().unwrap();
    context.set_promise_rejection_tracker(TestHostPromiseRejectionTrack::new());
    let value = context
        .eval(
            "async function test() { throw new Error(111); } test();",
            "test.js",
        )
        .unwrap();
    println!("{:?}", value);
    context.execute_pending_job().unwrap();
}
