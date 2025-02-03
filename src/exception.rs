use crate::JsValue;

pub trait HostPromiseRejectionTracker {
    fn track_promise_rejection(&mut self, promise: JsValue, reason: JsValue, is_handled: bool);
}

pub struct HostPromiseRejectionTrackerWrapper {
    pub tracker: Box<dyn HostPromiseRejectionTracker>,
}

impl HostPromiseRejectionTrackerWrapper {
    pub fn new(tracker: Box<dyn HostPromiseRejectionTracker>) -> Self {
        Self { tracker }
    }
}

pub struct DumpHostPromiseRejectionTracker {}

impl DumpHostPromiseRejectionTracker {
    pub fn new() -> Self {
        Self {}
    }
}

impl HostPromiseRejectionTracker for DumpHostPromiseRejectionTracker {
    fn track_promise_rejection(&mut self, _promise: JsValue, reason: JsValue, _is_handled: bool) {
        println!("uncaught promise rejection: {:?}", reason);
    }
}
