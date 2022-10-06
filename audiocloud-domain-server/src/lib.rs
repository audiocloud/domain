extern crate core;

use derive_more::IsVariant;
use serde::{Deserialize, Serialize};

use audiocloud_api::domain::DomainError;
use audiocloud_api::SecureKey;

pub mod config;
pub mod db;
pub mod events;
pub mod fixed_instances;
pub mod media;
pub mod models;
pub mod nats;
pub mod rest_api;
pub mod sockets;
pub mod tasks;
pub mod tracker;

#[derive(Clone, Copy, Debug, PartialEq, PartialOrd, IsVariant)]
pub enum ResponseMedia {
    MsgPack,
    Json,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, PartialOrd, IsVariant)]
pub enum DomainSecurity {
    Cloud,
    SecureKey(SecureKey),
}

pub type DomainResult<T = ()> = Result<T, DomainError>;
