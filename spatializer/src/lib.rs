#![allow(unused)]

use gluon::{Handler, Object, ObjectOrRef};
use rustc_hash::{FxHashMap, FxHashSet};
use stardust_xr_fusion::{
	client::{Client, ClientHandler},
	fields::{FieldRef, FieldSample, RayMarchResult},
	query::{InterfaceDependency, QueriedInterface, QueryableObjectRef},
	spatial::{Spatial, SpatialRef},
	spatial_query::{BeamQuery, BeamQueryHandler, BeamQueryHandlerHandler, SpatialQueryGuard},
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
	matching_handlers: RwLock<FxHashMap<QueryableObjectRef, Handler>>,
	closest_handler: RwLock<Option<(QueryableObjectRef, f32)>>,
	construct: fn(&str, ObjectOrRef) -> Option<Handler>,
	guard: OnceLock<SpatialQueryGuard>,
}
impl<Handler: Debug + Clone + Send + Sync + 'static> SpatialInputBeam<Handler> {
	pub async fn new(
		client: &Client<impl ClientHandler>,
		origin: SpatialRef,
		construct: fn(&str, ObjectOrRef) -> Option<Handler>,
		interface: String,
		max_length: f32,
	) -> stardust_xr_fusion::Result<Object<Self>> {
		let handler = client.pion_device().register_object(Self {
			matching_handlers: RwLock::default(),
			closest_handler: RwLock::default(),
			construct,
			guard: OnceLock::new(),
		});
		let guard = client
			.spatial_query_interface()
			.beam_query(BeamQuery {
				handler: BeamQueryHandler::from_handler(&handler),
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
		handler.guard.set(guard);
		Ok(handler)
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
		obj: QueryableObjectRef,
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
			.insert(obj.clone(), handler);
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
		obj: QueryableObjectRef,
		interfaces: Vec<QueriedInterface>,
	) -> impl Future<Output = ()> + Send + Sync {
		ready(())
	}

	async fn moved(&self, _ctx: gluon::Context, obj: QueryableObjectRef, sample: RayMarchResult) {
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

	async fn left(&self, _ctx: gluon::Context, obj: QueryableObjectRef) {
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
