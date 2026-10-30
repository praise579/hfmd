//! hfmd 的库入口。二进制只负责装配，实现放在各模块里以便单测。

pub mod api;
pub mod cache;
pub mod config;
pub mod download;
pub mod error;
pub mod lock;
pub mod repo;
pub mod segment;
pub mod state;
pub mod verify;
