//! Log analytics plugin for NGINX UI: searches nginx access logs and feeds the
//! traffic dashboard.

pub mod analytics;
pub mod api;
pub mod app;
pub mod collectors;
pub mod config;
pub mod engine;
pub mod events;
pub mod filesync;
pub mod geo;
pub mod geolite_download;
pub mod localtime;
pub mod logs;
pub mod manifest;
pub mod parse;
pub mod pipeline;
pub mod query;
pub mod schema;
pub mod search;
pub mod sizing;
pub mod state;
pub mod status;
pub mod store;
pub mod sys;
pub mod tokenizer;
pub mod useragent;
