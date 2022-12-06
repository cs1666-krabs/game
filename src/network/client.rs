use std::collections::{HashMap, HashSet, VecDeque};
use std::net::{SocketAddr, UdpSocket};

use super::*;
use crate::args::ClientArgs;
use crate::player::client::{spawn_other_player_at, CameraBoundsBox, LocalPlayer, Player};
use crate::player::{
    self, Inventory, PlayerInput, PlayerPosition, CAMERA_BOUNDS_SIZE, PLAYER_AND_BLOCK_SIZE,
};
use crate::states;
use crate::states::client::GameState;
use crate::world::{derender_chunk, render_chunk, RenderedBlock, Terrain, WorldDelta};
use crate::{WIN_H, WIN_W};
use bevy::prelude::*;
use iyes_loopless::prelude::*;

/// Should be used as a global resource on the client
#[derive(Debug)]
struct Client {
    /// UDP socket that should be used for everything
    socket: UdpSocket,
    /// There is only ever one server we care about
    server: SocketAddr,
    /// Our current sequence number
    current_sequence: u64,
    /// Last sequence we received from the server
    last_received_sequence: u64,
    /// Which bodies should be sent in the next outgoing packet
    bodies: Vec<ClientBodyElem>,
    /// Debugging pause: drop all packets in and out, stop any processing
    debug_paused: bool,
    /// TODO: replace this with iyes_loopless fixedtimestep
    real_tick_count: u64,
    /// Network buffer
    buffer: [u8; BUFFER_SIZE],
}

/// Global resource to contain messages, simplifies data path
#[derive(Default)]
struct Messages {
    messages: VecDeque<ServerBodyElem>,
}

impl Client {
    fn new(server_address: SocketAddr, local_port: u16) -> Result<Self, std::io::Error> {
        // port 0 means we let the OS decide
        let addr = SocketAddr::from(([0, 0, 0, 0], local_port));
        let sock = UdpSocket::bind(addr)?;

        // we want nonblocking sockets!
        sock.set_nonblocking(true)?;

        info!("bound socket: {:?}", sock);

        Ok(Self {
            socket: sock,
            server: server_address,
            last_received_sequence: 0,
            current_sequence: 0,
            bodies: Vec::with_capacity(DEFAULT_BODIES_VEC_CAPACITY),
            debug_paused: false,
            real_tick_count: 0,
            buffer: [0u8; BUFFER_SIZE],
        })
    }

    /// Send a message to the server
    fn send_message(&mut self, message: ClientToServer) -> Result<(), SendError> {
        send_message(&self.socket, self.server, message, &mut self.buffer)?;
        Ok(())
    }

    /// Non-blocking way to get one message from the socket
    fn get_one_message(&mut self) -> Result<ServerToClient, ReceiveError> {
        // read from socket
        let (_size, sender_addr) = self.socket.recv_from(&mut self.buffer).map_err(|e| match e
            .kind()
        {
            std::io::ErrorKind::WouldBlock => ReceiveError::NoMessage,
            _ => ReceiveError::IoError(e),
        })?;

        // check if it's actually from the server
        if sender_addr != self.server {
            return Err(ReceiveError::UnknownSender);
        }

        // decode message
        let (message, _size) = bincode::decode_from_slice(&self.buffer, BINCODE_CONFIG)
            .map_err(ReceiveError::DecodeError)?;

        Ok(message)
    }

    /// Push a body that will be sent to the server
    fn enqueue_body(&mut self, body: ClientBodyElem) {
        self.bodies.push(body);
    }
}

pub struct ClientPlugin {
    pub args: ClientArgs,
}

impl Plugin for ClientPlugin {
    fn build(&self, app: &mut App) {
        // add args as a resource
        app.insert_resource(self.args.clone());
        app.insert_resource(Messages::default());

        // enter system
        app.add_enter_system(states::client::GameState::InGame, create_client);

        // exit system
        app.add_exit_system(states::client::GameState::InGame, destroy_client);

        // add timestep
        app.add_fixed_timestep(
            std::time::Duration::from_secs_f64(1. / NETWORK_TICK_HZ as f64),
            NETWORK_TICK_LABEL,
        );

        // input systems (debug)
        app.add_system(
            o_pause_client
                .run_in_state(states::client::GameState::InGame)
                .label("pause"),
        )
        .add_system(
            p_queues_ping
                .run_in_state(states::client::GameState::InGame)
                .label("p_queues_ping"),
        );

        // network timestep systems
        app.add_fixed_timestep_system(
            NETWORK_TICK_LABEL,
            0,
            increase_tick
                .run_in_state(states::client::GameState::InGame)
                .label("increase_tick"),
        )
        .add_fixed_timestep_system(
            NETWORK_TICK_LABEL,
            0,
            fetch_messages
                .run_in_state(states::client::GameState::InGame)
                .label("fetch_messages")
                .after("increase_tick"),
        )
        .add_fixed_timestep_system(
            NETWORK_TICK_LABEL,
            0,
            queue_inputs
                .run_in_state(states::client::GameState::InGame)
                .label("queue_inputs")
                .after("fetch_messages"),
        )
        .add_fixed_timestep_system(
            NETWORK_TICK_LABEL,
            0,
            handle_messages
                .run_in_state(states::client::GameState::InGame)
                .label("handle_messages"),
        )
        .add_fixed_timestep_system(
            NETWORK_TICK_LABEL,
            0,
            send_bodies
                .run_in_state(states::client::GameState::InGame)
                .label("send_bodies")
                .after("handle_messages"),
        )
        .add_fixed_timestep_system(
            NETWORK_TICK_LABEL,
            0,
            client_timeout
                .run_in_state(states::client::GameState::InGame)
                .label("client_timeout")
                .after("send_bodies"),
        );
    }
}

fn create_client(mut commands: Commands, args: Res<ClientArgs>) {
    let client = match Client::new(
        SocketAddr::from((args.server_ip, args.server_port)),
        args.client_port,
    ) {
        Ok(s) => s,
        Err(e) => panic!("Unable to create client: {}", e),
    };
    info!("client created");
    commands.insert_resource(client);
}

fn destroy_client(mut commands: Commands) {
    info!("destroying client");
    commands.remove_resource::<Client>();
}

fn increase_tick(mut client: ResMut<Client>) {
    // don't increment when paused
    if !client.debug_paused {
        client.current_sequence += 1;
        client.real_tick_count += 1;
    }
}

fn o_pause_client(mut client: ResMut<Client>, input: Res<Input<KeyCode>>) {
    if !input.just_pressed(KeyCode::O) {
        return;
    }
    info!("o button pressed");

    client.debug_paused = !client.debug_paused;

    warn!(
        "client now {}paused",
        if client.debug_paused { "" } else { "un" }
    );
}

/// simple system to make P queue up a ping to the server
fn p_queues_ping(mut client: ResMut<Client>, input: Res<Input<KeyCode>>) {
    // return early if P was not pressed
    if !input.just_pressed(KeyCode::P) {
        return;
    }

    if client.debug_paused {
        return;
    }

    let num_ping_bodies = client
        .bodies
        .iter()
        .filter(|b| match b {
            ClientBodyElem::Ping => true,
            ClientBodyElem::Input(_) => false,
        })
        .count();

    // only allow one ping per network cycle
    if num_ping_bodies == 0 {
        info!("client queueing a ping");
        client.enqueue_body(ClientBodyElem::Ping);
    }
}

/// Scrape client inputs and queue up sending them to server
fn queue_inputs(
    mut client: ResMut<Client>,
    bevy_input: Res<Input<KeyCode>>,
    mouse: Res<Input<MouseButton>>,
    mut windows: ResMut<Windows>,
    mut query: Query<(&mut PlayerPosition, &mut CameraBoundsBox), With<LocalPlayer>>,
) {
    // TODO: remove
    if client.debug_paused {
        return;
    }

    //Code to calculate the block x and y to mine based on the mouse x and y from bevy

    let mut block_x_from_mouse = 0;
    let mut block_y_from_mouse = 0;

    let window = windows.get_primary_mut();

    if window.is_none() {
        error!("no window, cannot scrape inputs!");
    }

    let win = window.unwrap();
    let (player_position, camera_box) = query.single();
    let ms = win.cursor_position();

    if !ms.is_none() {
        let mouse_pos = ms.unwrap();

        //calculate distance of click from camera center
        let dist_x = mouse_pos.x - (WIN_W / 2.);
        let dist_y = mouse_pos.y - (WIN_H / 2.);

        //calculate bevy coords of click
        let game_x = camera_box.center_coord.x + dist_x;
        let game_y = camera_box.center_coord.y + dist_y;

        //calculate block coords from bevy coords
        block_x_from_mouse = (game_x / PLAYER_AND_BLOCK_SIZE).round() as usize;
        block_y_from_mouse = (-game_y / PLAYER_AND_BLOCK_SIZE).round() as usize;
    }

    let mut input = PlayerInput {
        left: bevy_input.pressed(KeyCode::A),
        right: bevy_input.pressed(KeyCode::D),
        jump: bevy_input.pressed(KeyCode::Space),
        mine: mouse.pressed(MouseButton::Left),
        block_x: block_x_from_mouse,
        block_y: block_y_from_mouse,
    };

    // TODO: remove
    // DEBUG: make G destroy the block below the player
    if bevy_input.pressed(KeyCode::G) {
        input.mine = true;
        input.block_x = player_position.x as usize;
        input.block_y = (-player_position.y) as usize + 1;
    }

    client.enqueue_body(ClientBodyElem::Input(input));
}

/// Get and handle all messages from server
fn fetch_messages(mut client: ResMut<Client>, mut messages: ResMut<Messages>) {
    if client.debug_paused {
        // eat all the messages
        let mut void = [0u8; 0];
        while client.socket.recv_from(&mut void).is_ok() {}
        return;
    }

    loop {
        match client.get_one_message() {
            Ok(message) => {
                // info!(
                //     "client received message with {} bodies",
                //     message.bodies.len()
                // );
                // only process newer messages, ignore old ones that arrive out of orders
                if message.header.sequence > client.last_received_sequence {
                    // wipe bodies from old packets, since the server is sending deltas anyway
                    messages.messages.clear();

                    // buffer all bodies sent from the server in this packet
                    for body in message.bodies {
                        messages.messages.push_back(body);
                    }

                    // if we are desync'd
                    if client.current_sequence != message.header.sequence {
                        let ticks_ahead =
                            client.current_sequence as i64 - message.header.sequence as i64;
                        let ahead = ticks_ahead > 0;
                        if ticks_ahead.abs() > 5 {
                            warn!(
                                "client out of sync, {} ticks {}!",
                                if ahead { ticks_ahead } else { -ticks_ahead },
                                if ahead { "ahead" } else { "behind" }
                            );
                        }

                        // jump to server's sequence
                        client.current_sequence = message.header.sequence;
                    }

                    // remember the last sequence that we received
                    client.last_received_sequence = message.header.sequence;
                }
            }
            Err(ReceiveError::UnknownSender) => {
                warn!("client got message, but not from server!");
            }
            Err(ReceiveError::NoMessage) => {
                // no more messages at the moment
                break;
            }
            Err(e) => {
                error!("client receive error: {:?}", e);
            }
        }
    }
}

/// Client logic for handling bodies received from the server
/// TODO: improve performance by avoiding copies
fn handle_messages(
    mut messages: ResMut<Messages>,
    mut commands: Commands,
    mut terrain: ResMut<Terrain>,
    mut other_players: Query<
        (Entity, &mut PlayerPosition, &ClientAddress),
        (With<Player>, Without<LocalPlayer>),
    >,
    mut local_player: Query<(&mut PlayerPosition, &mut Sprite, &mut Inventory), With<LocalPlayer>>,
    old_blocks: Query<Entity, With<RenderedBlock>>,
    assets: Res<AssetServer>,
) {
    // new players after this frame, so we can delete old players
    let mut all_players = HashSet::new();
    let mut new_players = HashMap::new();
    let mut got_some_player_info = false;

    while let Some(message) = messages.messages.pop_front() {
        match message {
            ServerBodyElem::Pong(pong) => info!("got pong for seqnum: {}", pong),
            ServerBodyElem::WorldDeltas(deltas) => {
                for delta in deltas {
                    match delta {
                        WorldDelta::NewChunks(new_terrain) => {
                            //
                            info!(
                                "got new completely new chunks!: {:?}",
                                new_terrain
                                    .chunks
                                    .iter()
                                    .map(|c| c.chunk_number)
                                    .collect::<Vec<_>>()
                            );

                            // de-render and destroy old chunks
                            for mut chunk in &mut terrain.chunks {
                                derender_chunk(&mut commands, &mut chunk)
                            }

                            // overwrite the terrain
                            *terrain = new_terrain;

                            // render new chunks
                            for mut chunk in &mut terrain.chunks {
                                render_chunk(&mut commands, &assets, &mut chunk);
                            }
                        }
                        WorldDelta::BlockDelete(delete) => {
                            // info!("got block deletion: {:?}", delete);

                            for chunk in &mut terrain.chunks {
                                if chunk.chunk_number == delete.chunk_number {
                                    let maybe_block = &mut chunk.blocks[delete.y][delete.x];
                                    match maybe_block {
                                        Some(block) => {
                                            // un-render block entity if it exists
                                            if let Some(e) = block.entity {
                                                // info!("despawning mined block");
                                                commands.entity(e).despawn();
                                            }
                                            // delete the block
                                            *maybe_block = None;
                                        }
                                        None => {
                                            // block already deleted
                                            // warn!("client got BlockDelete but block already doesn't exist!");
                                        }
                                    }
                                }
                            }
                        }
                    }

                    // info!("done processing received terrain");
                }
            }
            ServerBodyElem::PlayerInfo(info_vec) => {
                got_some_player_info = true;

                if info_vec.len() >= 1 {
                    let info = &info_vec[0];
                    // info!(
                    //     "new local player position is: ({}, {})",
                    //     info.position.x, info.position.y
                    // );
                    let (mut local_pos, mut local_sprite, _) = local_player.single_mut();

                    // update local player game position, will be rendered in another system
                    *local_pos = info.position.clone();

                    // recolor local player sprite
                    local_sprite.color = info.addr.color();
                }

                // setup non-local players
                if info_vec.len() > 1 {
                    // for each player
                    for info in &info_vec[1..] {
                        // if they already exist, set new position
                        let mut found = false;
                        for (e, mut pos, addr) in other_players.iter_mut() {
                            if info.addr == *addr {
                                *pos = info.position.clone();
                                found = true;
                            }
                        }
                        if !found {
                            // player wasn't found, spawn them in later
                            new_players.insert(info.addr.clone(), info.clone());
                        }
                        // don't despawn this player
                        all_players.insert(info.addr.clone());
                    }
                }

                // info!(
                //     "done processing received player info, len: {}",
                //     info_vec.len()
                // );
            }

            ServerBodyElem::Inventory(new_inv) => {
                // info!(
                //     "Got an inventory message! {:?}",
                //     inv
                // )
                // overwrite our inventory with new one
                let (_, _, mut our_inv) = local_player.single_mut();
                *our_inv = new_inv;
            }
        }
    }

    // spawn in new players
    if new_players.len() > 0 {
        for (_, player) in new_players {
            // spawn new entity with Player and transform at location
            spawn_other_player_at(
                &mut commands,
                assets.as_ref(),
                &player.addr,
                &player.position,
            );
            warn!("new player {}", player.addr);
        }
    }

    // if we actually got some player info this frame
    if got_some_player_info {
        // for all previously spawned players
        for (e, _pos, addr) in other_players.iter() {
            // if we didn't hear about them this frame
            if !all_players.contains(addr) {
                // delete
                commands.entity(e).despawn();
                warn!("delete player {}", addr);
            }
        }
    }
}

fn send_bodies(mut client: ResMut<Client>) {
    if client.debug_paused {
        return;
    }

    let message = ClientToServer {
        header: ClientHeader {
            current_sequence: client.current_sequence,
            last_received_sequence: client.last_received_sequence,
        },
        bodies: client.bodies.clone(),
    };
    let success_str = format!("client sent message to server: {:?}", message);
    match client.send_message(message) {
        Ok(_) => {
            // info!("{}", success_str),
        }
        Err(e) => error!("failed to send message to server: {:?}", e),
    }

    // client doesn't care if message arrives -- it never retransmits bodies
    client.bodies.clear();
}

// TODO: client-side timeout!
fn client_timeout(client: ResMut<Client>, commands: Commands) {
    if client.debug_paused {
        return;
    }
    let timeout = client.current_sequence - client.last_received_sequence
        >= FRAME_DIFFERENCE_BEFORE_DISCONNECT;
    if timeout {
        error!("Client Timeout");
        on_timeout(client, commands);
    }
}

//TODO: clean up after a timeout
fn on_timeout(mut client: ResMut<Client>, mut commands: Commands) {
    info!("Clearing bodies");
    client.bodies.clear();
    // go back to menu
    commands.insert_resource(NextState(GameState::Menu));
}
