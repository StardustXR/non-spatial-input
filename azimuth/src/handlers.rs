use glam::Vec3;
use rustc_hash::FxHashMap;
use stardust_xr_fusion::{
	data::{PulseReceiver, PulseSenderHandler},
	drawable::Lines,
	fields::{Field, FieldAspect, RayMarchResult},
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
	captured: Option<InputHandler>,
}
impl InputMethodHandler for PointerHandler {
	fn create_handler(&mut self, handler: InputHandler, field: Field) {
		self.handlers
			.insert(handler.node().get_id().unwrap(), (handler, field));
	}
	fn request_capture_handler(&mut self, uid: u64) {
		let Some((new_capture, _)) = self.handlers.get(&uid) else {
			return;
		};
		if self.captured.is_none() {
			self.captured.replace(new_capture.alias());
		}
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
			captured: None,
		}
	}
	pub fn update_pointer(&mut self, pointer_reticle: Lines) {
		if let Some(captured) = self.captured.take() {
			self.pointer.set_handler_order(&[captured.alias()]).unwrap();
			self.pointer.set_captures(&[captured.alias()]).unwrap();
		}

		let mut closest_hits: Option<(Vec<InputHandler>, RayMarchResult)> = None;
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
			while let Some(res) = join.join_next().await {
				let Ok((handler, Ok(ray_info))) = res else {
					continue;
				};
				if ray_info.min_distance > 0.0 {
					continue;
				}
				if let Some((hit_handlers, hit_info)) = &mut closest_hits {
					if ray_info.deepest_point_distance == hit_info.deepest_point_distance {
						hit_handlers.push(handler);
					} else if ray_info.deepest_point_distance < hit_info.deepest_point_distance {
						*hit_handlers = vec![handler];
						*hit_info = ray_info;
					}
				} else {
					closest_hits.replace((vec![handler], ray_info));
				}
			}

			if let Some((hit_handlers, hit_info)) = closest_hits {
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
