extern crate core;

use audiocloud_api::domain::DomainError;

pub mod audio_engine;
pub mod config;
pub mod db;
pub mod events;
pub mod instance;
pub mod media;
pub mod rest_api;
pub mod service;
pub mod task;
pub mod tracker;
pub mod web_sockets;
pub mod nats;

#[derive(Clone, Copy)]
pub enum ResponseMedia {
    MsgPack,
    Json,
}

pub type DomainResult<T = ()> = Result<T, DomainError>;
