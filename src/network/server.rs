use super::*;
use crate::{
    player::PlayerInput,
    states,
    world::{self, Terrain},
};
use bevy::prelude::*;
use iyes_loopless::prelude::*;
use std::{
    collections::HashMap,
    net::{SocketAddr, UdpSocket},
    path::PathBuf,
};

const MAX_CLIENTS: usize = 2; // final goal = 2, strech goal = 4

// holds command line info for server creation
pub struct ServerInfo {
    /// port
    pub port: u16,
    /// save file
    pub save_file: PathBuf,
}

/// Should be used as a global resource on the server
struct Server {
    /// UDP socket that should be used for everything
    socket: UdpSocket,
    /// HashMap of clients using the socket address as the key
    clients: HashMap<SocketAddr, ClientInfo>,
    /// The current sequence/tick number
    sequence: u64,
    /// Incoming buffer
    buffer: [u8; BUFFER_SIZE],
}

/// Information about a client
#[derive(Debug)]
struct ClientInfo {
    /// The socket address
    addr: SocketAddr,
    /// The last confirmed sequence number
    last_ack: u64,
    /// Body elements that we build up
    bodies: Vec<ServerBodyElem>,
    /// How many frames until we drop it
    until_drop: u64,
    /// Player inputs
    inputs: PlayerInput,
}

impl ClientInfo {
    fn new(addr: SocketAddr) -> Self {
        ClientInfo {
            addr,
            last_ack: 0,
            bodies: Vec::with_capacity(DEFAULT_BODIES_VEC_CAPACITY),
            until_drop: FRAME_DIFFERENCE_BEFORE_DISCONNECT,
            inputs: PlayerInput::default(),
        }
    }
}

impl Server {
    /// Binds the socket
    fn new(port: u16) -> Result<Self, std::io::Error> {
        let addr = SocketAddr::from((DEFAULT_SERVER_IP, port));
        let sock = UdpSocket::bind(addr)?;

        // we want nonblocking sockets!
        sock.set_nonblocking(true)?;

        Ok(Server {
            socket: sock,
            clients: HashMap::with_capacity(MAX_CLIENTS * 2), // avoid resizing (default capacity is 16).,
            sequence: 1u64,
            buffer: [0u8; BUFFER_SIZE],
        })
    }

    /// Send message to a specific client
    fn send_message(
        &self,
        client_addr: SocketAddr,
        message: ServerToClient,
    ) -> Result<(), SendError> {
        match &self.clients.get(&client_addr) {
            Some(client) => {
                send_message(&self.socket, client.addr, message)?;
                Ok(())
            }
            None => Err(SendError::NoSuchPeer),
        }
    }

    /// Non-blocking way to get one message from the socket
    fn get_one_message(&mut self) -> Result<(&mut ClientInfo, ClientToServer), ReceiveError> {
        // read from socket
        let (_size, sender_addr) = self.socket.recv_from(&mut self.buffer).map_err(|e| match e
            .kind()
        {
            std::io::ErrorKind::WouldBlock => ReceiveError::NoMessage,
            _ => ReceiveError::IoError(e),
        })?;

        // decode
        let (message, _size) = bincode::decode_from_slice(&self.buffer, BINCODE_CONFIG)
            .map_err(ReceiveError::DecodeError)?;

        // if the server recieves a msg from a new client
        if !self.clients.contains_key(&sender_addr) {
            // if at max clients, return error
            if self.clients.len() == MAX_CLIENTS {
                return Err(ReceiveError::UnknownSender);
            }
            // add the new client
            self.clients
                .insert(sender_addr, ClientInfo::new(sender_addr));
        }

        // unwrap OK because we just guaranteed the client is in our HashMap
        Ok((self.clients.get_mut(&sender_addr).unwrap(), message))
    }
}

/// Bevy plugin that implements server logic
pub struct ServerPlugin {
    pub port: u16,
    pub save_file: PathBuf,
}

impl Plugin for ServerPlugin {
    fn build(&self, app: &mut App) {
        // insert server info as resource
        app.insert_resource(ServerInfo{port: self.port, save_file: self.save_file.clone()});

        // add game tick
        app.add_fixed_timestep(
            std::time::Duration::from_secs_f64(1. / GAME_TICK_HZ as f64),
            GAME_TICK_LABEL,
        );

        // add network tick
        app.add_fixed_timestep(
            std::time::Duration::from_secs_f64(1. / NETWORK_TICK_HZ as f64),
            NETWORK_TICK_LABEL,
        );

        // enter systems
        app.add_enter_system(states::server::GameState::Running, create_server);

        // exit systems
        app.add_exit_system(states::server::GameState::Running, destroy_server);

        // game tick systems
        app.add_fixed_timestep_system(
            GAME_TICK_LABEL,
            0,
            server_handle_messages
                .run_in_state(states::server::GameState::Running)
                .label("handle_messages"),
        )
        .add_fixed_timestep_system(
            GAME_TICK_LABEL,
            0,
            || {}, // TODO: simulate physics
        );

        // TODO: add run condition to only run if self.clients.len() > 0
        // network tick systems
        app.add_fixed_timestep_system(
            NETWORK_TICK_LABEL,
            0,
            increase_network_tick
                .run_in_state(states::server::GameState::Running)
                .label("increase_network_tick"),
        )
        .add_fixed_timestep_system(
            NETWORK_TICK_LABEL,
            0,
            process_player_mining
                .run_in_state(states::server::GameState::Running)
                .label("process_player_mining")
                .after("increase_network_tick"),
        )
        .add_fixed_timestep_system(
            NETWORK_TICK_LABEL,
            0,
            enqueue_terrain
                .run_in_state(states::server::GameState::Running)
                .label("enqueue_terrain")
                .after("increase_network_tick"),
        )
        .add_fixed_timestep_system(
            NETWORK_TICK_LABEL,
            0,
            send_all_messages
                .run_in_state(states::server::GameState::Running)
                .after("enqueue_terrain")
                .label("send_messages"),
        )
        .add_fixed_timestep_system(
            NETWORK_TICK_LABEL,
            0,
            drop_disconnected_clients
                .run_in_state(states::server::GameState::Running)
                .after("send_messages")
                .label("drop_disconnected"),
        );
    }
}

fn create_server(mut commands: Commands, server_info: Res<ServerInfo>) {
    // TODO: use command line arguments for port and handle failure better
    let server = match Server::new(server_info.port) {
        Ok(s) => s,
        Err(e) => panic!("Unable to create server: {}", e),
    };

    commands.insert_resource(server);

    info!("server created");
}

fn destroy_server(mut commands: Commands) {
    commands.remove_resource::<Server>();
}

/// Server increase tick count
fn increase_network_tick(mut server: ResMut<Server>) {
    server.sequence += 1;
}

fn process_player_mining(
    server: Res<Server>,
    mut terrain: ResMut<Terrain>,
    mut commands: Commands,
) {
    for (addr, client) in &server.clients {
        let inputs = &client.inputs;
        if inputs.mine {
            let res =
                world::destroy_block(inputs.block_x, inputs.block_y, &mut commands, &mut terrain);
            match res {
                Ok(block) => {
                    info!(
                        "player {} destroyed block at ({}, {}): {:?}",
                        addr, inputs.block_x, inputs.block_y, block.block_type
                    );
                }
                Err(err) => {
                    error!(
                        "player {} unable to destroy block at ({}, {}): {:?}",
                        addr, inputs.block_x, inputs.block_y, err
                    );
                }
            }
        }
    }
}

/// Server system
fn server_handle_messages(mut server: ResMut<Server>) {
    loop {
        // handle all messages on our socket
        match server.get_one_message() {
            Ok((client, message)) => {
                compute_new_bodies(client, message);
            }
            Err(ReceiveError::NoMessage) => {
                // break whenever we run out of messages
                break;
            }
            Err(ReceiveError::UnknownSender) => {
                warn!("server recieve error: server is full!");
            }
            Err(e) => {
                // anything else is a "real" error that we should complain about
                error!("server receive error: {:?}", e);
            }
        }
    }
}

/// Process a client's message and push new bodies to the next packet sent to the client
/// TODO: will probably need direct World access in the future
fn compute_new_bodies(client: &mut ClientInfo, message: ClientToServer) {
    // TODO: just impl Display or Debug instead
    let mut bodies_str = "".to_string();
    for body in &message.bodies {
        bodies_str.push_str(match body {
            ClientBodyElem::Ping => "ping,",
            ClientBodyElem::Input(_) => "input,",
        });
    }
    info!(
        "server got message from client @ {} with {} bodies: {}",
        client.addr,
        message.bodies.len(),
        bodies_str
    );

    let mut in_order = false;

    // this message is in-order
    // TODO: whenever the clients send inputs, ignore any that are out of order
    // i.e. only use the most recent input
    if message.header.last_received_sequence > client.last_ack {
        client.last_ack = message.header.last_received_sequence;
        client.bodies.clear();

        // reset its drop timer
        client.until_drop = FRAME_DIFFERENCE_BEFORE_DISCONNECT;

        in_order = true;
    }

    // compute our direct responses
    let mut body_elems: Vec<ServerBodyElem> = message
        .bodies
        .iter()
        // match client bodies to server bodies
        .filter_map(|elem| match elem {
            ClientBodyElem::Ping => Some(ServerBodyElem::Pong(message.header.current_sequence)),
            ClientBodyElem::Input(input) => {
                if in_order {
                    info!("server inputs for client {}", client.addr);
                    // add inputs to server
                    client.inputs = input.clone();
                }

                // never respond directly to input bodies
                None
            }
        })
        .collect();

    // info!(
    //     "server responses += {}",
    //     // debug format of all new elems
    //     body_elems.iter().fold(String::new(), |mut acc, s| {
    //         acc.push_str(&format!("({:?}) ", s));
    //         acc
    //     })
    // );

    // queue up our responses to be sent our in the next packet
    client.bodies.append(&mut body_elems);

    // only keep pongs that are in response to a ping newer than or equals to the client's last_ack
    client.bodies.retain(|elem| match elem {
        ServerBodyElem::Pong(seq) => *seq >= client.last_ack,
        ServerBodyElem::Terrain(_) => true, // always keep terrains
    });
}

fn send_all_messages(mut server: ResMut<Server>) {
    // loop over clients
    for (client_addr, client_info) in &server.clients {
        let message = ServerToClient {
            header: ServerHeader {
                sequence: server.sequence,
            },
            bodies: client_info.bodies.clone(),
        };

        // form message via borrow before consuming it
        let success_msg = format!("server sent message to {:?}", client_info.addr);
        match server.send_message(*client_addr, message) {
            Ok(_) => info!("{}", success_msg),
            Err(e) => error!("server unable to send message: {:?}", e),
        }
    }

    // filter out client bodies
    for client_info in server.clients.values_mut() {
        client_info.bodies.retain(|b| match b {
            ServerBodyElem::Pong(_) => true, // keep pongs until we know they were received
            ServerBodyElem::Terrain(_) => false, // never keep old terrains
        });
    }
}

/// Add the terrain to the next packet sent
/// TODO: convert to delta and baseline
/// TODO: use reference for terrain instead of clone?
fn enqueue_terrain(mut server: ResMut<Server>, terrain: Res<Terrain>) {
    for client in server.clients.values_mut() {
        client.bodies.push(ServerBodyElem::Terrain(terrain.clone()));
        info!("enqueued terrain");
    }
}

fn drop_disconnected_clients(mut server: ResMut<Server>) {
    // drop clients that haven't responded in a while
    server.clients.retain(|address, client| {
        let keep = client.until_drop > 0;
        if !keep {
            warn!("dropping client {}", address);
        }

        keep
    });

    // loop through active clients
    for client_info in server.clients.values_mut() {
        client_info.until_drop -= 1;
    }
}
