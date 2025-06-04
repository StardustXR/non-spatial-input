#![allow(unused)]

use rustc_hash::FxHashMap;
use stardust_xr_fusion::{
	fields::{FieldRef, FieldRefAspect},
	node::NodeType,
	objects::{
		interfaces::FieldRefProxy,
		object_registry::{ObjectInfo, ObjectRegistry},
		FieldRefProxyExt,
	},
	spatial::{SpatialRef, SpatialRefAspect},
};
use std::sync::Arc;
use tokio::sync::{mpsc, Notify};
use tracing::{debug_span, Instrument};
use zbus::{proxy::Defaults, Connection, Proxy};

type FieldCache = FxHashMap<ObjectInfo, FieldRef>;

pub async fn spatial_beam_target(
	conn: Connection,
	object_registry: &ObjectRegistry,
	interface_str: &'static str,
	field_cache: &mut FieldCache,
	beam_origin: &SpatialRef,
) -> Option<ObjectInfo> {
	let handlers = object_registry.get_objects(interface_str);

	let mut join_set = tokio::task::JoinSet::new();
	for handler in &handlers {
		let handler = handler.clone();
		let field_ref = if let Some(cached) = field_cache.get(&handler) {
			cached.clone()
		} else {
			let proxy = handler
				.to_typed_proxy::<FieldRefProxy>(&conn)
				.await
				.unwrap();
			let Some(field_ref) = proxy.import(&beam_origin.client().unwrap()).await else {
				eprintln!("field import was None");
				continue;
			};
			field_cache.insert(handler.clone(), field_ref.clone());
			field_ref
		};

		join_set.spawn({
			let beam_origin = beam_origin.clone();
			async move {
				let result = match field_ref
					.ray_march(&beam_origin, [0.0, 0.0, 0.0], [0.0, 0.0, -1.0])
					.instrument(debug_span!("raymarching"))
					.await
				{
					Ok(r) => r,
					Err(err) => {
						eprintln!("error while raymarching: {err}");
						return None;
					}
				};

				if result.deepest_point_distance > 0.0 && result.min_distance < 0.05 {
					Some((handler, result.deepest_point_distance))
				} else {
					None
				}
			}
		});
	}
	field_cache.retain(|object, _| handlers.contains(object));

	let mut closest_distance = f32::INFINITY;
	let mut closest_handler = None;
	while let Some(result) = join_set.join_next().await {
		if let Ok(Some((handler, distance))) = result {
			if distance < closest_distance {
				closest_distance = distance;
				closest_handler = Some(handler);
			}
		}
	}

	closest_handler
}

#[allow(clippy::too_many_arguments)]
pub async fn spatial_input_beam<P: From<Proxy<'static>> + Defaults + Clone + 'static, E, C>(
	conn: Connection,
	beam_origin: SpatialRef,
	mut events: mpsc::UnboundedReceiver<E>,
	handle_event: impl AsyncFn(&P, E, &mut Option<C>),
	handle_cached_event: impl AsyncFn(&P, C),
	reset: impl AsyncFn(&P),
) {
	let conn = &conn;
	let object_registry = ObjectRegistry::new(conn).await.unwrap();
	let mut field_cache = FxHashMap::<ObjectInfo, FieldRef>::default();
	let mut last_handler: Option<ObjectInfo> = None;
	let mut buf = Vec::new();
	let interface_str = P::INTERFACE.as_ref().unwrap().as_str();
	loop {
		buf.clear();
		events.recv_many(&mut buf, 32).await;

		let Some(closest_handler_object) = spatial_beam_target(
			conn.clone(),
			&object_registry,
			interface_str,
			&mut field_cache,
			&beam_origin,
		)
		.await
		else {
			continue;
		};
		let closest_handler = closest_handler_object
			.to_typed_proxy::<P>(&conn)
			.instrument(debug_span!("getting proxy"))
			.await;

		if let Some(last) = last_handler.take() {
			if last != closest_handler_object {
				if let Ok(last) = last
					.to_typed_proxy::<P>(&conn)
					.instrument(debug_span!("getting proxy"))
					.await
				{
					reset(&last).await;
				}
			}
		}

		let mut cached_state = None;
		if let Ok(handler) = closest_handler {
			for event in buf.drain(..) {
				handle_event(&handler, event, &mut cached_state)
					.instrument(debug_span!("calling handler fn"))
					.await
			}
			if let Some(cached_state) = cached_state {
				handle_cached_event(&handler, cached_state)
					.instrument(debug_span!("calling cached handler"))
					.await
			}
		} else {
			while events.try_recv().is_ok() {}
		}
		last_handler.replace(closest_handler_object);
	}
}
