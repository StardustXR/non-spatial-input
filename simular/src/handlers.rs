use rustc_hash::FxHashMap;
use stardust_xr_fusion::{
	data::{NewReceiverInfo, PulseReceiver, PulseSenderHandler},
	fields::UnknownField,
	input::{InputHandler, InputMethodHandler},
};

#[derive(Debug, Default)]
pub struct PulseReceiverCollector(pub FxHashMap<String, (PulseReceiver, UnknownField)>);
impl PulseSenderHandler for PulseReceiverCollector {
	fn new_receiver(
		&mut self,
		info: NewReceiverInfo,
		receiver: PulseReceiver,
		field: UnknownField,
	) {
		self.0.insert(info.uid, (receiver, field));
	}
	fn drop_receiver(&mut self, uid: &str) {
		self.0.remove(uid);
	}
}

#[derive(Debug, Default)]
pub struct InputHandlerCollector(pub FxHashMap<String, InputHandler>);
impl InputMethodHandler for InputHandlerCollector {
	fn create_handler(&mut self, uid: &str, handler: InputHandler) {
		self.0.insert(uid.to_string(), handler);
	}
	fn drop_handler(&mut self, uid: &str) {
		self.0.remove(uid);
	}
}
