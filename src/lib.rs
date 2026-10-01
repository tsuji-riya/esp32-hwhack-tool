#![recursion_limit = "512"]
#![no_std]

extern crate alloc;

pub mod config;
pub mod flash_hub;
pub mod flash_ws;
pub mod uart_bridge;
pub mod web;
pub mod wifi;
