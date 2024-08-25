use glam::Vec3;
use rustc_hash::{FxHashMap, FxHashSet};
use stardust_xr_fusion::{
	data::{PulseReceiver, PulseSenderHandler},
	drawable::Lines,
	fields::{Field, FieldRefAspect, RayMarchResult},
	input::{InputHandler, InputMethod, InputMethodAspect, InputMethodHandler},
	node::NodeType,
	spatial::{SpatialAspect, Transform},
};
use tokio::task::JoinSet;

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

#[derive(Debug)]
pub struct PointerHandler {
	pointer: InputMethod,
	handlers: FxHashMap<u64, (InputHandler, Field)>,
	capture_requests: FxHashSet<u64>,
	captured: Option<u64>,
}
impl InputMethodHandler for PointerHandler {
	fn create_handler(&mut self, handler: InputHandler, field: Field) {
		self.handlers
			.insert(handler.node().get_id().unwrap(), (handler, field));
	}
	fn request_capture_handler(&mut self, uid: u64) {
		dbg!(uid);
		self.capture_requests.insert(uid);
	}
	fn destroy_handler(&mut self, uid: u64) {
		self.handlers.remove(&uid);
	}
}
impl PointerHandler {
	pub fn new(pointer: InputMethod) -> Self {
		PointerHandler {
			pointer,
			handlers: FxHashMap::default(),
			capture_requests: FxHashSet::default(),
			captured: None,
		}
	}
	pub fn update_pointer(&mut self, pointer_reticle: Lines) {
		if let Some(captured_id) = self.captured {
			dbg!(captured_id);
			if !self.capture_requests.contains(&captured_id) {
				self.captured = None;
			}
		}
		if self.captured.is_none() {
			self.captured = self.capture_requests.drain().next();
		}
		if let Some((captured, _)) = self.captured.and_then(|id| self.handlers.get(&id)) {
			self.pointer.set_handler_order(&[captured.alias()]).unwrap();
			self.pointer.set_captures(&[captured.alias()]).unwrap();
			return;
		}
		let _ = self.pointer.set_captures(&[]);

		let mut join = JoinSet::new();
		for (handler, field) in self.handlers.values() {
			let handler = handler.alias();
			let field = field.alias();
			let pointer = self.pointer.alias();
			join.spawn(async move {
				(
					handler,
					field.ray_march(&pointer, [0.0; 3], [0.0, 0.0, -1.0]).await,
				)
			});
		}

		let pointer = self.pointer.alias();
		tokio::spawn(async move {
			let mut handlers: Vec<(InputHandler, RayMarchResult)> = Vec::new();
			while let Some(res) = join.join_next().await {
				let Ok((handler, Ok(ray_info))) = res else {
					continue;
				};
				if ray_info.min_distance > 0.0 {
					continue;
				}
				if ray_info.deepest_point_distance < 0.01 {
					continue;
				}
				handlers.push((handler, ray_info));
			}
			let closest_hits = handlers
				.into_iter()
				.map(|(a, b)| (vec![a], b))
				// now collect all handlers that are same distance if they're the closest
				.reduce(|(mut handlers_a, result_a), (handlers_b, result_b)| {
					if (result_a.deepest_point_distance - result_b.deepest_point_distance).abs()
						< 0.001
					{
						// distance is basically the same
						handlers_a.extend(handlers_b);
						(handlers_a, result_a)
					} else if result_a.deepest_point_distance < result_b.deepest_point_distance {
						(handlers_a, result_a)
					} else {
						(handlers_b, result_b)
					}
				});
			// let dbg_info = closest_hits
			// 	.as_ref()
			// 	.map(|(handlers, info)| (handlers.len(), info.deepest_point_distance));
			// dbg!(dbg_info);
			if let Some((hit_handlers, hit_info)) = closest_hits {
				dbg!(hit_handlers.len());
				let _ = pointer.set_handler_order(hit_handlers.as_slice());
				let _ = pointer_reticle.set_relative_transform(
					&pointer,
					Transform::from_translation(
						Vec3::from(hit_info.ray_origin)
							+ Vec3::from(hit_info.ray_direction)
								* hit_info.deepest_point_distance
								* 0.95,
					),
				);
			} else {
				let _ = pointer.set_handler_order(&[]);
				let _ = pointer_reticle.set_relative_transform(
					&pointer,
					Transform::from_translation([0.0, 0.0, -0.5]),
				);
			}
		});
	}
}
