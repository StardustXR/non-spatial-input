use rustc_hash::FxHashMap;
use stardust_xr_fusion::{
	data::{PulseReceiver, PulseSenderHandler},
	fields::Field,
	node::NodeType,
};

#[derive(Debug, Default)]
pub struct PulseReceiverCollector(pub FxHashMap<u64, (PulseReceiver, Field)>);
impl PulseSenderHandler for PulseReceiverCollector {
	fn new_receiver(&mut self, receiver: PulseReceiver, field: Field) {
		self.0
			.insert(receiver.node().get_id().unwrap(), (receiver, field));
	}
	fn drop_receiver(&mut self, uid: u64) {
		self.0.remove(&uid);
	}
}
