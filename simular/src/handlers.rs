use rustc_hash::FxHashMap;
use stardust_xr_fusion::{
	data::{PulseReceiver, PulseSenderHandler},
	fields::Field,
	input::{InputHandler, InputMethodHandler},
};

#[derive(Debug, Default)]
pub struct PulseReceiverCollector(pub FxHashMap<String, (PulseReceiver,  Field)>);
impl PulseSenderHandler for PulseReceiverCollector {
	fn new_receiver(&mut self, uid: String, receiver: PulseReceiver, field:  Field) {
		self.0.insert(uid, (receiver, field));
	}
	fn drop_receiver(&mut self, uid: String) {
		self.0.remove(&uid);
	}
}

#[derive(Debug, Default)]
pub struct InputHandlerCollector(pub FxHashMap<String, InputHandler>);
impl InputMethodHandler for InputHandlerCollector {
	fn create_handler(&mut self, uid: String, handler: InputHandler, _field: Field) {
		self.0.insert(uid, handler);
	}

	fn request_capture_handler(&mut self, _uid: String) {}

	fn destroy_handler(&mut self, uid: String) {
		self.0.remove(&uid);
	}
}
