use bevy::prelude::*;
use bincode::{Decode, Encode};
use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    net::{SocketAddr, UdpSocket},
};

use crate::{
    player::{Inventory, PlayerInput, PlayerPosition},
    world::{BlockType, Terrain, WorldDelta},
};

/// This is the bincode config that we should use everywhere
pub const BINCODE_CONFIG: bincode::config::Configuration = bincode::config::standard()
    .with_little_endian()
    .with_variable_int_encoding()
    .write_fixed_array_length();

/// placeholders
/// TODO: remove whenever command line arguments can be parsed
pub const DEFAULT_SERVER_PORT: u16 = 8888u16;
/// the default server IP that the client looks for
pub const DEFAULT_CLIENT_SERVER_IP: [u8; 4] = [127, 0, 0, 1];

/// buffer size for networking
/// max packet size in UDP is 2^16 bytes
pub const BUFFER_SIZE: usize = (2 as usize).pow(16);

/// Default size of allocated bodies vec, larger numbers may help reduce reallocation
pub const DEFAULT_BODIES_VEC_CAPACITY: usize = 10;

/// How many frames does a client have to not respond for before the server assumes it's dead
pub const FRAME_DIFFERENCE_BEFORE_DISCONNECT: u64 = NETWORK_TICK_HZ * 2;

/// how many times per second will the network tick occur
pub const NETWORK_TICK_HZ: u64 = 60;

/// timestep for sending out network messages
pub const NETWORK_TICK_LABEL: &str = "NETWORK_TICK";

/// how many times per second will the game tick occur
pub const GAME_TICK_HZ: u64 = 60;

/// timestep for doing world calculations
pub const GAME_TICK_LABEL: &str = "GAME_TICK";

/// Marker trait for network structs
pub trait NetworkMessage: Encode + Decode {}

/// Message from the server to a client
#[derive(Encode, Decode, Debug)]
pub struct ServerToClient {
    pub header: ServerHeader,
    pub bodies: Vec<ServerBodyElem>,
}

/// Header for ServerToClient message
#[derive(Encode, Decode, Debug)]
pub struct ServerHeader {
    /// Sequence/tick number
    pub sequence: u64,
}

/// One element (message) for the body of a ServerToClient message
#[derive(Encode, Decode, Debug, Clone)]
pub enum ServerBodyElem {
    /// contains sequence number of ping
    /// TODO: remove
    Pong(u64),
    /// Change in world state, either baseline or delta
    WorldDeltas(Vec<WorldDelta>),
    /// Player location info
    /// 0th element is the client's local player
    PlayerInfo(Vec<SingleNetPlayerInfo>),
    /// The local player's inventory
    Inventory(Inventory),
}

/// Contains information about a single player
#[derive(Encode, Decode, Debug, Clone)]
pub struct SingleNetPlayerInfo {
    pub addr: ClientAddress,
    pub position: PlayerPosition, // TODO: put inputs here if we want client-side prediction
}

impl NetworkMessage for ServerToClient {}

/// Message from a client to the server
#[derive(Encode, Decode, Debug)]
pub struct ClientToServer {
    pub header: ClientHeader,
    pub bodies: Vec<ClientBodyElem>,
}

/// Header for ClientToServer message
#[derive(Encode, Decode, Debug)]
pub struct ClientHeader {
    /// Client's current sequence/tick number
    /// TODO: is this ever useful?
    pub current_sequence: u64,
    /// Last received sequence/tick number
    pub last_received_sequence: u64,
}

/// One element (message) for the body of a ClientToServer message
#[derive(Encode, Decode, Debug, Clone)]
pub enum ClientBodyElem {
    /// asks server to send a pong as a response
    /// pong should contain the sequence number of this packet
    Ping,
    /// sends entire input
    Input(PlayerInput),
}

impl NetworkMessage for ClientToServer {}

#[derive(Debug)]
pub enum SendError {
    IoError(std::io::Error),
    EncodeError(bincode::error::EncodeError),
    //NoSuchPeer,
}

#[derive(Debug)]
pub enum ReceiveError {
    IoError(std::io::Error),
    DecodeError(bincode::error::DecodeError),
    UnknownSender,
    NoMessage,
}

/// Helper method for sending a message
pub fn send_message<M: NetworkMessage>(
    socket: &UdpSocket,
    target: SocketAddr,
    message: M,
    buffer: &mut [u8],
) -> Result<(), SendError> {
    // TODO: use a buffer instead of allocating into vector
    let size = bincode::encode_into_slice(message, buffer, BINCODE_CONFIG)
        .map_err(|e| SendError::EncodeError(e))?;
    // info!("message size: {} bytes", size);
    socket
        .send_to(&buffer[0..size], target)
        .map_err(|e| SendError::IoError(e))?;
    Ok(())
}

/// A component on _all_ players, connected or not
#[derive(Component, Debug, Encode, Decode, Clone, PartialEq, Eq, Hash)]
pub struct ClientAddress {
    pub addr: SocketAddr,
}

impl std::fmt::Display for ClientAddress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.addr)
    }
}

impl ClientAddress {
    pub fn color(&self) -> Color {
        // generate color tint
        let mut hasher = DefaultHasher::new();
        self.addr.to_string().hash(&mut hasher);
        let h = hasher.finish();
        let bytes = h.to_le_bytes();

        // use one byte for each color value
        let r = bytes[0] as f32 / 255.;
        let b = bytes[1] as f32 / 255.;
        let g = bytes[2] as f32 / 255.;

        return Color::rgb(r, b, g);
    }
}
