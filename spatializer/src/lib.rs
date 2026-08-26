#![allow(unused)]

use gluon::{Handler, Node, Ref, RefExt};
use rustc_hash::{FxHashMap, FxHashSet};
use stardust_xr_fusion::{
	client::{Client, ClientHandler},
	fields::{FieldRef, FieldSample, RayMarchResult},
	query::{InterfaceDependency, QueriedInterface, QueryableId},
	spatial::{Spatial, SpatialRef},
	spatial_query::{BeamQuery, BeamQueryHandle, BeamQueryHandler, BeamQueryHandlerHandler},
};
use std::{
	fmt::Debug,
	future::ready,
	hash::Hash,
	sync::{Arc, OnceLock},
};
use tokio::sync::{Notify, RwLock, mpsc};
use tracing::{Instrument, debug_span};

#[derive(Debug, Handler)]
pub struct SpatialInputBeam<Handler: Debug + Clone + Send + Sync + 'static> {
	matching_handlers: RwLock<FxHashMap<QueryableId, Handler>>,
	closest_handler: RwLock<Option<(QueryableId, f32)>>,
	construct: fn(&str, Ref) -> Option<Handler>,
	guard: OnceLock<BeamQueryHandle>,
}
impl<Handler: Debug + Clone + Send + Sync + 'static> SpatialInputBeam<Handler> {
	pub async fn new(
		client: &Client<impl ClientHandler>,
		origin: SpatialRef,
		construct: fn(&str, Ref) -> Option<Handler>,
		interface: String,
		max_length: f32,
	) -> stardust_xr_fusion::Result<Node<Self>> {
		let (node, handler) = BeamQueryHandler::new_node(Self {
			matching_handlers: RwLock::default(),
			closest_handler: RwLock::default(),
			construct,
			guard: OnceLock::new(),
		})?;
		let handler = handler.into_proxy();
		let guard = client
			.spatial_query_interface()
			.beam_query(BeamQuery {
				handler,
				interfaces: vec![InterfaceDependency {
					id: interface,
					optional: false,
				}],
				reference_spatial: origin,
				origin: [0.0; 3].into(),
				direction: [0.0, 0.0, -1.0].into(),
				max_length,
			})
			.await?
			.unwrap();
		node.guard.set(guard);
		Ok(node)
	}
	pub async fn get_handler(&self) -> Option<Handler> {
		self.matching_handlers
			.read()
			.await
			.get(&self.closest_handler.read().await.as_ref()?.0)
			.cloned()
	}
}
impl<Handler: Debug + Clone + Send + Sync + 'static> BeamQueryHandlerHandler
	for SpatialInputBeam<Handler>
{
	async fn intersected(
		&self,
		_ctx: gluon::Context,
		obj: QueryableId,
		field: FieldRef,
		spatial: SpatialRef,
		mut interfaces: Vec<QueriedInterface>,
		sample: RayMarchResult,
	) {
		let interface = interfaces.remove(0);
		let Some(handler) = (self.construct)(&interface.interface_id, interface.interface) else {
			return;
		};
		self.matching_handlers
			.write()
			.await
			.insert(obj, handler);
		if self
			.closest_handler
			.read()
			.await
			.as_ref()
			.is_none_or(|v| v.1 > sample.min_distance)
		{
			self.closest_handler
				.write()
				.await
				.replace((obj, sample.deepest_point_distance));
		}
	}

	fn interfaces_changed(
		&self,
		_ctx: gluon::Context,
		obj: QueryableId,
		interfaces: Vec<QueriedInterface>,
	) -> impl Future<Output = ()> + Send + Sync {
		ready(())
	}

	async fn moved(&self, _ctx: gluon::Context, obj: QueryableId, sample: RayMarchResult) {
		if self
			.closest_handler
			.read()
			.await
			.as_ref()
			.is_none_or(|v| v.1 > sample.min_distance)
		{
			self.closest_handler
				.write()
				.await
				.replace((obj, sample.deepest_point_distance));
		}
	}

	async fn left(&self, _ctx: gluon::Context, obj: QueryableId) {
		self.matching_handlers.write().await.remove(&obj);
		if self
			.closest_handler
			.read()
			.await
			.as_ref()
			.is_some_and(|v| v.0 == obj)
		{
			self.closest_handler.write().await.take();
		}
	}
}
