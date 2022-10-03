extern crate core;

use derive_more::IsVariant;

use audiocloud_api::domain::DomainError;

pub mod config;
pub mod db;
pub mod events;
pub mod fixed_instances;
pub mod media;
pub mod nats;
pub mod rest_api;
pub mod sockets;
pub mod tasks;
pub mod tracker;
pub mod models;

#[derive(Clone, Copy, IsVariant)]
pub enum ResponseMedia {
    MsgPack,
    Json,
}

pub type DomainResult<T = ()> = Result<T, DomainError>;
