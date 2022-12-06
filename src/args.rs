use std::net::IpAddr;
use std::path::PathBuf;

use clap::{Args, Parser};

use crate::{network, save};

pub fn get_args() -> GameArgs {
    GameArgs::parse()
}

#[derive(Parser, Debug)]
pub enum GameArgs {
    /// Server mode
    Server(ServerArgs),

    /// Client mode
    Client(ClientArgs),
}

#[derive(Args, Debug, Clone)]
// #[command(arg_required_else_help(true))]
pub struct ServerArgs {
    /// File to load and save to
    #[arg(short = 'f', long = "file", default_value_os_t = save::default_save_path_server())]
    pub save_file: PathBuf,

    /// Port to open server on
    #[arg(short = 'p', long, default_value_t = network::DEFAULT_SERVER_PORT)]
    pub port: u16,
}

#[derive(Args, Debug, Clone)]
// #[command(arg_required_else_help(true))]
pub struct ClientArgs {
    /// Address of server
    #[arg(short = 'i', long = "ip", default_value_t = network::DEFAULT_CLIENT_SERVER_IP.into())]
    pub server_ip: IpAddr,

    /// Port of server
    #[arg(short = 'p', long, default_value_t = network::DEFAULT_SERVER_PORT)]
    pub server_port: u16,

    /// Port of client
    #[arg(short = 'c', long, default_value_t = 0)]
    pub client_port: u16,
}
