use super::*;
use crate::{
    args::ServerArgs,
    player::{
        server::{handle_movement, JumpDuration, JumpState},
        Inventory, PlayerInput, PlayerPosition,
    },
    states,
    world::{
        self, server::check_generate_new_chunks, BlockDelete, Terrain, WorldDelta, CHUNK_HEIGHT,
        CHUNK_WIDTH,
    },
};
use bevy::prelude::*;
use iyes_loopless::prelude::*;
use std::{
    collections::{HashMap, VecDeque},
    f32::consts::E,
    net::{SocketAddr, UdpSocket},
};

pub const MESSAGE_QUEUE_SIZE: usize = 20;

/// Should be used as a global resource on the server
pub struct Server {
    /// UDP socket that should be used for everything
    socket: UdpSocket,
    /// The current sequence/tick number
    sequence: u64,
    /// Incoming buffer
    buffer: [u8; BUFFER_SIZE],
}

/// Helper resource to decouple message reception and processing
#[derive(Default)]
struct Messages {
    messages: VecDeque<(SocketAddr, ClientToServer)>,
}

/// Information about a client, stored as a component on players that are connected
#[derive(Component, Debug)]
pub struct ConnectedClientInfo {
    /// The last confirmed sequence number
    pub last_ack: u64,
    /// Body elements that we build up
    pub bodies: Vec<ServerBodyElem>,
    /// How many frames until we drop it
    pub until_drop: u64,
    /// Last confirmed world state (only the chunks that it knows about)
    pub last_confirmed_terrain: Terrain,
    /// Map of sequence numbers to deltas sent
    pub deltas: HashMap<u64, Vec<WorldDelta>>,
}

impl Default for ConnectedClientInfo {
    fn default() -> Self {
        ConnectedClientInfo {
            last_ack: 0, // must be set immediately after creation
            bodies: Vec::with_capacity(DEFAULT_BODIES_VEC_CAPACITY),
            until_drop: FRAME_DIFFERENCE_BEFORE_DISCONNECT,
            last_confirmed_terrain: Terrain::empty(),
            deltas: HashMap::new(),
        }
    }
}

impl Server {
    /// Binds the socket
    fn new(port: u16) -> Result<Self, std::io::Error> {
        let addr = SocketAddr::from(([0, 0, 0, 0], port));
        let sock = UdpSocket::bind(addr)?;

        // we want nonblocking sockets!
        sock.set_nonblocking(true)?;

        info!("bound socket: {:?}", sock);

        Ok(Server {
            socket: sock,
            sequence: 1u64,
            buffer: [0u8; BUFFER_SIZE],
        })
    }

    /// Send message to a specific client
    fn send_message(
        &mut self,
        client_addr: SocketAddr,
        message: ServerToClient,
    ) -> Result<(), SendError> {
        // TODO: check if address is acually a connected client via a query?
        send_message(&self.socket, client_addr, message, &mut self.buffer)?;
        Ok(())
    }

    /// Non-blocking way to get one message from the socket
    /// Can receive messages from _any_ address, not just connected clients
    fn get_one_message(&mut self) -> Result<(SocketAddr, ClientToServer), ReceiveError> {
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

        // unwrap OK because we just guaranteed the client is in our HashMap
        Ok((sender_addr, message))
    }
}

/// Bevy plugin that implements server logic
pub struct ServerPlugin {
    pub args: ServerArgs,
}

impl Plugin for ServerPlugin {
    fn build(&self, app: &mut App) {
        // add arguments
        app.insert_resource(self.args.clone());

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
            retrieve_messages
                .run_in_state(states::server::GameState::Running)
                .label("retrieve_messages"),
        )
        .add_fixed_timestep_system(
            GAME_TICK_LABEL,
            0,
            handle_messages
                .run_in_state(states::server::GameState::Running)
                .label("handle_messages")
                .after("retrieve_messages"),
        )
        .add_fixed_timestep_system(
            GAME_TICK_LABEL,
            0,
            check_generate_new_chunks
                .run_in_state(states::server::GameState::Running)
                .label("check_generate_new_chunks")
                .after("handle_messages"),
        )
        .add_fixed_timestep_system(
            GAME_TICK_LABEL,
            0,
            handle_movement
                .run_in_state(states::server::GameState::Running)
                .label("handle_movement")
                .after("check_generate_new_chunks"),
        );

        // debug print player info
        // app.add_fixed_timestep_system(
        //     NETWORK_TICK_LABEL,
        //     0,
        //     debug_print_players
        //         .run_in_state(states::server::GameState::Running)
        //         .label("debug_print_players"),
        // );

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
            enqueue_player_info
                .run_in_state(states::server::GameState::Running)
                .label("enqueue_player_info")
                .after("increase_network_tick"),
        )
        .add_fixed_timestep_system(
            NETWORK_TICK_LABEL,
            0,
            enqueue_inventory
                .run_in_state(states::server::GameState::Running)
                .label("enqueue_inventory")
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
                .after("enqueue_player_info")
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

fn create_server(mut commands: Commands, args: Res<ServerArgs>) {
    // TODO: use command line arguments for port and handle failure better
    let server = match Server::new(args.port) {
        Ok(s) => s,
        Err(e) => panic!("Unable to create server: {}", e),
    };

    commands.insert_resource(server);

    commands.insert_resource(Messages::default());

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
    mut query: Query<(
        &ClientAddress,
        &PlayerInput,
        &mut ConnectedClientInfo,
        &mut Inventory,
    )>,
    mut terrain: ResMut<Terrain>,
    mut commands: Commands,
) {
    for (addr, inputs, mut client, mut inventory) in query.iter_mut() {
        if inputs.mine {
            // destroy the block
            let res = world::server::destroy_block(
                inputs.block_x,
                inputs.block_y,
                &mut commands,
                &mut terrain,
            );
            //we really care what happens because of inventory
            match res {
                Ok(block) => {
                    // modify inventory
                    match inventory.amounts.get_mut(&block.block_type) {
                        Some(amount) => {
                            *amount += 1;
                        }
                        None => {
                            error!("block_type {:?} not in inventory??", block.block_type);
                        }
                    }

                    // info!(
                    //     "player {} destroyed block at ({}, {}): {:?}, new inv: {:?}",
                    //     addr, inputs.block_x, inputs.block_y, block.block_type, inventory
                    // );
                }
                Err(_err) => {
                    // error!(
                    //     "player {} unable to destroy block at ({}, {}): {:?}",
                    //     addr, inputs.block_x, inputs.block_y, err
                    // );
                }
            }
        }
    }
}

/// Server system that runs on _every_ frame
/// Places messages into Messages resource
fn retrieve_messages(mut server: ResMut<Server>, mut messages: ResMut<Messages>) {
    // loop until we break (on NoMessage)
    loop {
        // handle all messages on our socket
        match server.get_one_message() {
            Ok(m) => {
                // put into resource

                // info!("message queue size: {}", messages.messages.len());
                if messages.messages.len() > MESSAGE_QUEUE_SIZE {
                    warn!(
                        "trashing messages due to overflow! current message queue size: {}",
                        messages.messages.len()
                    );
                }

                while messages.messages.len() > MESSAGE_QUEUE_SIZE {
                    messages.messages.pop_front();
                }
                messages.messages.push_back(m);
            }
            Err(ReceiveError::NoMessage) => {
                // break whenever we run out of messages
                break;
            }
            Err(ReceiveError::UnknownSender) => {
                warn!("server recieve error: server is full!");
            }
            #[cfg(target_os = "windows")]
            Err(ReceiveError::IoError(e)) if e.kind() == std::io::ErrorKind::ConnectionReset => {
                // ignore
                // why does windows even do this?? UDP is connectionless
            }
            Err(e) => {
                // anything else is a "real" error that we should complain about
                error!("server receive error: {:?}", e);
            }
        }
    }
}

/// System that handles all messages from the Messages resource
fn handle_messages(
    mut messages: ResMut<Messages>,
    mut commands: Commands,
    mut query: Query<(
        Entity,
        &ClientAddress,
        Option<&mut ConnectedClientInfo>,
        &mut PlayerInput,
    )>,
) {
    /*
    We have to handle several different cases and we need immediate access
    to all components (spawn() has a 1-tick delay), so if needed, we create
    the components inside this function, call process_client_message,
    then add the components to the entity
    */

    // process all messages from new clients all together at the end of this function,
    // since entities aren't spawned until next frame
    let mut new_clients: HashMap<SocketAddr, Vec<ClientToServer>> = HashMap::new();

    // for each message
    while let Some((addr, message)) = messages.messages.pop_front() {
        let mut entity: Option<Entity> = None;

        // check if we have a player at this address already
        for (e, client_addr, _, _) in query.iter() {
            if client_addr.addr == addr {
                entity = Some(e)
            }
        }

        match entity {
            Some(entity) => {
                // client is either currently connected or has connected before
                // unwrap OK since we iterated to find it above
                let e = query.get_mut(entity).unwrap();

                // unpack tuple here for readability
                let maybe_connected = e.2;
                let mut input = e.3;

                match maybe_connected {
                    Some(mut connected) => {
                        // client is currently connected

                        // process the client message
                        process_client_message(&addr, &mut connected, message, &mut input);
                    }
                    None => {
                        // client has connected before, but timed out
                        info!("reconnection from {}", addr);
                        let mut connected = ConnectedClientInfo::default();

                        // process the client message
                        process_client_message(&addr, &mut connected, message, &mut input);

                        // add connected to the entity
                        commands.entity(entity).insert(connected);

                        // add other connected-only components to entity
                        commands
                            .entity(entity)
                            .insert(JumpDuration::default())
                            .insert(JumpState::default());
                    }
                };
            }
            None => {
                // if we already got a message from this new client this frame
                if let Some(mut client_messages) = new_clients.get_mut(&addr) {
                    client_messages.push(message);
                } else {
                    // else this is the first messages from this new client this frame
                    new_clients.insert(addr.clone(), vec![message]);
                }
            }
        }
    }

    for (addr, c_messages) in new_clients {
        // new connection
        let client_addr = ClientAddress { addr };
        let position = PlayerPosition::default();
        let mut input = PlayerInput::default();
        let jump_dur = JumpDuration::default();
        let jump_state = JumpState::default();
        let inventory = Inventory::default();
        let mut connected = ConnectedClientInfo::default();

        info!("new connection from {}", client_addr);

        for message in c_messages {
            // process the message
            process_client_message(&client_addr.addr, &mut connected, message, &mut input);
        }

        // create entity with components
        // ONLY once per new client
        commands
            .spawn()
            .insert(client_addr)
            .insert(position)
            .insert(input)
            .insert(connected)
            .insert(jump_dur)
            .insert(jump_state)
            .insert(inventory);
    }
}

/// Process a client's message and push new bodies to the next packet sent to the client
/// Uses client message info to overwrite player input components
fn process_client_message(
    addr: &SocketAddr,
    client: &mut ConnectedClientInfo,
    message: ClientToServer,
    input: &mut PlayerInput,
) {
    // TODO: just impl Display or Debug instead
    let mut bodies_str = "".to_string();
    for body in &message.bodies {
        bodies_str.push_str(match body {
            ClientBodyElem::Ping => "ping,",
            ClientBodyElem::Input(_) => "input,",
        });
    }
    // info!(
    //     "server got message from client @ {} with {} bodies: {}",
    //     addr,
    //     message.bodies.len(),
    //     bodies_str
    // );

    let mut in_order = false;

    // this message is in-order
    // TODO: whenever the clients send inputs, ignore any that are out of order
    // i.e. only use the most recent input
    if message.header.last_received_sequence > client.last_ack {
        client.last_ack = message.header.last_received_sequence;
        client.bodies.clear(); // clear any pending pings

        // get the changes we need to apply to our baseline
        let changes = client.deltas.get(&client.last_ack);

        match changes {
            // apply the changes
            Some(changes) => {
                for change in changes {
                    match change {
                        WorldDelta::NewChunks(terrain) => {
                            // replace entire terrain
                            client.last_confirmed_terrain = terrain.clone();
                        }
                        WorldDelta::BlockDelete(delete) => {
                            // delete single block

                            // find chunk
                            for mut chunk in &mut client.last_confirmed_terrain.chunks {
                                if chunk.chunk_number == delete.chunk_number {
                                    // delete the block
                                    chunk.blocks[delete.y][delete.x] = None;
                                }
                            }
                        }
                    }
                }
            }
            None => {
                error!(
                    "client ack'd a message that doesn't have a stored changelist?: {}",
                    client.last_ack
                );
            }
        }

        // drop all old stored changes
        client
            .deltas
            .retain(|&seq_num, _| seq_num > client.last_ack);

        // reset client's drop timer
        client.until_drop = FRAME_DIFFERENCE_BEFORE_DISCONNECT;

        // this message was in-order
        in_order = true;
    }

    // compute our direct responses
    let mut body_elems: Vec<ServerBodyElem> = message
        .bodies
        .iter()
        // match client bodies to server bodies
        .filter_map(|elem| match elem {
            ClientBodyElem::Ping => Some(ServerBodyElem::Pong(message.header.current_sequence)),
            ClientBodyElem::Input(new_input) => {
                if in_order {
                    // info!("server got inputs for client {}", addr);
                    // add inputs to player entity's input component
                    *input = new_input.clone();
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
        _ => true, // keep everything else
    });
}

fn send_all_messages(
    mut server: ResMut<Server>,
    mut query: Query<(&ClientAddress, &mut ConnectedClientInfo)>,
) {
    // loop over clients
    for (client_addr, client_info) in query.iter_mut() {
        let message = ServerToClient {
            header: ServerHeader {
                sequence: server.sequence,
            },
            bodies: client_info.bodies.clone(),
        };

        // form message via borrow before consuming it
        let success_msg = format!("server sent message to {:?}", client_addr);
        match server.send_message(client_addr.addr, message) {
            Ok(_) => {
                // info!("{}", success_msg),
            }
            Err(e) => error!("server unable to send message: {:?}", e),
        }
    }

    // filter out client bodies
    for (_, mut client_info) in query.iter_mut() {
        client_info.bodies.retain(|b| match b {
            ServerBodyElem::Pong(_) => true, // keep pongs until we know they were received
            _ => false,                      // never keep anything else
        });
    }
}

/// Add the terrain to the next packet sent
/// TODO: convert to delta and baseline
/// TODO: use reference for terrain instead of clone?
fn enqueue_terrain(
    terrain: Res<Terrain>,
    server: Res<Server>,
    mut clients: Query<(&ClientAddress, &mut ConnectedClientInfo, &PlayerPosition)>,
) {
    for (addr, mut client, player_position) in clients.iter_mut() {
        // the number of the chunk that the player is in
        let player_chunk = -(player_position.y) as usize / CHUNK_HEIGHT as usize;
        let chunk_range = if player_chunk == 0 {
            0..=1
        } else {
            (player_chunk - 1)..=(player_chunk + 1)
        };

        // info!("enqueuing partial terrain {:?} to {}", chunk_range, addr);

        // chunks that the client has
        let client_chunks: Vec<u64> = client
            .last_confirmed_terrain
            .chunks
            .iter()
            .map(|c| c.chunk_number)
            .collect();

        // check if the client doesn't have a chunk that it should
        let mut needs_baseline = false;
        for chunk_num in chunk_range.clone() {
            // check if client is missing this chunk number
            let mut filter = client_chunks.iter().filter(|c| **c == chunk_num as u64);
            if filter.next().is_none() {
                // if it is missing a chunk, it needs a new baseline
                needs_baseline = true;
            }
        }

        let mut world_changes = Vec::new();

        if needs_baseline {
            // resend the entire baseline!
            // the terrain we will send them
            let mut baseline = Terrain::empty();
            // clone in only specified chunks
            for chunk_number in chunk_range {
                baseline.chunks.push(terrain.chunks[chunk_number].clone())
            }

            // push it
            world_changes.push(WorldDelta::NewChunks(baseline));
        } else {
            // just calcluate the block deletions
            for client_chunk in &mut client.last_confirmed_terrain.chunks {
                let chunk_num = client_chunk.chunk_number;

                // server chunks are always at their correct index
                let server_chunk = terrain.chunks.get(chunk_num as usize);
                match server_chunk {
                    Some(server_chunk) => {
                        // loop over blocks in chunk
                        for y in 0..CHUNK_HEIGHT {
                            for x in 0..CHUNK_WIDTH {
                                // if the client chunk has a block here but server doesn't
                                if client_chunk.blocks[y][x].is_some()
                                    && server_chunk.blocks[y][x].is_none()
                                {
                                    // create delta (deletion)
                                    let block_deletion = BlockDelete {
                                        chunk_number: chunk_num,
                                        x,
                                        y,
                                    };
                                    // push it to the client
                                    world_changes.push(WorldDelta::BlockDelete(block_deletion));
                                }
                            }
                        }
                    }
                    None => {
                        error!(
                            "client somehow has chunk that server doesn't have: {}",
                            chunk_num
                        );
                    }
                }
            }
        }

        // send client these deltas
        client
            .bodies
            .push(ServerBodyElem::WorldDeltas(world_changes.clone()));

        // keep track of what we've sent so we can update their baseline when they respond
        client.deltas.insert(server.sequence, world_changes);
    }
}

/// Enqueues all player information to each client
fn enqueue_player_info(
    // With<> for connected players only
    info: Query<(&ClientAddress, &PlayerPosition), With<ConnectedClientInfo>>,
    mut clients: Query<(&ClientAddress, &mut ConnectedClientInfo)>,
) {
    // for each connected client
    for (target_client_addr, mut target_client) in clients.iter_mut() {
        // body for this target client
        let mut players = Vec::new();

        // loop over every connected player info
        for (addr, pos) in info.iter() {
            let player_info = SingleNetPlayerInfo {
                addr: addr.clone(),
                position: pos.clone(),
            };

            if addr.addr == target_client_addr.addr {
                // this is the target player information
                // put it at index 0
                players.insert(0, player_info);
            } else {
                // this is some other player
                // push it toend
                players.push(player_info);
            }
        }

        // enqueue the body
        target_client
            .bodies
            .push(ServerBodyElem::PlayerInfo(players));
    }
}

/// Enqueue player inventory info to each client
fn enqueue_inventory(mut clients: Query<(&mut ConnectedClientInfo, &Inventory)>) {
    for (mut client, inv) in clients.iter_mut() {
        client.bodies.push(ServerBodyElem::Inventory(inv.clone()));
    }
}

/// drop clients (remove ConnectedClientInfo) that haven't responded in a while
fn drop_disconnected_clients(
    mut clients: Query<(Entity, &ClientAddress, &mut ConnectedClientInfo)>,
    mut commands: Commands,
) {
    for (entity, addr, mut client) in clients.iter_mut() {
        // if we need to drop them
        if client.until_drop == 0 {
            warn!("dropping client {}", addr);
            // remove all connected-only components
            commands
                .entity(entity)
                .remove::<ConnectedClientInfo>()
                .remove::<JumpState>()
                .remove::<JumpDuration>();
        } else {
            // in else so we never underflow
            client.until_drop -= 1;
        }
    }
}

/// debug print client info
fn debug_print_players(query: Query<(Entity, &ClientAddress, Option<&ConnectedClientInfo>)>) {
    // print entity, address, and connected
    for (e, addr, connected) in query.iter() {
        info!(
            "e:{}, addr:{}, connected:{}",
            e.id(),
            addr,
            connected.is_some()
        );
    }
}
